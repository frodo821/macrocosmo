//! Regression test: NPC AI must filter `colonizable_systems` by Core
//! presence. Without this gate, the settling handler rejects
//! `colonize_system` orders for systems where the empire has no Core
//! deployed (#299 Core sovereignty check), and the AI re-emits the same
//! impossible order every decision tick.
//!
//! See `ai/npc_decision.rs::npc_decision_tick` and the precomputed
//! `core_systems_per_empire` map. Long-term plan (`gh issue #446` /
//! `#447`): give the AI explicit `deploy_core` commands and let the
//! Short layer decompose colonize → deploy + colonize.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::command_outbox::AiCommandOutbox;
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::components::Position;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::AtSystem;
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, SystemKnowledge, SystemSnapshot, SystemVisibilityMap,
    SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{CoreShip, Owner, Ship};

use common::{advance_time, spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

/// Spawn an AI-controlled empire with one colony ship at `home`. The
/// empire's `KnowledgeStore` is seeded so `home` and `target` are both
/// known surveyed; `target` is uncolonized — i.e. it would be a valid
/// colonization candidate *except* for the Core gate.
fn setup_colonizer_scenario(app: &mut App) -> (Entity, Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Vesk".into(),
            },
            PlayerEmpire,
            Faction {
                id: "vesk".into(),
                name: "Vesk".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let target = spawn_test_system(app.world_mut(), "Target", [0.5, 0.0, 0.0], 1.0, true, false);

    spawn_test_ruler(app.world_mut(), empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(target, SystemVisibilityTier::Surveyed);
    }

    let home_pos = [0.0, 0.0, 0.0];
    let target_pos = [0.5, 0.0, 0.0];
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update(SystemKnowledge {
            system: home,
            observed_at: 0,
            received_at: 0,
            data: SystemSnapshot {
                name: "Home".into(),
                position: home_pos,
                surveyed: true,
                colonized: true,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
        store.update(SystemKnowledge {
            system: target,
            observed_at: 0,
            received_at: 0,
            data: SystemSnapshot {
                name: "Target".into(),
                position: target_pos,
                surveyed: true,
                colonized: false,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
    }

    let colony_ship = spawn_test_ship(
        app.world_mut(),
        "Colonizer-1",
        "colony_ship_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(colony_ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    (empire, home, target, colony_ship)
}

/// Returns `true` if there's an in-flight `colonize_system` AI command
/// for the given target system OR the issuing empire has stamped a
/// `PendingAssignment::Colonize` marker on a ship for this target.
///
/// Surfaces checked:
/// 1. `AiCommandOutbox.entries` — pre-#468-PR2 light-speed shim (still
///    used by non-migrated kinds).
/// 2. `PendingAiShipCommand` — #468 PR-2 per-ship pipeline. With
///    Ruler→ship = 0 light delay the holder is drained the same tick
///    it's spawned, so this surface only catches the very-narrow
///    in-flight window.
/// 3. `PendingAssignment::Colonize` — stamped on the ship at dispatch
///    time and removed once the issuing empire learns the target is
///    colonized. This is the durable "the AI committed to this target"
///    surface that survives the courier window collapsing to zero.
fn outbox_has_colonize_for(app: &mut App, target_system: Entity) -> bool {
    use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
    let outbox = app.world().resource::<AiCommandOutbox>();
    let outbox_hit = outbox.entries.iter().any(|entry| {
        let cmd = &entry.command;
        if cmd.kind != cmd_ids::colonize_system() {
            return false;
        }
        match cmd.params.get("target_system") {
            Some(macrocosmo_ai::CommandValue::System(sys_id)) => {
                target_system.to_bits() == sys_id.0
            }
            _ => false,
        }
    });
    if outbox_hit {
        return true;
    }
    // #468 PR-2: colonize_system migrated to PendingAiShipCommand.
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    let in_flight = {
        let mut q = app.world_mut().query::<&PendingAiShipCommand>();
        q.iter(app.world())
            .any(|p| p.kind == cmd_ids::colonize_system() && p.target_system == target_system)
    };
    if in_flight {
        return true;
    }
    // Marker surface: dispatch stamps `PendingAssignment::Colonize`
    // BEFORE the holder matures, so even with zero light-delay (Ruler
    // at the ship) this is a durable signal that the AI committed.
    let mut q = app.world_mut().query::<&PendingAssignment>();
    q.iter(app.world()).any(|pa| {
        matches!(pa.kind, AssignmentKind::Colonize)
            && matches!(pa.target, AssignmentTarget::System(t) if t == target_system)
    })
}

/// Spawn a CoreShip directly on `system`, owned by `empire`.
fn place_core_at(world: &mut World, empire: Entity, system: Entity, position: [f64; 3]) -> Entity {
    let pos = Position::from(position);
    world
        .spawn((
            Ship {
                name: "Core".to_string(),
                design_id: "infrastructure_core_v1".to_string(),
                hull_id: "infrastructure_core_hull".to_string(),
                modules: Vec::new(),
                owner: Owner::Empire(empire),
                sublight_speed: 0.0,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: system,
                design_revision: 0,
                fleet: None,
            },
            macrocosmo::ship::ShipState::InSystem { system },
            pos,
            macrocosmo::ship::ShipHitpoints {
                hull: 400.0,
                hull_max: 400.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            macrocosmo::ship::CommandQueue::default(),
            macrocosmo::ship::Cargo::default(),
            macrocosmo::ship::ShipModifiers::default(),
            macrocosmo::ship::ShipStats::default(),
            macrocosmo::ship::RulesOfEngagement::default(),
            CoreShip,
            AtSystem(system),
            FactionOwner(empire),
        ))
        .id()
}

#[test]
fn colonize_system_not_emitted_when_target_lacks_core() {
    // Colonizable system with no Core deployed must not draw a
    // `colonize_system` command — the settling handler would reject it
    // and the AI would loop forever.
    let mut app = test_app();
    let (_empire, _home, target, _colonizer) = setup_colonizer_scenario(&mut app);

    // Drive a few ticks so `npc_decision_tick` runs at least once.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    assert!(
        !outbox_has_colonize_for(&mut app, target),
        "AI emitted colonize_system for a target with no own Core present \
         — this is the loop bug the Core-presence filter must close."
    );
}

#[test]
fn colonize_system_emitted_when_target_has_core() {
    // Positive control: same scenario but the empire has already deployed
    // a Core at `target` ⇒ the AI should emit `colonize_system`.
    let mut app = test_app();
    let (empire, _home, target, _colonizer) = setup_colonizer_scenario(&mut app);

    place_core_at(app.world_mut(), empire, target, [0.5, 0.0, 0.0]);

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    assert!(
        outbox_has_colonize_for(&mut app, target),
        "AI must emit colonize_system when own Core is present at the target"
    );
}
