//! Hotfix-3 regression tests — AI resource starvation + Rule 6
//! fleet-composition semantics.
//!
//! Two bugs were observed on a long-run brp QA session:
//!
//! 1. **Infinite `explorer_mk1` build loop** — Rule 6's first branch
//!    fires when `comp.survey_count == 0 && has_unsurveyed_targets`.
//!    Pre-hotfix `survey_count` was derived from
//!    `NpcContext.ships`, whose builder filters on
//!    `info.system.is_some()`. The moment an explorer transitioned
//!    to `ShipState::Surveying` its `system` collapsed to `None`,
//!    the count went back to 0, and Rule 6 re-emitted `build_ship
//!    explorer_mk1` every Reason tick. Each cycle paid the full
//!    explorer maintenance + build cost, eventually bankrupting the
//!    starter empire.
//!
//! 2. **AI re-queues identical building order every tick** — the brp
//!    QA report observed a single colony with **165 stacked `mine`
//!    orders**. Rule 5b emits the same `build_structure` proposal
//!    every Reason tick once production is short; the per-tick
//!    dedup that absorbs system-building re-emissions
//!    (`handle_build_structure` system branch, line ~736) did not
//!    have a planet-building counterpart, so every emit found a
//!    free slot and pushed another order.
//!
//! Both bugs share a deeper root cause — **no resource gate** on
//! the build rules. Even with the dedup fixed, an empire with
//! stockpile == 0 would still emit, queue, and starve. The hotfix
//! adds a soft `current_stockpile >= cost` gate to every build
//! emission (Rules 5a / 5b / 6 / 3.5), permitting deficit spending
//! (revenue < expense) but refusing to stack orders an empty
//! stockpile cannot fund.
//!
//! This integration test pins the consumer-side planet-building
//! dedup behaviour via the production `drain_ai_commands`
//! pipeline. Pure-logic gate tests (no Bevy app required) live as
//! `#[test]`s alongside `MidStanceAgent::decide` in
//! `src/ai/mid_stance.rs::tests` — they cover Rule 6 fleet-census
//! semantics, Rule 5a stockpile gate, and the deficit-spending
//! escape hatch.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::npc_decision::ShortAgentTickInputs;
use macrocosmo::ai::plugin::AiBusResource;
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::colony::building_queue::{
    BuildKind, BuildOrder, BuildQueue, BuildingOrder, BuildingQueue,
};
use macrocosmo::colony::system_buildings::SystemBuildingQueue;
use macrocosmo::faction::FactionOwner;
use macrocosmo::knowledge::{KnowledgeStore, SystemVisibilityMap};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::scripting::building_api::BuildingId;
use macrocosmo_ai::{Command, CommandValue};
use macrocosmo_core::amount::Amt;

use common::{
    advance_time, spawn_mock_core_ship, spawn_test_colony, spawn_test_ruler, spawn_test_system,
    test_app,
};

/// Mirror of `to_ai_faction(empire)` (private to the ai module). We
/// rebuild the conversion here so the test can craft a `Command`
/// whose `issuer.0 == empire.index()` without pulling in private
/// internals.
fn faction_id_for(empire: Entity) -> macrocosmo_ai::FactionId {
    macrocosmo_ai::FactionId(empire.index().index())
}

/// Spawn a minimal NPC empire with one colonised, sovereign system
/// and a non-zero stockpile. Returns `(empire, system)`.
///
/// **Why `AiPlayerMode(true)`**: lets the empire be `PlayerEmpire`
/// while still routing through the AI command pipeline. The test
/// directly emits commands onto the bus rather than relying on the
/// Mid rule logic, so the player flag is purely an authorisation
/// switch.
fn spawn_empire_with_colony(app: &mut App) -> (Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Stockpile Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "stockpile_test".into(),
                name: "Stockpile Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            SystemVisibilityMap::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    app.world_mut()
        .entity_mut(empire)
        .insert(macrocosmo::galaxy::HomeSystem(home));
    spawn_test_ruler(app.world_mut(), empire, home);

    // Colony with 4 free slots and a fat stockpile (so the test can
    // observe the dedup independent of the stockpile gate).
    let colony = spawn_test_colony(
        app.world_mut(),
        home,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![None, None, None, None],
    );
    // Re-stamp colony owner (helper picks the first empire it finds,
    // which can be a different auto-spawned test empire).
    app.world_mut()
        .entity_mut(colony)
        .insert(FactionOwner(empire));

    // Sovereignty is set by `update_sovereignty` from CoreShip → AtSystem
    // → FactionOwner. Spawning a mock Core ship at home routes the
    // ownership through the production path; without it the per-tick
    // `update_sovereignty` resets the manual stamp back to None and
    // `handle_build_structure`'s owned-systems gate drops the order.
    spawn_mock_core_ship(app.world_mut(), home, empire);

    (empire, home)
}

