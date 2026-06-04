//! #444 regression tests — AI region-deadlock hotfix.
//!
//! Before this hotfix a starter NPC empire (one capital, capital
//! surveyed + colonised + cored, region.member_systems = {capital})
//! emitted **zero** AI commands for ship movement:
//!
//! * Rule 3 (colonize): `colonizable_systems` was empty — every
//!   "surveyed, not colonised, own-Core" filter applied to a region
//!   with exactly one already-colonised system trivially collapses
//!   the candidate set.
//! * Rule 2 (survey): `candidates` was empty — the region-scope
//!   filter `member_systems_set.contains(e)` excluded every
//!   non-capital, and the capital is by definition surveyed.
//! * No `deploy_deliverable(infra_core)` path existed at all, so the
//!   AI could never plant a new Core anywhere — meaning Rule 3 never
//!   acquires a candidate that passes the own-Core gate.
//!
//! The hotfix lands three coupled changes:
//!
//! 1. **Rule 3.5** (`mid_stance.rs`): emit
//!    `deploy_deliverable(infrastructure_core)` for surveyed,
//!    not-owned systems, paired with idle courier ships.
//! 2. **Survey region-scope filter removed** (`npc_decision.rs`):
//!    Rule 2 is once again galaxy-wide — surveyed targets feed
//!    Rule 3.5's frontier candidate set the next tick.
//! 3. **Eager macro decomposition in `dispatch_ai_pending_commands`**
//!    (`command_outbox.rs`): `deploy_deliverable` (and other future
//!    macros that the Short layer doesn't claim) get expanded to
//!    `build_deliverable → load_deliverable → reposition →
//!    unload_deliverable` before routing, so the chain actually
//!    moves a ship.
//!
//! These four tests pin those three changes from the AI bus's point of
//! view — they do not depend on Lua scripts, so they run with the same
//! lightweight `test_app()` harness the rest of the AI suite uses.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::MidAgent;
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
use macrocosmo::region::{Region, RegionMembership, RegionRegistry};
use macrocosmo::ship::{CoreShip, Owner, Ship};

