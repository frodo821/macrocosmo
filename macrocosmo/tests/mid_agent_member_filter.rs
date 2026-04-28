//! #449 PR2b: when a single empire owns more than one `Region` (each
//! with its own `MidAgent`), each Mid sees only the systems in **its**
//! `member_systems` slice. Cross-region intel does not bleed into the
//! "wrong" Mid's decision context.
//!
//! Today the production spawn path always creates exactly one Region
//! per empire (= legacy NPC behavior preserved bit-for-bit), so this
//! test hand-builds a 2-region empire to exercise the filter directly.
//! Once #449 PR2c+ wires region splits, this becomes a regression
//! guard for the live multi-region case.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::command_outbox::AiCommandOutbox;
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::ai::{AiPlayerMode, MidAgent};
use macrocosmo::components::Position;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::AtSystem;
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, SystemKnowledge, SystemSnapshot, SystemVisibilityMap,
    SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::region::{Region, RegionMembership, RegionRegistry};
use macrocosmo::ship::{CoreShip, Owner, Ship};

use common::{advance_time, spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

/// Place an Infrastructure Core at `system` so the colonize gate (#299)
/// recognises it as a candidate. Mirrors the helper in
/// `tests/ai_npc_outbox_dedup.rs`.
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

/// Spawn a 2-region empire with a colony-target system in each region.
/// Returns `(empire, region_a, region_b, system_a_target, system_b_target)`.
fn setup_two_region_empire(
    app: &mut App,
) -> (Entity, Entity, Entity, Entity, Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Aurelian".into(),
            },
            PlayerEmpire,
            Faction {
                id: "aurelian".into(),
                name: "Aurelian".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
        ))
        .id();

    // Two home systems, far apart, each with a colony candidate next to it.
    // Region A: home_a + target_a (close); Region B: home_b + target_b.
    let home_a = spawn_test_system(app.world_mut(), "HomeA", [0.0, 0.0, 0.0], 1.0, true, true);
    let target_a = spawn_test_system(
        app.world_mut(),
        "TargetA",
        [0.5, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let home_b = spawn_test_system(app.world_mut(), "HomeB", [100.0, 0.0, 0.0], 1.0, true, true);
    let target_b = spawn_test_system(
        app.world_mut(),
        "TargetB",
        [100.5, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    spawn_test_ruler(app.world_mut(), empire, home_a);

    // Knowledge: empire knows all four systems, both targets surveyed +
    // un-colonized so Rule 3 (colonize) sees them as candidates.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        for sys in [home_a, target_a, home_b, target_b] {
            vis.set(sys, SystemVisibilityTier::Surveyed);
        }
    }
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        let mut record = |sys: Entity, name: &str, pos: [f64; 3], colonized: bool| {
            store.update(SystemKnowledge {
                system: sys,
                observed_at: 0,
                received_at: 0,
                data: SystemSnapshot {
                    name: name.into(),
                    position: pos,
                    surveyed: true,
                    colonized,
                    ..Default::default()
                },
                source: ObservationSource::Direct,
            });
        };
        record(home_a, "HomeA", [0.0, 0.0, 0.0], true);
        record(target_a, "TargetA", [0.5, 0.0, 0.0], false);
        record(home_b, "HomeB", [100.0, 0.0, 0.0], true);
        record(target_b, "TargetB", [100.5, 0.0, 0.0], false);
    }

    // Cores in both target systems (#299 gate).
    place_core_at(app.world_mut(), empire, target_a, [0.5, 0.0, 0.0]);
    place_core_at(app.world_mut(), empire, target_b, [100.5, 0.0, 0.0]);

    // Build the two regions explicitly (bypass the auto-backfill —
    // we want a controlled multi-region layout).
    if app.world().get_resource::<RegionRegistry>().is_none() {
        app.world_mut().insert_resource(RegionRegistry::default());
    }
    let region_a = app
        .world_mut()
        .spawn(Region {
            empire,
            member_systems: vec![home_a, target_a],
            capital_system: home_a,
            mid_agent: None,
        })
        .id();
    let region_b = app
        .world_mut()
        .spawn(Region {
            empire,
            member_systems: vec![home_b, target_b],
            capital_system: home_b,
            mid_agent: None,
        })
        .id();
    app.world_mut()
        .entity_mut(home_a)
        .insert(RegionMembership { region: region_a });
    app.world_mut()
        .entity_mut(target_a)
        .insert(RegionMembership { region: region_a });
    app.world_mut()
        .entity_mut(home_b)
        .insert(RegionMembership { region: region_b });
    app.world_mut()
        .entity_mut(target_b)
        .insert(RegionMembership { region: region_b });
    {
        let mut reg = app.world_mut().resource_mut::<RegionRegistry>();
        reg.by_empire
            .entry(empire)
            .or_default()
            .extend([region_a, region_b]);
    }

    let mid_a = app
        .world_mut()
        .spawn(MidAgent {
            region: region_a,
            state: macrocosmo_ai::MidTermState::default(),
            auto_managed: true,
        })
        .id();
    let mid_b = app
        .world_mut()
        .spawn(MidAgent {
            region: region_b,
            state: macrocosmo_ai::MidTermState::default(),
            auto_managed: true,
        })
        .id();
    app.world_mut()
        .get_mut::<Region>(region_a)
        .unwrap()
        .mid_agent = Some(mid_a);
    app.world_mut()
        .get_mut::<Region>(region_b)
        .unwrap()
        .mid_agent = Some(mid_b);

    (
        empire, region_a, region_b, home_a, target_a, home_b, target_b,
    )
}