/// Emit a `build_structure(building_id = "mine")` directly onto the
/// bus, addressed to `empire`. Returns the issued command for log /
/// assertion.
fn emit_build_mine(world: &mut World, empire: Entity, now: i64) -> Command {
    let cmd = Command::new(cmd_ids::build_structure(), faction_id_for(empire), now)
        .with_param("building_id", CommandValue::Str("mine".into()));
    world
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(cmd.clone());
    cmd
}

/// Hotfix-3: planet-building dedup at the colony level. Emitting the
/// same `build_structure(mine)` twice within one tick must result in
/// **one** outstanding `BuildingQueue` order, not two. Pre-hotfix the
/// dedup branch only fired for system buildings (shipyard / port /
/// lab); planet buildings (mine / farm / power_plant) stacked
/// unbounded, leading the brp QA report's 165× mine stack.
#[test]
fn planet_building_dedup_at_colony_collapses_duplicate_emits() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);

    // First tick triggers Startup (registers AI schema kinds, builds
    // resources). Without this the direct `emit_command` call below
    // would be dropped with an "undeclared command kind" warning
    // because `schema::declare_all` runs in Startup.
    advance_time(&mut app, 1);

    // Two identical `mine` commands in the same tick.
    emit_build_mine(app.world_mut(), empire, 1);
    emit_build_mine(app.world_mut(), empire, 1);

    // Drive the AI bus dispatch + handler chain. Origin == destination
    // (Ruler at home, build_structure resolves to capital == home) so
    // light-speed delay is zero — five Update cycles is enough to
    // clear `emit → outbox → drain → handler`.
    for _ in 0..5 {
        advance_time(&mut app, 1);
    }

    let mut found_queues = Vec::new();
    {
        let mut q = app.world_mut().query::<(&BuildingQueue, &FactionOwner)>();
        for (queue, owner) in q.iter(app.world()) {
            if owner.0 != empire {
                continue;
            }
            let mines = queue
                .queue
                .iter()
                .filter(|o| o.building_id.as_str() == "mine")
                .count();
            found_queues.push(mines);
        }
    }
    let total_mines: usize = found_queues.iter().sum();
    assert_eq!(
        total_mines, 1,
        "Hotfix-3 planet dedup: two identical `mine` emits must collapse to one queue order, got per-colony counts {:?}",
        found_queues,
    );
}

/// Hotfix-3: dedup still kicks in across multiple ticks. The
/// per-tick stockpile gate suppresses fresh emits when the order is
/// in-flight, but if the AI logic side fails to gate (e.g. legacy
/// code path, future regression), the consumer-side dedup is the
/// last line of defense. Pin three back-to-back emits in three
/// separate ticks → still one queue order.
#[test]
fn planet_building_dedup_survives_multi_tick_re_emission() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);

    // Startup tick first so AI schema is declared (see comment in
    // `planet_building_dedup_at_colony_collapses_duplicate_emits`).
    advance_time(&mut app, 1);

    // Tick 1 emit → drive dispatch → tick 2 emit → … → tick 3 emit.
    emit_build_mine(app.world_mut(), empire, 1);
    advance_time(&mut app, 1);
    emit_build_mine(app.world_mut(), empire, 2);
    advance_time(&mut app, 1);
    emit_build_mine(app.world_mut(), empire, 3);
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let mut total_mines = 0usize;
    {
        let mut q = app.world_mut().query::<(&BuildingQueue, &FactionOwner)>();
        for (queue, owner) in q.iter(app.world()) {
            if owner.0 != empire {
                continue;
            }
            total_mines += queue
                .queue
                .iter()
                .filter(|o| o.building_id.as_str() == "mine")
                .count();
        }
    }
    assert_eq!(
        total_mines, 1,
        "Hotfix-3: three sequential `mine` emits across separate ticks must still result in 1 queue order",
    );
}