use common::{advance_time, spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

/// Spawn a starter NPC empire with one capital, surveyed + colonised +
/// cored, plus a frontier system that is surveyed but not owned. Adds
/// one idle colony ship at the capital to act as a courier candidate.
///
/// Returns `(empire, capital, frontier, colony_ship)`.
///
/// **Region scope**: the empire's `Region.member_systems` is explicitly
/// `{capital}` only — the frontier system is intentionally outside the
/// region scope to exercise Rule 3.5. This bypasses the auto-backfill
/// (`backfill_mid_agents_for_ai_controlled`) which would otherwise pull
/// every `StarSystem` into a single region, defeating the frontier
/// filter.
fn spawn_starter_with_frontier(app: &mut App) -> (Entity, Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    // Inject the `infrastructure_core` deliverable into the test
    // design registry — required by `handle_build_deliverable`'s
    // registry.get() lookup. The default `create_test_design_registry`
    // only ships flying ship designs; without this entry the
    // dispatched `build_deliverable` rejects with "unknown deliverable
    // definition" and the chain dies silently.
    register_test_infrastructure_core(app);

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

    let capital = spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [0.5, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    // Mark capital as the empire's home system. Required by
    // `resolve_capital_system` (called from
    // `dispatch_ai_pending_commands` for spatial-less commands like
    // `build_deliverable`) — without it the dispatcher drops the
    // command with a "no destination" warn.
    app.world_mut()
        .entity_mut(empire)
        .insert(macrocosmo::galaxy::HomeSystem(capital));

    spawn_test_ruler(app.world_mut(), empire, capital);

    // Make the empire know both systems exist, and treat the capital
    // as colonised + already-cored.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(capital, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Surveyed);
    }
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update(SystemKnowledge {
            system: capital,
            observed_at: 0,
            received_at: 0,
            data: SystemSnapshot {
                name: "Capital".into(),
                position: [0.0, 0.0, 0.0],
                surveyed: true,
                colonized: true,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
        store.update(SystemKnowledge {
            system: frontier,
            observed_at: 0,
            received_at: 0,
            data: SystemSnapshot {
                name: "Frontier".into(),
                position: [0.5, 0.0, 0.0],
                surveyed: true,
                colonized: false,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
    }

    // Plant a Core at the capital so it counts as owned (matches the
    // production spawn pipeline that places `infrastructure_core_v1`
    // at the starter capital).
    place_core_at(app.world_mut(), empire, capital, [0.0, 0.0, 0.0]);

    // Manually mark capital sovereignty as owned by the empire so the
    // `handle_build_deliverable` owned-systems gate finds it on tick 1.
    // The production `update_sovereignty` system writes the same field
    // from the CoreShip ownership, but it doesn't run before
    // `dispatch_ai_pending_commands` on the first tick of a hand-built
    // test world.
    {
        use macrocosmo::galaxy::Sovereignty;
        let mut em = app.world_mut().entity_mut(capital);
        if let Some(mut sov) = em.get_mut::<Sovereignty>() {
            sov.owner = Some(Owner::Empire(empire));
        }
    }

    // Drop a real `Colony` at the capital's planet. Without this,
    // `propagate_knowledge` writes `colonized = false` into the
    // capital's `SystemSnapshot` every tick (overwriting the manual
    // snapshot above) and the colonisable filter then picks the
    // capital as a Rule 3 candidate (capital has Core, member of
    // region, not colonised by snapshot) — which steals the courier
    // ship from Rule 3.5.
    let colony = common::spawn_test_colony(
        app.world_mut(),
        capital,
        macrocosmo_core::amount::Amt::units(100),
        macrocosmo_core::amount::Amt::units(100),
        vec![],
    );
    // `spawn_test_colony` picks the FIRST empire it finds, which in
    // `test_app()` is the auto-spawned "Test Empire" rather than our
    // hand-built `Vesk`. Re-stamp `FactionOwner` so
    // `pick_host_colony` sees our empire as the colony's owner.
    app.world_mut()
        .entity_mut(colony)
        .insert(FactionOwner(empire));

    // Idle colony ship at capital — Rule 3.5's courier candidate.
    let ship = spawn_test_ship(
        app.world_mut(),
        "Colonizer-1",
        "colony_ship_mk1",
        capital,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    // Build the Region + MidAgent pair manually so member_systems is
    // strictly `{capital}` — the frontier system stays outside.
    install_capital_only_region(app, empire, capital);

    (empire, capital, frontier, ship)
}

/// Explicitly build a Region/MidAgent pair for `empire` whose
/// `member_systems = vec![capital]`. Mirrors the setup pattern in
/// `tests/mid_agent_member_filter.rs`, scoped to a single region.
fn install_capital_only_region(app: &mut App, empire: Entity, capital: Entity) {
    if app.world().get_resource::<RegionRegistry>().is_none() {
        app.world_mut().insert_resource(RegionRegistry::default());
    }
    let region = app
        .world_mut()
        .spawn(Region {
            empire,
            member_systems: vec![capital],
            capital_system: capital,
            mid_agent: None,
        })
        .id();
    app.world_mut()
        .entity_mut(capital)
        .insert(RegionMembership { region });
    {
        let mut reg = app.world_mut().resource_mut::<RegionRegistry>();
        reg.by_empire.entry(empire).or_default().push(region);
    }
    let mid = app
        .world_mut()
        .spawn(MidAgent {
            region,
            state: macrocosmo_ai::MidTermState::default(),
            auto_managed: true,
        })
        .id();
    app.world_mut().get_mut::<Region>(region).unwrap().mid_agent = Some(mid);
}

/// Register a minimal `infrastructure_core` deliverable definition in
/// the test deliverable registry. Mirrors the production
/// `scripts/structures/cores.lua` shape (id matches; cost / build_time
/// scaled down so tests don't need to advance the BuildQueue out).
/// Called from each test's setup before any `advance_time` so the
/// dispatched `build_deliverable` lookup succeeds.
///
/// #532 F1: this used to inject a fake `ShipDesignDefinition` with the
/// deliverable id because `handle_build_deliverable` previously looked
/// up `ShipDesignRegistry`. After F1 the handler resolves via
/// `DeliverableRegistry`, so the test fixture has to inject a real
/// `DeliverableDefinition` instead — otherwise these tests would now
/// pass the bug-free production handler an unknown deliverable.
fn register_test_infrastructure_core(app: &mut App) {
    use macrocosmo::deep_space::{
        DeliverableMetadata, DeliverableRegistry, ResourceCost, StructureDefinition,
    };
    use macrocosmo_core::amount::Amt;
    let mut registry = app
        .world_mut()
        .remove_resource::<DeliverableRegistry>()
        .unwrap_or_default();
    registry.insert(StructureDefinition {
        id: "infrastructure_core".into(),
        name: "Infrastructure Core".into(),
        description: String::new(),
        max_hp: 400.0,
        energy_drain: Amt::ZERO,
        capabilities: std::collections::HashMap::new(),
        prerequisites: None,
        deliverable: Some(DeliverableMetadata {
            cost: ResourceCost {
                minerals: Amt::units(1),
                energy: Amt::units(1),
            },
            build_time: 1,
            cargo_size: 1,
            scrap_refund: 0.25,
            // Test fixture: in production this points at
            // `infrastructure_core_v1` (the immobile core ship design).
            // The deploy / spawn pipeline is not exercised by these
            // AI-bus tests, so we leave it `None`.
            spawns_as_ship: None,
        }),
        upgrade_to: Vec::new(),
        upgrade_from: None,
        on_built: None,
        on_upgraded: None,
    });
    app.insert_resource(registry);
}

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

/// Surface check: is there evidence that a `deploy_deliverable` chain
/// was emitted for `target_system`? Checks three surfaces (any one
/// sufficient, in cost order):
///
/// 1. `AiCommandOutbox` entries — courier-window-resident macros /
///    `build_deliverable`.
/// 2. `PendingAiShipCommand` holders — courier-window-resident
///    primitives (`load_deliverable`, `reposition`,
///    `unload_deliverable`).
/// 3. `BuildQueue` deliverable orders — `handle_build_deliverable` has
///    already queued the chain's first phase at an owned colony.
///
/// Surface (3) is the durable signal: light-delay-zero test scenarios
/// drain (1) and (2) the same tick they appear, but the BuildQueue
/// order outlives the tick and survives across the
/// `advance_time / app.update()` boundary. The ship-CommandQueue
/// surface was intentionally NOT included — it produces false
/// positives when a parallel `colonize_system` (Rule 3) writes orders
/// into the same ship, which the double-book test specifically wants
/// to distinguish.
fn deploy_chain_in_flight_for(app: &mut App, target_system: Entity) -> bool {
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    let outbox = app.world().resource::<AiCommandOutbox>();
    let target_bits = target_system.to_bits();

    // 1) AiCommandOutbox entries — covers the build/macro phase plus
    //    spatial-less `build_deliverable`.
    let outbox_hit = outbox.entries.iter().any(|entry| {
        let cmd = &entry.command;
        let kind = cmd.kind.as_str();
        if !(kind == cmd_ids::deploy_deliverable().as_str()
            || kind == cmd_ids::build_deliverable().as_str()
            || kind == cmd_ids::load_deliverable().as_str()
            || kind == cmd_ids::reposition().as_str()
            || kind == cmd_ids::unload_deliverable().as_str())
        {
            return false;
        }
        match cmd.params.get("target_system") {
            Some(macrocosmo_ai::CommandValue::System(s)) => s.0 == target_bits,
            _ => false,
        }
    });
    if outbox_hit {
        return true;
    }

    // 2) PendingAiShipCommand — covers the per-ship pipeline after the
    //    dispatcher has stamped the holder.
    let mut q = app.world_mut().query::<&PendingAiShipCommand>();
    let pipeline_hit = q.iter(app.world()).any(|p| {
        let k = p.kind.as_str();
        (k == cmd_ids::deploy_deliverable().as_str()
            || k == cmd_ids::load_deliverable().as_str()
            || k == cmd_ids::reposition().as_str()
            || k == cmd_ids::unload_deliverable().as_str())
            && p.target_system == target_system
    });
    if pipeline_hit {
        return true;
    }

    // 3) BuildQueue — `handle_build_deliverable` queues a Deliverable
    //    BuildOrder on the owned colony. The order outlives the tick
    //    and survives `advance_time` re-entry.
    use macrocosmo::colony::{BuildKind, BuildQueue};
    let mut bq = app.world_mut().query::<&BuildQueue>();
    let build_hit = bq.iter(app.world()).any(|q| {
        q.queue.iter().any(|o| {
            matches!(o.kind, BuildKind::Deliverable { .. })
                && (o.design_id == "infrastructure_core" || o.design_id == "infra_core")
        })
    });
    if build_hit {
        return true;
    }

    false
}

/// Survey-target inspection: is there any AI-committed survey signal
/// for `target_system` across the four post-dispatch surfaces?
///   1. `AiCommandOutbox.entries` (courier-window-resident),
///   2. `PendingAiShipCommand` holder,
///   3. `PendingAssignment::Survey` marker (durable),
///   4. The fact that no test ship is colony-capable but at least one
///      is in transit / has a non-empty queue is the catch-all signal
///      used by the existing dedup helpers.
///
/// Surface (3) is the durable signal — `apply_survey_to_ship` stamps
/// the marker before despawning the holder, so it survives a
/// zero-light-delay dispatch.
fn outbox_has_survey_for(app: &mut App, target_system: Entity) -> bool {
    use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    let outbox = app.world().resource::<AiCommandOutbox>();
    let target_bits = target_system.to_bits();
    let outbox_hit = outbox.entries.iter().any(|entry| {
        let cmd = &entry.command;
        if cmd.kind != cmd_ids::survey_system() {
            return false;
        }
        match cmd.params.get("target_system") {
            Some(macrocosmo_ai::CommandValue::System(s)) => s.0 == target_bits,
            _ => false,
        }
    });
    if outbox_hit {
        return true;
    }
    let mut q = app.world_mut().query::<&PendingAiShipCommand>();
    if q.iter(app.world())
        .any(|p| p.kind == cmd_ids::survey_system() && p.target_system == target_system)
    {
        return true;
    }
    let mut pq = app.world_mut().query::<&PendingAssignment>();
    pq.iter(app.world()).any(|pa| {
        matches!(pa.kind, AssignmentKind::Survey)
            && matches!(pa.target, AssignmentTarget::System(t) if t == target_system)
    })
}

#[test]
fn mid_emits_deploy_deliverable_for_unowned_surveyed_system() {
    // Starter empire with a surveyed-but-not-owned frontier system +
    // an idle colony ship at the capital. Rule 3.5 should fire and
    // emit `deploy_deliverable(infrastructure_core)` targeting the
    // frontier; the dispatcher's eager macro expansion turns that into
    // the build/load/reposition/unload chain.
    let mut app = test_app();
    let (_empire, _capital, frontier, _ship) = spawn_starter_with_frontier(&mut app);

    // Drive a few ticks so `npc_decision_tick` + dispatcher both run.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    assert!(
        deploy_chain_in_flight_for(&mut app, frontier),
        "Rule 3.5 must emit deploy_deliverable (or its decomposed primitives) \
         for the frontier system — without it the AI never plants a new Core."
    );
}

#[test]
fn survey_fires_galaxy_wide_not_region_bound() {
    // Three-system galaxy: capital (owned), frontier (surveyed,
    // unowned, in the region's member set by default because the
    // backfill copies `member_systems = all StarSystems`), and a
    // remote unsurveyed system. Pre-hotfix the region-scope filter on
    // Rule 2 would skip every unsurveyed star outside the region; the
    // hotfix lifts that filter so `unsurveyed_systems` finds the
    // remote target.
    let mut app = test_app();
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

    let capital = spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let _surveyed_neighbor = spawn_test_system(
        app.world_mut(),
        "Neighbor",
        [0.3, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let remote = spawn_test_system(app.world_mut(), "Remote", [3.0, 0.0, 0.0], 1.0, true, false);

    spawn_test_ruler(app.world_mut(), empire, capital);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(capital, SystemVisibilityTier::Local);
        // Both other systems are "Catalogued" (so they show up as
        // unsurveyed targets) — not Surveyed.
        vis.set(_surveyed_neighbor, SystemVisibilityTier::Catalogued);
        vis.set(remote, SystemVisibilityTier::Catalogued);
    }
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update(SystemKnowledge {
            system: capital,
            observed_at: 0,
            received_at: 0,
            data: SystemSnapshot {
                name: "Capital".into(),
                position: [0.0, 0.0, 0.0],
                surveyed: true,
                colonized: true,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
    }
    place_core_at(app.world_mut(), empire, capital, [0.0, 0.0, 0.0]);

    // Surveyor at the capital so Rule 2 has a ship to dispatch with.
    let scout = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        capital,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(scout)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    // Capital-only region — without this, the backfill makes
    // member_systems = {all StarSystems}, and the pre-hotfix region
    // filter would have included `_surveyed_neighbor` and `remote` in
    // the survey candidate set anyway, defeating the purpose of the
    // test.
    install_capital_only_region(&mut app, empire, capital);

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    // The hotfix removed the region-scope filter on `candidates`, so
    // both the `_surveyed_neighbor` and the `remote` system are
    // legitimate Rule 2 targets. Assert the AI dispatched at least one
    // survey — galaxy-wide visibility, not region-restricted.
    let neighbor_hit = outbox_has_survey_for(&mut app, _surveyed_neighbor);
    let remote_hit = outbox_has_survey_for(&mut app, remote);
    assert!(
        neighbor_hit || remote_hit,
        "Survey command must reach at least one unsurveyed system \
         (neighbor or remote) — pre-hotfix the region filter pinned the AI \
         to an empty candidate set."
    );
}

#[test]
fn no_double_deploy_during_courier_window() {
    // After Rule 3.5 emits once for the frontier, a second
    // `npc_decision_tick` on the next hexadies must NOT emit a fresh
    // `deploy_deliverable` for the same target — the in-flight outbox
    // entry (or per-ship pipeline holder) must short-circuit the
    // dedup gate. Without this the AI would re-spend the courier
    // every tick and pile up parallel build orders.
    let mut app = test_app();
    let (_empire, _capital, frontier, _ship) = spawn_starter_with_frontier(&mut app);

    // First tick: Rule 3.5 fires.
    for _ in 0..2 {
        advance_time(&mut app, 1);
    }
    assert!(
        deploy_chain_in_flight_for(&mut app, frontier),
        "Setup precondition: first tick must emit the chain."
    );

    // Count "in-flight or committed" `deploy_deliverable` chain
    // entries: outbox + per-ship pipeline + colony BuildQueue
    // deliverable orders. The BuildQueue surface is critical for
    // zero-light-delay tests — outbox + pipeline drain the same tick
    // they're spawned, but the BuildQueue order is durable.
    let count_chain_entries = |app: &mut App| -> usize {
        use macrocosmo::ai::command_consumer::PendingAiShipCommand;
        use macrocosmo::colony::{BuildKind, BuildQueue};
        let target_bits = frontier.to_bits();
        let outbox = app.world().resource::<AiCommandOutbox>();
        let outbox_count = outbox
            .entries
            .iter()
            .filter(|entry| {
                let kind = entry.command.kind.as_str();
                kind == cmd_ids::deploy_deliverable().as_str()
                    || kind == cmd_ids::build_deliverable().as_str()
                    || kind == cmd_ids::load_deliverable().as_str()
                    || kind == cmd_ids::unload_deliverable().as_str()
            })
            .filter(|entry| match entry.command.params.get("target_system") {
                Some(macrocosmo_ai::CommandValue::System(s)) => s.0 == target_bits,
                _ => false,
            })
            .count();
        let mut q = app.world_mut().query::<&PendingAiShipCommand>();
        let pipeline_count = q
            .iter(app.world())
            .filter(|p| {
                let k = p.kind.as_str();
                (k == cmd_ids::deploy_deliverable().as_str()
                    || k == cmd_ids::load_deliverable().as_str()
                    || k == cmd_ids::reposition().as_str()
                    || k == cmd_ids::unload_deliverable().as_str())
                    && p.target_system == frontier
            })
            .count();
        let mut bq = app.world_mut().query::<&BuildQueue>();
        let build_count = bq
            .iter(app.world())
            .map(|q| {
                q.queue
                    .iter()
                    .filter(|o| {
                        matches!(o.kind, BuildKind::Deliverable { .. })
                            && o.design_id == "infrastructure_core"
                    })
                    .count()
            })
            .sum::<usize>();
        outbox_count + pipeline_count + build_count
    };

    let baseline = count_chain_entries(&mut app);
    assert!(baseline > 0, "Setup precondition: baseline must be > 0");

    // Re-tick several times. The chain count is allowed to stay flat
    // or *decrease* (entries mature out of the outbox into the ship
    // pipeline) but must not grow — a growth would mean Rule 3.5
    // re-fired despite the in-flight commitment.
    let mut max_seen = baseline;
    for _ in 0..4 {
        advance_time(&mut app, 1);
        let now = count_chain_entries(&mut app);
        if now > max_seen {
            max_seen = now;
        }
    }
    assert!(
        max_seen <= baseline,
        "Dedup leak: chain entries grew across re-emits \
         (baseline={baseline}, max_seen={max_seen}). Rule 3.5 must skip the \
         frontier while the previous deploy is still in flight."
    );
}

#[test]
fn rule_3_and_rule_3_5_do_not_double_book_ship() {
    // Verify the courier partition: with both a colonisable system
    // (own-Core present, surveyed, not-colonised, in-region) AND a
    // frontier system available, plus a single idle colony ship, only
    // ONE rule may claim it per tick. Today's policy biases toward
    // Rule 3 (colonize) — Rule 3.5 stands down when the colonizable
    // demand consumes every idle courier.
    let mut app = test_app();
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

    let capital = spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let in_region = spawn_test_system(
        app.world_mut(),
        "Coreable",
        [0.3, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [1.5, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    spawn_test_ruler(app.world_mut(), empire, capital);
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(capital, SystemVisibilityTier::Local);
        vis.set(in_region, SystemVisibilityTier::Surveyed);
        vis.set(frontier, SystemVisibilityTier::Surveyed);
    }
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        for (sys, name, pos, colonised) in [
            (capital, "Capital", [0.0, 0.0, 0.0], true),
            (in_region, "Coreable", [0.3, 0.0, 0.0], false),
            (frontier, "Frontier", [1.5, 0.0, 0.0], false),
        ] {
            store.update(SystemKnowledge {
                system: sys,
                observed_at: 0,
                received_at: 0,
                data: SystemSnapshot {
                    name: name.into(),
                    position: pos,
                    surveyed: true,
                    colonized: colonised,
                    ..Default::default()
                },
                source: ObservationSource::Direct,
            });
        }
    }
    // Put a Core in BOTH capital and `in_region` so Rule 3 sees a
    // colonisable system without a Rule 3.5 detour.
    place_core_at(app.world_mut(), empire, capital, [0.0, 0.0, 0.0]);
    place_core_at(app.world_mut(), empire, in_region, [0.3, 0.0, 0.0]);

    // Colony at capital so `propagate_knowledge` writes `colonized=true`
    // — otherwise the snapshot says capital is uncolonised and Rule 3
    // picks capital instead of in_region.
    let cap_colony = common::spawn_test_colony(
        app.world_mut(),
        capital,
        macrocosmo_core::amount::Amt::units(100),
        macrocosmo_core::amount::Amt::units(100),
        vec![],
    );
    app.world_mut()
        .entity_mut(cap_colony)
        .insert(FactionOwner(empire));
    app.world_mut()
        .entity_mut(empire)
        .insert(macrocosmo::galaxy::HomeSystem(capital));

    let ship = spawn_test_ship(
        app.world_mut(),
        "Colonizer-1",
        "colony_ship_mk1",
        capital,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    // Region scope: capital + in_region. `frontier` is intentionally
    // outside so Rule 3.5 sees it as a frontier candidate.
    if app.world().get_resource::<RegionRegistry>().is_none() {
        app.world_mut().insert_resource(RegionRegistry::default());
    }
    let region = app
        .world_mut()
        .spawn(Region {
            empire,
            member_systems: vec![capital, in_region],
            capital_system: capital,
            mid_agent: None,
        })
        .id();
    app.world_mut()
        .entity_mut(capital)
        .insert(RegionMembership { region });
    app.world_mut()
        .entity_mut(in_region)
        .insert(RegionMembership { region });
    {
        let mut reg = app.world_mut().resource_mut::<RegionRegistry>();
        reg.by_empire.entry(empire).or_default().push(region);
    }
    let mid = app
        .world_mut()
        .spawn(MidAgent {
            region,
            state: macrocosmo_ai::MidTermState::default(),
            auto_managed: true,
        })
        .id();
    app.world_mut().get_mut::<Region>(region).unwrap().mid_agent = Some(mid);

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    // With Core at `in_region`, Rule 3 fires and emits `colonize_system`
    // (the existing `dispatch_table` route stamps it as a primitive).
    // Rule 3.5 must NOT also emit a `deploy_deliverable` for the
    // frontier the same tick — the single idle ship was consumed by
    // Rule 3.
    use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
    let colonize_marker_hits: usize = {
        let mut q = app.world_mut().query::<&PendingAssignment>();
        q.iter(app.world())
            .filter(|pa| {
                matches!(pa.kind, AssignmentKind::Colonize)
                    && matches!(pa.target, AssignmentTarget::System(t) if t == in_region)
            })
            .count()
    };
    assert!(
        colonize_marker_hits > 0,
        "Rule 3 must claim the idle ship for the colonisable in-region target."
    );
    assert!(
        !deploy_chain_in_flight_for(&mut app, frontier),
        "Rule 3.5 must NOT double-book the same ship: Rule 3 has it."
    );
}