/// Count outbox entries of `kind` whose `target_system` matches.
fn count_outbox_for(app: &App, kind: macrocosmo_ai::CommandKindId, target: Entity) -> usize {
    let outbox = app.world().resource::<AiCommandOutbox>();
    outbox
        .entries
        .iter()
        .filter(|entry| {
            let cmd = &entry.command;
            if cmd.kind != kind {
                return false;
            }
            match cmd.params.get("target_system") {
                Some(macrocosmo_ai::CommandValue::System(sys_id)) => target.to_bits() == sys_id.0,
                _ => false,
            }
        })
        .count()
}

#[test]
fn each_mid_agent_only_dispatches_within_its_own_region() {
    let mut app = test_app();
    let (empire, _ra, _rb, home_a, target_a, home_b, target_b) = setup_two_region_empire(&mut app);

    // Two colony ships: one in region A's home, one in region B's home.
    let colony_a = spawn_test_ship(
        app.world_mut(),
        "ColonyA",
        "colony_ship_mk1",
        home_a,
        [0.0, 0.0, 0.0],
    );
    let colony_b = spawn_test_ship(
        app.world_mut(),
        "ColonyB",
        "colony_ship_mk1",
        home_b,
        [100.0, 0.0, 0.0],
    );
    for s in [colony_a, colony_b] {
        app.world_mut()
            .entity_mut(s)
            .get_mut::<Ship>()
            .unwrap()
            .owner = Owner::Empire(empire);
    }

    // Drive enough ticks for both Mids to fire at least once.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    // Each Mid must have dispatched a colonize_system command at the
    // target inside ITS region only. Cross-region targets must stay
    // empty (no leakage).
    let target_a_count = count_outbox_for(&app, cmd_ids::colonize_system(), target_a);
    let target_b_count = count_outbox_for(&app, cmd_ids::colonize_system(), target_b);
    assert!(
        target_a_count >= 1,
        "Mid A must dispatch colonize on target_a (region A); got {}",
        target_a_count
    );
    assert!(
        target_b_count >= 1,
        "Mid B must dispatch colonize on target_b (region B); got {}",
        target_b_count
    );

    // Each colonize command's `ship_0` parameter must be the ship from
    // the same region (no cross-region ship reuse).
    let outbox = app.world().resource::<AiCommandOutbox>();
    let cmd_kind = cmd_ids::colonize_system();
    for entry in outbox.entries.iter() {
        let cmd = &entry.command;
        if cmd.kind != cmd_kind {
            continue;
        }
        let target_sys = match cmd.params.get("target_system") {
            Some(macrocosmo_ai::CommandValue::System(s)) => Entity::from_bits(s.0),
            _ => continue,
        };
        let ship = match cmd.params.get("ship_0") {
            Some(macrocosmo_ai::CommandValue::Entity(e)) => Entity::from_bits(e.0),
            _ => continue,
        };
        if target_sys == target_a {
            assert_eq!(
                ship, colony_a,
                "colonize on target_a must use the region-A ship (colony_a); \
                 got ship={:?}, expected {:?} — cross-region leakage",
                ship, colony_a
            );
        } else if target_sys == target_b {
            assert_eq!(
                ship, colony_b,
                "colonize on target_b must use the region-B ship (colony_b); \
                 got ship={:?}, expected {:?} — cross-region leakage",
                ship, colony_b
            );
        }
    }
}