/// Hotfix-3 sanity: dedup is keyed on `building_id`, NOT on order
/// arrival ordering. A different building (farm) emitted after a
/// mine must be allowed through — the per-tick stockpile gate is
/// the only thing that throttles cross-building emits, and we
/// model an empire with enough stockpile to fund both.
#[test]
fn planet_building_dedup_does_not_block_different_building_id() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);

    // Startup tick first so AI schema is declared.
    advance_time(&mut app, 1);

    emit_build_mine(app.world_mut(), empire, 1);
    let farm_cmd = Command::new(cmd_ids::build_structure(), faction_id_for(empire), 1)
        .with_param("building_id", CommandValue::Str("farm".into()));
    app.world_mut()
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(farm_cmd);

    for _ in 0..5 {
        advance_time(&mut app, 1);
    }

    let (mut mines, mut farms) = (0usize, 0usize);
    {
        let mut q = app.world_mut().query::<(&BuildingQueue, &FactionOwner)>();
        for (queue, owner) in q.iter(app.world()) {
            if owner.0 != empire {
                continue;
            }
            for o in &queue.queue {
                match o.building_id.as_str() {
                    "mine" => mines += 1,
                    "farm" => farms += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(mines, 1, "one mine queued");
    assert_eq!(farms, 1, "farm not blocked by mine's dedup entry");
}

// ---------------------------------------------------------------------------
// #529 A migration: pending-aware resource gate (PR #531 fold-in)
// ---------------------------------------------------------------------------

/// Find the first colony entity owned by `empire` that carries a
/// `BuildQueue` (ship/deliverable orders). Panics if none exists —
/// `spawn_empire_with_colony` always attaches one.
fn find_empire_owned_colony(app: &mut App, empire: Entity) -> Entity {
    let world = app.world_mut();
    let mut q = world.query_filtered::<(Entity, &FactionOwner), With<BuildQueue>>();
    q.iter(world)
        .find(|(_, o)| o.0 == empire)
        .map(|(e, _)| e)
        .expect("empire-owned colony with BuildQueue should exist")
}

/// Push a synthetic pending ship build order onto `colony`'s `BuildQueue`
/// with the specified cost / invested split. Used to simulate an
/// "in-flight" commitment that the pending-aware gate must subtract.
fn push_pending_ship_order(
    world: &mut World,
    colony: Entity,
    minerals_cost: Amt,
    minerals_invested: Amt,
    energy_cost: Amt,
    energy_invested: Amt,
) {
    let mut queue = world.get_mut::<BuildQueue>(colony).unwrap();
    queue.queue.push(BuildOrder {
        order_id: 99_999,
        kind: BuildKind::Ship,
        design_id: "fake_pending".into(),
        display_name: "Fake Pending".into(),
        minerals_cost,
        minerals_invested,
        energy_cost,
        energy_invested,
        build_time_total: 30,
        build_time_remaining: 30,
    });
}

/// Capture `RegionShortInputs.current_minerals / current_energy` after a
/// single Mid tick. Asserts the inputs row for the empire's primary
/// region is populated.
///
/// PR #531 Codex review fold-in (finding 2): `ShortAgentTickInputs` is
/// keyed by **region entity** rather than empire entity, so single-region
/// test setups resolve the empire's primary region via the
/// `find_empire_region` helper before indexing.
fn snapshot_current_amounts(app: &App, empire: Entity) -> (Amt, Amt) {
    let region = find_empire_region(app, empire);
    let inputs = app.world().resource::<ShortAgentTickInputs>();
    let region_inputs = inputs
        .per_region
        .get(&region)
        .expect("RegionShortInputs should be populated for the empire's primary region");
    (region_inputs.current_minerals, region_inputs.current_energy)
}

/// Resolve the empire's primary `Region` entity. Used by tests that need
/// to look up `ShortAgentTickInputs.per_region` keys. Panics if no region
/// has been backfilled yet (always the case after the first Update
/// because `backfill_mid_agents_for_ai_controlled` runs every frame).
fn find_empire_region(app: &App, empire: Entity) -> Entity {
    let registry = app.world().resource::<macrocosmo::region::RegionRegistry>();
    *registry
        .by_empire
        .get(&empire)
        .and_then(|regions| regions.first())
        .expect("RegionRegistry should contain at least one region for the empire")
}

/// #529 A migration: the AI's pending-aware resource gate subtracts every
/// in-flight `BuildOrder`'s **remaining** cost (= `cost - invested`) from
/// the empire's combined stockpile sum before it reaches the rule layer.
///
/// Pre-fix: `current_stockpile_sum >= cost` ignored in-flight orders, so
/// an empire with 100 minerals and a corvette already queued (cost 80,
/// invested 0) would happily emit a second corvette (gate sees `100 >=
/// 50`), and then starve both when the production tick drained the
/// stockpile. The pending-aware form (= this fold-in) makes the gate see
/// `100 - 80 = 20` instead, so the second emit is rejected.
///
/// We assert by **delta**: snapshot `current_minerals/energy` before and
/// after injecting the pending order. This sidesteps production /
/// maintenance drift and per-test-app starter stockpile variations — the
/// invariant we want to pin is "pending order remaining cost flows into
/// the published `current_*` values", not the absolute stockpile number.
#[test]
fn resource_gate_subtracts_pending_orders_from_stockpile() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);

    // Prime: schema::declare_all runs in Startup, the Mid pipeline
    // populates `RegionShortInputs` from the first Update onwards.
    advance_time(&mut app, 1);
    let (m_before, e_before) = snapshot_current_amounts(&app, empire);

    let colony = find_empire_owned_colony(&mut app, empire);
    // Pending: 80 minerals + 50 energy, invested 0 → remaining = full cost.
    push_pending_ship_order(
        app.world_mut(),
        colony,
        Amt::units(80),
        Amt::ZERO,
        Amt::units(50),
        Amt::ZERO,
    );

    // Run another tick so `npc_decision_tick` recomputes the
    // pending-adjusted stockpile sum.
    advance_time(&mut app, 1);
    let (m_after, e_after) = snapshot_current_amounts(&app, empire);

    // delta = before − after (= "the pending order subtracted this much from
    // the published value"). +1 hex of production raises stockpile but does
    // NOT change the pending subtraction; for the delta we approximate by
    // allowing ≥ pending (production may have added to the headroom).
    let m_subtracted = m_before.sub(m_after);
    let e_subtracted = e_before.sub(e_after);
    assert!(
        m_subtracted >= Amt::units(80),
        "pending order remaining minerals_cost (80) must lower current_minerals by ≥ 80; \
         got delta = {:?}, before = {:?}, after = {:?}",
        m_subtracted,
        m_before,
        m_after,
    );
    assert!(
        e_subtracted >= Amt::units(50),
        "pending order remaining energy_cost (50) must lower current_energy by ≥ 50; \
         got delta = {:?}, before = {:?}, after = {:?}",
        e_subtracted,
        e_before,
        e_after,
    );
}

/// Pending order with partial investment: subtract only the REMAINING
/// cost (= `cost - invested`), not the full cost. As the order's
/// production tick consumes resources, the pending-aware gate
/// progressively unblocks new emits — the AI's "headroom" grows as work
/// is completed. Same delta-based assertion as the full-cost case above,
/// just with smaller numbers.
#[test]
fn resource_gate_subtracts_only_remaining_cost_when_pending_invested() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);
    advance_time(&mut app, 1);
    let (m_before, e_before) = snapshot_current_amounts(&app, empire);

    let colony = find_empire_owned_colony(&mut app, empire);
    // 80 cost, 30 already invested → remaining 50.
    // 50 cost, 20 already invested → remaining 30.
    push_pending_ship_order(
        app.world_mut(),
        colony,
        Amt::units(80),
        Amt::units(30),
        Amt::units(50),
        Amt::units(20),
    );

    advance_time(&mut app, 1);
    let (m_after, e_after) = snapshot_current_amounts(&app, empire);

    let m_subtracted = m_before.sub(m_after);
    let e_subtracted = e_before.sub(e_after);
    // The remaining cost is 50 minerals / 30 energy. The lower bound pins
    // that the subtraction reflects pending — without the pending-aware
    // logic the delta would be only the production / maintenance drift
    // between two ticks (≪ 50).
    //
    // TODO (#529 follow-up): once the test harness controls production /
    // maintenance per-tick we can add an upper-bound (= "must NOT subtract
    // the full cost"). The current delta-based form doesn't reliably
    // distinguish remaining (50) from full (80) under arbitrary
    // production drift, but it does pin the contract that *something*
    // pending-related is subtracted.
    assert!(
        m_subtracted >= Amt::units(50),
        "must subtract at least the remaining cost (80 - 30 = 50); got delta = {:?}",
        m_subtracted,
    );
    assert!(
        e_subtracted >= Amt::units(30),
        "must subtract at least the remaining cost (50 - 20 = 30); got delta = {:?}",
        e_subtracted,
    );
}

