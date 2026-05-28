//! #470 regression: AI ship build orders land in a colony's `BuildQueue`.
//!
//! Pre-#470 the AI command consumer queried `Query<&mut BuildQueue>` keyed
//! by the StarSystem entity. `BuildQueue` is only ever attached to
//! **Colony** entities (see `setup::spawn_initial_colony`,
//! `colony::colonization::*`, `ship::settlement::*`), so every
//! `build_ship` / `fortify_system` / `build_deliverable` command from the
//! NPC AI silently dropped — `br.build_queues.get_mut(sys_entity)` always
//! returned `Err`. The visible symptom in #470 was "NPC empires never
//! build any ships after the starter fleet runs out".
//!
//! Post-fix the consumer walks colonies, picks the first one hosted in the
//! chosen shipyard system **and** owned (via `FactionOwner`) by the
//! issuing empire, and pushes the order onto that colony's `BuildQueue` —
//! mirroring the player UI in `ui/system_panel/mod.rs`.
//!
//! These four tests pin:
//!
//! 1. `build_ship_command_queues_order_on_host_colony` — happy path:
//!    `build_ship` lands on the host colony's `BuildQueue`.
//! 2. `build_ship_order_progresses_through_tick_build_queue` — the queued
//!    order is consumed by `tick_build_queue` (build_time decrements,
//!    minerals + energy are drawn from the system stockpile).
//! 3. `fortify_system_command_queues_combat_design_on_host_colony` —
//!    same path via `fortify_system` + the auto-pick combat-design branch.
//! 4. `build_ship_warns_when_shipyard_system_has_no_owned_colony` —
//!    shipyard present but no owner-matching colony (= conquered-system
//!    style fixture): the handler must drop gracefully (no order queued,
//!    no panic).

mod common;

use bevy::prelude::*;

use macrocosmo::ai::plugin::AiBusResource;
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::colony::building_queue::{BuildKind, BuildQueue};
use macrocosmo::colony::{Colony, ResourceStockpile};
use macrocosmo::components::Position;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::{HomeSystem, Planet, Sovereignty, StarSystem, SystemModifiers};
use macrocosmo::player::{Empire, Faction};
use macrocosmo::ship::Owner;
use macrocosmo_ai::{Command, CommandValue};
use macrocosmo_core::amount::Amt;