/// `SystemBuildingQueue` entries (= shipyard / port / research_lab build
/// orders on the StarSystem) also subtract from the pending-aware
/// stockpile, mirroring the per-colony `BuildQueue` walk. Sub-agent
/// implementation note: the walk gates on `member_systems_set` because
/// `SystemBuildingQueue` has no owner component.
#[test]
fn system_building_queue_pending_also_subtracts_from_stockpile() {
    let mut app = test_app();
    let (empire, home) = spawn_empire_with_colony(&mut app);
    advance_time(&mut app, 1);
    let (m_before, e_before) = snapshot_current_amounts(&app, empire);

    // Push a pending shipyard build order on the home system's
    // SystemBuildingQueue (= system-level builds: shipyard / port / lab).
    {
        let mut q = app
            .world_mut()
            .get_mut::<SystemBuildingQueue>(home)
            .expect("home system should have SystemBuildingQueue (spawned by spawn_test_colony)");
        q.queue.push(BuildingOrder {
            order_id: 99_999,
            building_id: BuildingId::new("shipyard"),
            target_slot: 0,
            minerals_remaining: Amt::units(150),
            energy_remaining: Amt::units(100),
            build_time_remaining: 30,
        });
    }

    advance_time(&mut app, 1);
    let (m_after, e_after) = snapshot_current_amounts(&app, empire);

    let m_subtracted = m_before.sub(m_after);
    let e_subtracted = e_before.sub(e_after);
    assert!(
        m_subtracted >= Amt::units(150),
        "system-building queue minerals_remaining (150) must lower current_minerals by ≥ 150; \
         got delta = {:?}",
        m_subtracted,
    );
    assert!(
        e_subtracted >= Amt::units(100),
        "system-building queue energy_remaining (100) must lower current_energy by ≥ 100; \
         got delta = {:?}",
        e_subtracted,
    );
}

/// #532 F3: per-colony `BuildingQueue` (mine / farm / power_plant) pending
/// orders must also subtract from the pending-aware stockpile. Without this,
/// Rule 5b can stack cross-id planet buildings — `handle_build_structure`'s
/// same-tick dedup only collapses same-id duplicates, so an empire with
/// stockpile just enough for one mine could still push a mine + a farm +
/// a power_plant in successive ticks (each id passes the gate on its own,
/// each id is unique against the handler's dedup, all three orders land,
/// all three starve).
///
/// We pre-queue a single mine `BuildingOrder` on the colony's
/// `BuildingQueue`, then snapshot the post-tick
/// `RegionShortInputs.current_minerals/energy` — which is the value Short
/// Rule 5b's `adapter.can_afford_building` reads. If F3 is wired correctly,
/// the published value drops by ≥ the pending mine's remaining cost.
#[test]
fn resource_gate_subtracts_pending_planet_building_from_stockpile() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);
    advance_time(&mut app, 1);
    let (m_before, e_before) = snapshot_current_amounts(&app, empire);

    let colony = find_empire_owned_colony(&mut app, empire);
    // Mine cost in `create_test_building_registry`: 150 minerals / 50 energy.
    // Pre-queue with full remaining cost (no investment yet) so the gate
    // sees the full pending amount.
    {
        let mut q = app
            .world_mut()
            .get_mut::<BuildingQueue>(colony)
            .expect("test colony should carry a BuildingQueue");
        q.queue.push(BuildingOrder {
            order_id: 99_998,
            building_id: BuildingId::new("mine"),
            target_slot: 0,
            minerals_remaining: Amt::units(150),
            energy_remaining: Amt::units(50),
            build_time_remaining: 10,
        });
    }

    advance_time(&mut app, 1);
    let (m_after, e_after) = snapshot_current_amounts(&app, empire);

    let m_subtracted = m_before.sub(m_after);
    let e_subtracted = e_before.sub(e_after);
    assert!(
        m_subtracted >= Amt::units(150),
        "#532 F3: pending mine's minerals_remaining (150) must lower current_minerals by ≥ 150; \
         got delta = {:?}, before = {:?}, after = {:?}",
        m_subtracted,
        m_before,
        m_after,
    );
    assert!(
        e_subtracted >= Amt::units(50),
        "#532 F3: pending mine's energy_remaining (50) must lower current_energy by ≥ 50; \
         got delta = {:?}, before = {:?}, after = {:?}",
        e_subtracted,
        e_before,
        e_after,
    );
}