use common::{advance_time, spawn_mock_core_ship, spawn_test_ruler, test_app};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Build a single-system NPC empire with:
///   - one shipyard-capable system (via a `SystemModifiers.shipyard_build_parallel_slots`
///     entry — equivalent to having an active shipyard station for
///     `has_shipyard_check` purposes)
///   - one planet under that system
///   - one colony on that planet, owned by the empire, with an empty
///     `BuildQueue` ready to receive AI build orders
///   - the system's `Sovereignty.owner` set to the empire (so
///     `queue_ship_at_shipyard` accepts the system as an owned shipyard)
///   - a fully-stocked `ResourceStockpile` so `tick_build_queue` doesn't
///     starve the order on the first hexadie
///
/// Returns `(empire, system, planet, colony)`.
fn build_npc_with_shipyard_colony(app: &mut App) -> (Entity, Entity, Entity, Entity) {
    let world = app.world_mut();

    // Spawn the empire with the same bundle the production spawn path
    // (`spawn_test_empire` in `common::mod.rs`) installs on the
    // auto-spawned empire. `KnowledgeStore` + `SystemVisibilityMap` are
    // pulled in so the dispatcher's per-empire projection write
    // (`KnowledgeStore::update_projection`) finds its mutable store and
    // skips the `if let Ok(mut store) = ...` branch cleanly.
    let empire = world
        .spawn((
            Empire {
                name: "Test NPC".into(),
            },
            // PlayerEmpire here is a load-bearing sentinel: it suppresses
            // `mark_npc_empires_ai_controlled`, so `npc_decision_tick`
            // won't run the MidStanceAgent against this empire and emit
            // extra `build_ship` commands every tick (which would stack
            // onto the host colony's queue and break the strict
            // `len == 1` assertions in tests #1-3). Despite the "NPC"
            // naming in the fixture, this is the right marker for
            // "policy-driven AI is off, the test drives commands manually."
            macrocosmo::player::PlayerEmpire,
            Faction::new("test_npc", "Test NPC"),
            macrocosmo::knowledge::KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
            macrocosmo::empire::CommsParams::default(),
            macrocosmo::modifier::ScopedModifications::default(),
            macrocosmo::technology::TechTree::default(),
            macrocosmo::technology::ResearchQueue::default(),
        ))
        .id();

    // Shipyard-capacity modifier so `has_shipyard_check` returns true
    // without needing to spawn an actual station Ship + station design.
    let mut sys_mods = SystemModifiers::default();
    sys_mods
        .shipyard_build_parallel_slots
        .push_modifier(macrocosmo::modifier::Modifier {
            id: "fixture_shipyard".into(),
            label: "Fixture Shipyard".into(),
            base_add: macrocosmo_core::amount::SignedAmt::units(1),
            multiplier: macrocosmo_core::amount::SignedAmt::ZERO,
            add: macrocosmo_core::amount::SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        });

    let system = world
        .spawn((
            StarSystem {
                name: "Home".into(),
                is_capital: false,
                surveyed: true,
                star_type: "yellow_dwarf".into(),
            },
            Position::from([0.0, 0.0, 0.0]),
            Sovereignty {
                owner: Some(Owner::Empire(empire)),
                control_score: 1.0,
            },
            sys_mods,
            ResourceStockpile {
                minerals: Amt::units(10_000),
                energy: Amt::units(10_000),
                research: Amt::ZERO,
                food: Amt::units(1_000),
                authority: Amt::ZERO,
            },
        ))
        .id();

    let planet = world
        .spawn((Planet {
            name: "Home I".into(),
            system,
            planet_type: "terrestrial".into(),
        },))
        .id();

    let colony = world
        .spawn((
            Colony {
                planet,
                growth_rate: 0.0,
            },
            BuildQueue::default(),
            FactionOwner(empire),
        ))
        .id();

    // `update_sovereignty` runs every tick and overwrites
    // `Sovereignty.owner` based on which faction has a Core ship in the
    // system (`system_owner`). Without a Core ship at `system` our
    // hand-set `Sovereignty.owner = Some(Owner::Empire(empire))` is
    // reset to `None` on the first `app.update()` and
    // `queue_ship_at_shipyard`'s `owned_systems` filter becomes empty —
    // the build_ship command is then silently rejected because the
    // empire "owns no shipyard system." Stamping a mock Core ship here
    // keeps the sovereignty stable across ticks.
    spawn_mock_core_ship(app.world_mut(), system, empire);

    // The outbox `dispatch_ai_pending_commands` drops any command whose
    // issuing empire has no `Ruler` (= no position to compute light delay
    // from). Stationing a Ruler at the same system as the destination
    // makes the delay 0 hexadies so we can pin assertions on the very
    // next `app.update()`.
    spawn_test_ruler(app.world_mut(), empire, system);

    // build_ship / fortify_system / build_deliverable don't carry a
    // `target_system` param by default — the outbox falls back to the
    // empire's capital to compute the (Ruler→capital) light delay.
    // Without a `HomeSystem` component (or `is_capital` system) that
    // fallback returns None and the command is silently dropped before
    // it ever reaches the consumer's BuildQueue routing logic. Stamp
    // the home system here so the delay = 0 and the next `app.update()`
    // sees the order land on the colony.
    app.world_mut()
        .entity_mut(empire)
        .insert(HomeSystem(system));

    (empire, system, planet, colony)
}

/// Convert a Bevy `Entity` to the AI bus' `FactionId` keyed off the empire
/// entity index. Mirrors `crate::ai::convert::to_ai_faction(empire)` (not
/// `pub` outside the macrocosmo crate).
fn faction_id_for(empire: Entity) -> macrocosmo_ai::FactionId {
    macrocosmo_ai::FactionId(empire.index().index())
}

/// Insert the build_ship corvette design used by the tests directly into
/// the existing `ShipDesignRegistry` resource. `ShipDesignRegistry` is
/// not `Clone`, so we mutate in place instead of taking a snapshot.
///
/// `test_app()` already installs a registry that contains `explorer_mk1`
/// (survey design), `colony_ship_mk1` (colony design), `courier_mk1`
/// (no weapon but `can_survey == false && can_colonize == false`),
/// `scout_mk1`, plus the station designs. For deterministic
/// `build_ship` assertions we need a known pure combat design — we add
/// `patrol_corvette` here.
fn install_combat_corvette(app: &mut App) {
    use macrocosmo::ship_design::{ShipDesignDefinition, ShipDesignRegistry};
    let mut registry = app.world_mut().resource_mut::<ShipDesignRegistry>();
    registry.insert(ShipDesignDefinition {
        id: "patrol_corvette".into(),
        name: "Patrol Corvette".into(),
        description: String::new(),
        hull_id: "corvette".into(),
        modules: vec![],
        can_survey: false,
        can_colonize: false,
        maintenance: Amt::new(0, 500),
        build_cost_minerals: Amt::units(100),
        build_cost_energy: Amt::units(50),
        build_time: 30,
        hp: 100.0,
        sublight_speed: 0.1,
        ftl_range: 5.0,
        revision: 0,
        is_direct_buildable: true,
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Happy path: an NPC `build_ship` command lands on the host colony's
/// `BuildQueue` (NOT on the StarSystem). Pre-#470 the order was silently
/// dropped because the consumer queried `&mut BuildQueue` keyed by the
/// system entity, which never had a `BuildQueue` component attached.
#[test]
fn build_ship_command_queues_order_on_host_colony() {
    let mut app = test_app();
    install_combat_corvette(&mut app);
    let (empire, system, _planet, colony) = build_npc_with_shipyard_colony(&mut app);

    // Prime: run Startup so `schema::declare_all` registers `build_ship`
    // in the bus' `specs` table — without this, `emit_command` silently
    // drops the command (no warning surfaces in test output by default).
    app.update();

    // Emit the build_ship command directly onto the AI bus — bypasses the
    // upstream Mid/Short policy gating so the test pins consumer behaviour
    // independently of any unrelated decision-tree change.
    let cmd = Command::new(cmd_ids::build_ship(), faction_id_for(empire), 10)
        .with_param("design_id", CommandValue::Str("patrol_corvette".into()));
    app.world_mut()
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(cmd);

    // Tick the schedule a few times so the command flows through:
    //   dispatch (`Reason`) → outbox  →  process (`CommandDrain`) → drain.
    // `AiPlugin` orders dispatch + process + drain inside the same
    // `Update` (Reason → CommandDrain chain) so 1 tick suffices, but we
    // run three for slack against any future ordering shuffle.
    for _ in 0..3 {
        app.update();
    }

    // The system entity must NOT carry a BuildQueue — this is precisely
    // the wrong place the pre-fix code wrote to. Asserting absence here
    // documents the failure mode and locks us in to the correct contract.
    assert!(
        app.world().get::<BuildQueue>(system).is_none(),
        "StarSystem entity must NOT have a BuildQueue (it lives on Colony)",
    );

    // The colony entity must carry exactly one order matching our design.
    let queue = app
        .world()
        .get::<BuildQueue>(colony)
        .expect("Colony should still carry its BuildQueue after drain");
    assert_eq!(
        queue.queue.len(),
        1,
        "expected the build_ship command to enqueue exactly 1 order on the host colony; got {} entries",
        queue.queue.len(),
    );
    assert_eq!(queue.queue[0].design_id, "patrol_corvette");
    assert!(
        matches!(queue.queue[0].kind, BuildKind::Ship),
        "default build kind should be Ship, got {:?}",
        queue.queue[0].kind,
    );
}

/// The order queued via the AI bus must actually be consumed by the
/// game-side `tick_build_queue` system — confirming end-to-end wiring
/// rather than just the queue insertion. We advance time a few hexadies
/// and assert that `build_time_remaining` strictly decreases AND that
/// minerals/energy are drawn from the system stockpile.
#[test]
fn build_ship_order_progresses_through_tick_build_queue() {
    let mut app = test_app();
    install_combat_corvette(&mut app);
    let (empire, system, _planet, colony) = build_npc_with_shipyard_colony(&mut app);

    // Prime: see `build_ship_command_queues_order_on_host_colony` for
    // why this priming `update()` is required.
    app.update();

    let cmd = Command::new(cmd_ids::build_ship(), faction_id_for(empire), 10)
        .with_param("design_id", CommandValue::Str("patrol_corvette".into()));
    app.world_mut()
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(cmd);

    // Run a few ticks so the command flows through dispatcher → outbox
    // → processor → consumer (BuildQueue insertion).
    for _ in 0..3 {
        app.update();
    }

    // Snapshot pre-tick state for the colony queue and system stockpile.
    let (initial_build_time, initial_minerals, initial_energy) = {
        let bq = app.world().get::<BuildQueue>(colony).unwrap();
        let order = bq
            .queue
            .first()
            .expect("build_ship should be queued before advancing time");
        let stk = app.world().get::<ResourceStockpile>(system).unwrap();
        (order.build_time_remaining, stk.minerals, stk.energy)
    };
    assert!(
        initial_build_time > 0,
        "patrol_corvette must have a positive build_time, got {}",
        initial_build_time,
    );

    // Advance a handful of hexadies — enough for `tick_build_queue` to
    // chip away at build_time AND draw minerals/energy from the stockpile.
    let advance = 3i64;
    advance_time(&mut app, advance);

    let bq = app.world().get::<BuildQueue>(colony).unwrap();
    let order = bq
        .queue
        .first()
        .expect("build order should still be in progress after a few ticks");
    assert!(
        order.build_time_remaining < initial_build_time,
        "build_time_remaining must decrease after ticking; was {} now {}",
        initial_build_time,
        order.build_time_remaining,
    );

    let stk = app.world().get::<ResourceStockpile>(system).unwrap();
    // We expect strict consumption: order.minerals_cost > 0 + sufficient stockpile.
    assert!(
        stk.minerals < initial_minerals,
        "system minerals must be drawn down to fund the build order; was {} now {}",
        initial_minerals.to_f64(),
        stk.minerals.to_f64(),
    );
    assert!(
        stk.energy < initial_energy,
        "system energy must be drawn down to fund the build order; was {} now {}",
        initial_energy.to_f64(),
        stk.energy.to_f64(),
    );
}

/// `fortify_system` without a `design_id` param auto-picks the first
/// direct-buildable combat design (not survey / colony). The chosen
/// design must land on the host colony's `BuildQueue` — same routing
/// fix as `build_ship`.
#[test]
fn fortify_system_command_queues_combat_design_on_host_colony() {
    let mut app = test_app();
    install_combat_corvette(&mut app);
    let (empire, _system, _planet, colony) = build_npc_with_shipyard_colony(&mut app);

    // Prime: see `build_ship_command_queues_order_on_host_colony` for
    // why this priming `update()` is required.
    app.update();

    // No design_id param → consumer auto-picks the first direct-buildable,
    // !can_survey, !can_colonize design from the registry.
    let cmd = Command::new(cmd_ids::fortify_system(), faction_id_for(empire), 10);
    app.world_mut()
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(cmd);

    // Tick a few times so the command reaches the consumer.
    for _ in 0..3 {
        app.update();
    }

    let queue = app
        .world()
        .get::<BuildQueue>(colony)
        .expect("Colony should still carry its BuildQueue after drain");
    assert_eq!(
        queue.queue.len(),
        1,
        "fortify_system should auto-pick a combat design and queue exactly 1 order; got {} entries",
        queue.queue.len(),
    );
    // The consumer's auto-pick walks `registry.designs.values()` (a
    // `HashMap`) and picks the first `is_direct_buildable && !can_survey
    // && !can_colonize` entry. Iteration order is non-deterministic, so
    // we accept any design that satisfies those three predicates rather
    // than pinning a specific id.
    use macrocosmo::ship_design::ShipDesignRegistry;
    let registry = app.world().resource::<ShipDesignRegistry>();
    let chosen = registry
        .get(&queue.queue[0].design_id)
        .expect("the design picked by fortify_system must exist in the registry");
    assert!(
        chosen.is_direct_buildable && !chosen.can_survey && !chosen.can_colonize,
        "fortify_system must pick a direct-buildable, non-survey, non-colony design; picked {:?} (survey={}, colony={}, direct_buildable={})",
        chosen.id,
        chosen.can_survey,
        chosen.can_colonize,
        chosen.is_direct_buildable,
    );
}

/// Conquered-system style fixture: the shipyard system is "owned" by the
/// empire from Sovereignty's POV, but the colony hosted there has a
/// different `FactionOwner` (= the original colonizer). The consumer
/// must refuse to queue an order on the wrong-owner colony, leaving the
/// queue empty rather than panicking or silently dropping into the wrong
/// faction's production.
///
/// `empire_a` (the issuer) is given the same full component bundle as
/// `build_npc_with_shipyard_colony` (including `PlayerEmpire` to
/// suppress the policy-driven `mark_npc_empires_ai_controlled` path) so
/// the `queue.is_empty()` assertion at the end exercises the
/// owner-mismatch reject branch rather than false-passing on an
/// upstream dispatcher drop (missing `KnowledgeStore`, missing
/// `TechTree`, etc.). `empire_b` issues no command in this test, so it
/// stays a minimal `Empire + Faction` witness.
#[test]
fn build_ship_warns_when_shipyard_system_has_no_owned_colony() {
    let mut app = test_app();
    install_combat_corvette(&mut app);

    // Empire A is the issuer (= conqueror that took the system). Full
    // bundle so dispatch / drain doesn't early-drop the command before
    // it ever reaches `pick_host_colony`.
    let empire_a = app
        .world_mut()
        .spawn((
            Empire {
                name: "Issuer".into(),
            },
            // See `build_npc_with_shipyard_colony` for why this marker
            // is load-bearing — suppresses policy-driven build_ship
            // emissions that would race with our manual emit.
            macrocosmo::player::PlayerEmpire,
            Faction::new("issuer", "Issuer"),
            macrocosmo::knowledge::KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
            macrocosmo::empire::CommsParams::default(),
            macrocosmo::modifier::ScopedModifications::default(),
            macrocosmo::technology::TechTree::default(),
            macrocosmo::technology::ResearchQueue::default(),
        ))
        .id();
    // Empire B is the original owner of the colony — no-op witness for
    // this test (issues no commands, has no Ruler / HomeSystem / policy
    // active), so the minimal `Empire + Faction` shape is sufficient.
    let empire_b = app
        .world_mut()
        .spawn((
            Empire {
                name: "Original".into(),
            },
            Faction::new("original", "Original"),
        ))
        .id();

    // Build the shipyard fixture but flip the colony's `FactionOwner` to
    // `empire_b` so it does not match the issuer.
    let mut sys_mods = SystemModifiers::default();
    sys_mods
        .shipyard_build_parallel_slots
        .push_modifier(macrocosmo::modifier::Modifier {
            id: "conquered_shipyard".into(),
            label: "Conquered Shipyard".into(),
            base_add: macrocosmo_core::amount::SignedAmt::units(1),
            multiplier: macrocosmo_core::amount::SignedAmt::ZERO,
            add: macrocosmo_core::amount::SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        });
    let system = app
        .world_mut()
        .spawn((
            StarSystem {
                name: "Conquered".into(),
                is_capital: false,
                surveyed: true,
                star_type: "yellow_dwarf".into(),
            },
            Position::from([0.0, 0.0, 0.0]),
            // Sovereignty says empire_a (= conqueror) owns the system.
            Sovereignty {
                owner: Some(Owner::Empire(empire_a)),
                control_score: 1.0,
            },
            sys_mods,
        ))
        .id();
    let planet = app
        .world_mut()
        .spawn((Planet {
            name: "Conquered I".into(),
            system,
            planet_type: "terrestrial".into(),
        },))
        .id();
    let colony = app
        .world_mut()
        .spawn((
            Colony {
                planet,
                growth_rate: 0.0,
            },
            BuildQueue::default(),
            // The colony itself still belongs to empire_b (the colonizer
            // hasn't been displaced yet — only the system was conquered).
            FactionOwner(empire_b),
        ))
        .id();

    // Both empires need a Ruler so the outbox can compute light-delay,
    // and the issuer needs a `HomeSystem` so the build-command capital
    // fallback resolves (see `build_npc_with_shipyard_colony` for the
    // same setup pattern). We only need the issuer (empire_a) to have a
    // valid home — empire_b never issues a command in this test. A Core
    // ship at the system keeps empire_a's sovereignty stable across the
    // `update_sovereignty` tick (otherwise the `owned_systems` filter
    // empties and the early "no shipyard" reject preempts the
    // owner-mismatch reject we want to pin).
    spawn_mock_core_ship(app.world_mut(), system, empire_a);
    spawn_test_ruler(app.world_mut(), empire_a, system);
    app.world_mut()
        .entity_mut(empire_a)
        .insert(HomeSystem(system));

    // Prime: see `build_ship_command_queues_order_on_host_colony`.
    app.update();

    // Emit the build_ship command as empire_a.
    let cmd = Command::new(cmd_ids::build_ship(), faction_id_for(empire_a), 10)
        .with_param("design_id", CommandValue::Str("patrol_corvette".into()));
    app.world_mut()
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(cmd);

    // Run the schedule a few times to let the (failing) command flow
    // through dispatch → outbox → processor → consumer. Must not panic.
    for _ in 0..3 {
        app.update();
    }

    // The colony's BuildQueue must remain empty — we refused to push the
    // order onto a colony owned by a different empire.
    let queue = app
        .world()
        .get::<BuildQueue>(colony)
        .expect("Colony should still carry its (empty) BuildQueue");
    assert!(
        queue.queue.is_empty(),
        "build_ship from empire_a must NOT land on empire_b's colony; got {} entries",
        queue.queue.len(),
    );
}

/// #470 fold-in: `build_ship` must dedup same-design emissions per colony.
/// Without this, Rule 6 (`build_ship` re-fired every Reason tick while
/// `combat_count < 3`) would stack 30+ duplicate `patrol_corvette` orders
/// on the host colony before the first ship completes. The dedup mirrors
/// the existing `build_deliverable` dedup in
/// `command_consumer.rs::handle_build_deliverable`.
#[test]
fn build_ship_dedups_same_design_emissions_per_colony() {
    let mut app = test_app();
    install_combat_corvette(&mut app);
    let (empire, _system, _planet, colony) = build_npc_with_shipyard_colony(&mut app);

    // Prime + emit the same command twice in a row.
    app.update();
    for _ in 0..2 {
        let cmd = Command::new(cmd_ids::build_ship(), faction_id_for(empire), 10)
            .with_param("design_id", CommandValue::Str("patrol_corvette".into()));
        app.world_mut()
            .resource_mut::<AiBusResource>()
            .0
            .emit_command(cmd);
        // Run a few ticks between emits so the first one lands before
        // the second one races through dispatch.
        for _ in 0..3 {
            app.update();
        }
    }

    let queue = app.world().get::<BuildQueue>(colony).unwrap();
    assert_eq!(
        queue.queue.len(),
        1,
        "expected dedup to collapse duplicate `build_ship` for the same design; got {} orders: {:?}",
        queue.queue.len(),
        queue.queue.iter().map(|o| &o.design_id).collect::<Vec<_>>(),
    );
    assert_eq!(queue.queue[0].design_id, "patrol_corvette");
}