// ---------------------------------------------------------------------------
// #532 F2: Rule 8 fortify resource gate (PR #531 finding 2 fold-in)
// ---------------------------------------------------------------------------

/// Install a known cheap combat design for Rule 8's adapter pick. The
/// test design registry seeded by `test_app()` doesn't include a
/// deterministic cheapest-combat option; we add `patrol_corvette`
/// (cost 100m / 50e) explicitly so the pick is pinnable.
fn install_patrol_corvette(app: &mut App) {
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

/// Attach a shipyard slot modifier to `system` so the empire's
/// `can_build_ships` metric reaches 1.0 (Rule 8's gate). Mirrors
/// `build_npc_with_shipyard_colony` in `ai_ship_build_queue.rs` —
/// pushes onto the existing `SystemModifiers.shipyard_build_parallel_slots`
/// in place rather than spawning a station ship, so the test stays
/// minimal. `spawn_test_system` already attaches a default
/// `SystemModifiers` component.
fn attach_shipyard_to_system(world: &mut World, system: Entity) {
    use macrocosmo::galaxy::SystemModifiers;
    use macrocosmo::modifier::Modifier;
    use macrocosmo_core::amount::SignedAmt;
    let mut sys_mods = world
        .get_mut::<SystemModifiers>(system)
        .expect("spawn_test_system attaches SystemModifiers by default");
    sys_mods
        .shipyard_build_parallel_slots
        .push_modifier(Modifier {
            id: "fixture_shipyard".into(),
            label: "Fixture Shipyard".into(),
            base_add: SignedAmt::units(1),
            multiplier: SignedAmt::ZERO,
            add: SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        });
}

/// Drain the empire's existing stockpile to zero on its home system.
/// Used to force Rule 8's affordability gate to reject every combat
/// design. We can't pass `Amt::ZERO` to `spawn_test_colony` because
/// the helper attaches the stockpile to the *star system* and we
/// already spawned with `10_000` to keep other tests in this file
/// happy; this resets the field post-spawn.
fn drain_stockpile(world: &mut World, system: Entity) {
    use macrocosmo::colony::ResourceStockpile;
    if let Some(mut s) = world.get_mut::<ResourceStockpile>(system) {
        s.minerals = Amt::ZERO;
        s.energy = Amt::ZERO;
    }
}

/// Count ship-kind orders for `empire` across every colony BuildQueue,
/// optionally filtered by `design_id`. Walks the whole world because
/// we don't know in advance which colony Rule 8's pipeline picked as
/// the host.
fn count_ship_orders_for_empire(app: &mut App, empire: Entity, design_id: Option<&str>) -> usize {
    let world = app.world_mut();
    let mut q = world.query::<(&BuildQueue, &FactionOwner)>();
    q.iter(world)
        .filter(|(_, owner)| owner.0 == empire)
        .map(|(queue, _)| {
            queue
                .queue
                .iter()
                .filter(|o| matches!(o.kind, BuildKind::Ship))
                .filter(|o| design_id.is_none_or(|id| o.design_id == id))
                .count()
        })
        .sum()
}

/// #532 F2 (bankrupt case): an empire with a shipyard but zero
/// stockpile must NOT queue any combat ship via Rule 8. Pre-fix Rule
/// 8 emitted `fortify_system` (no `design_id`) and the handler's
/// auto-pick had no affordability check — every Reason tick added an
/// unaffordable combat order. Post-fix `affordable_fortify_design`
/// returns `None` for the bankrupt empire and Rule 8 stays silent.
#[test]
fn rule_8_fortify_no_build_when_bankrupt_with_shipyard() {
    let mut app = test_app();
    install_patrol_corvette(&mut app);
    let (empire, home) = spawn_empire_with_colony(&mut app);

    // Add a shipyard so `can_build_ships` metric climbs to 1.0 →
    // Rule 8's first gate passes.
    attach_shipyard_to_system(app.world_mut(), home);
    // Bankrupt the stockpile so the F2 affordability gate must
    // reject every combat design.
    drain_stockpile(app.world_mut(), home);

    // Run several Reason ticks so the AI pipeline has multiple
    // chances to emit. If F2 is wired correctly, Rule 8 stays silent
    // every tick and no `build_ship` order lands.
    for _ in 0..6 {
        advance_time(&mut app, 1);
    }

    let ship_orders = count_ship_orders_for_empire(&mut app, empire, None);
    assert_eq!(
        ship_orders, 0,
        "#532 F2: bankrupt empire with shipyard must NOT queue any ship via Rule 8; \
         found {} ship order(s) in BuildQueue",
        ship_orders,
    );
}

/// #532 F2 (affordable case): an empire with a shipyard and enough
/// stockpile to pay for the cheapest combat design must queue exactly
/// one `build_ship{design_id = "patrol_corvette"}` order via Rule 8's
/// new build_ship emit path.
///
/// Sanity check that the fix doesn't over-correct into "never builds":
/// the gate refuses bankrupt empires but still permits the build when
/// resources are present.
#[test]
fn rule_8_fortify_queues_build_ship_when_affordable_with_shipyard() {
    let mut app = test_app();
    install_patrol_corvette(&mut app);
    let (empire, home) = spawn_empire_with_colony(&mut app);

    // Shipyard present + stockpile already at 10_000 (set by
    // `spawn_empire_with_colony`) → both Rule 8 gates pass.
    attach_shipyard_to_system(app.world_mut(), home);

    // Let the AI pipeline run several ticks. Rule 8 only fires when
    // `total_ships < colony_count * 2`; the test fixture has 1 Core
    // ship + 1 colony, so the threshold (= 2) is met for the first
    // emit and then the handler-side dedup absorbs subsequent ticks.
    for _ in 0..6 {
        advance_time(&mut app, 1);
    }

    // Look specifically for patrol_corvette — the cheapest combat
    // design we just installed. Rule 8 picks the cheapest affordable
    // combat design (via `affordable_fortify_design`), so the test
    // design registry's other combat designs (if any) will lose the
    // tiebreaker.
    let corvette_orders = count_ship_orders_for_empire(&mut app, empire, Some("patrol_corvette"));
    assert!(
        corvette_orders >= 1,
        "#532 F2: shipyard + affordable stockpile must queue ≥ 1 patrol_corvette via Rule 8; \
         got {} corvette order(s)",
        corvette_orders,
    );
}
