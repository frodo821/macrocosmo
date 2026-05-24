//! #445 regression: shipyard count actually multiplies build throughput.
//!
//! Pre-#445 every callsite reading `SystemModifiers.shipyard_capacity`
//! only checked `> 0` — so a system with two shipyards had the **same**
//! ship throughput as a system with one. `tick_build_queue` processed
//! only `queue[0]` per hexadie and the AI emitter capped the
//! `systems_with_shipyard` metric at the binary "has shipyard" tier.
//!
//! Post-fix the canonical modifier is renamed
//! `shipyard_build_parallel_slots`; `tick_build_queue` advances up to
//! `final_value()` head orders per hexadie and a separate
//! `shipyard_build_speed` modifier (default 1.0 multiplier) lets future
//! techs/modules speed throughput without piggybacking on the shipyard
//! building.
//!
//! These eight tests pin the new behaviour (5 baseline + 3 adversarial
//! fold-in from #445 review):
//!
//! 1. `shipyard_two_parallel_orders_progress_simultaneously` —
//!    two shipyards (= parallel_slots=2), three queued orders, one tick
//!    advances the first two by 1 hexadie each while the third is
//!    untouched.
//! 2. `shipyard_one_parallel_order_only_head_progresses` —
//!    single shipyard (= parallel_slots=1), three queued orders, only
//!    `queue[0]` decrements (regression: previous behaviour preserved
//!    for the single-shipyard case).
//! 3. `shipyard_zero_no_progress` — parallel_slots=0, queue is
//!    untouched and the "system lacks a Shipyard" warn fires.
//! 4. `shipyard_build_speed_default_1_0` — speed modifier untouched,
//!    `effective_delta == delta`, ten ticks decrement build_time by ten.
//! 5. `shipyard_build_speed_multiplier_2x_speeds_progress` — push a
//!    `+1.0` (= total 2.0×) speed modifier and confirm a single tick
//!    decrements build_time by two.
//! 6. `shipyard_parallel_cost_consumption_matches_slot_count` — fold-in
//!    HIGH: per-tick cost is prorated across parallel slots (regression:
//!    pre-fold-in slot 0 was greedy and slots 1+ got zero invested).
//! 7. `shipyard_build_speed_half_progress_accumulates_over_2_ticks` —
//!    fold-in BLOCKER: sub-1.0 speed multipliers accumulate over
//!    multiple ticks instead of being nullified by the floor-1 fallback.
//! 8. `shipyard_parallel_starvation_does_not_advance_build_time` —
//!    fold-in HIGH: a starved parallel slot's `build_time` does NOT
//!    decrement (regression: pre-fold-in it did, leading to
//!    instant-complete on resource refill).

mod common;

use bevy::prelude::*;
use macrocosmo::amount::{Amt, SignedAmt};
use macrocosmo::colony::building_queue::{BuildKind, BuildOrder, BuildQueue};
use macrocosmo::colony::{
    Buildings, Colony, FoodConsumption, MaintenanceCost, Production, ProductionFocus,
    ResourceCapacity, ResourceStockpile,
};
use macrocosmo::components::Position;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::SystemModifiers;
use macrocosmo::modifier::{ModifiedValue, Modifier};

use common::{advance_time, find_planet, spawn_test_empire, spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Build a `Modifier` that adds `units` parallel build slots. Used to seed
/// shipyard count without spawning station ships.
fn shipyard_slot_modifier(id: &str, units: i64) -> Modifier {
    Modifier {
        id: id.into(),
        label: id.into(),
        base_add: SignedAmt::units(units),
        multiplier: SignedAmt::ZERO,
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    }
}

/// Build a `Modifier` adding `units` to `shipyard_build_speed`. Base value
/// is 1.0, so `+1.0` becomes a 2× speed multiplier.
fn shipyard_speed_modifier(id: &str, units: i64) -> Modifier {
    Modifier {
        id: id.into(),
        label: id.into(),
        // We add to base, not multiplier. `final_value` = (base + base_add)
        // × (1.0 + Σ multiplier) — keeping the addition on base avoids
        // double-multiplying when future modifiers also touch base.
        base_add: SignedAmt::units(units),
        multiplier: SignedAmt::ZERO,
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    }
}

/// Like `shipyard_speed_modifier` but takes a milli-units offset so
/// sub-1.0 debuffs can be exercised (e.g. `-500` = -0.5, which combines
/// with the 1.0 base to a final 0.5× speed).
fn shipyard_speed_modifier_milli(id: &str, millis: i64) -> Modifier {
    Modifier {
        id: id.into(),
        label: id.into(),
        base_add: SignedAmt::milli(millis),
        multiplier: SignedAmt::ZERO,
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    }
}

/// Replace the `SystemModifiers` on `sys` with one tuned to the given
/// shipyard count + optional extra speed-modifier units. `speed_extra_units`
/// = 0 keeps the default `1.0` base.
fn install_shipyard_mods(world: &mut World, sys: Entity, shipyards: i64, speed_extra_units: i64) {
    let mut mods = SystemModifiers::default();
    if shipyards > 0 {
        mods.shipyard_build_parallel_slots
            .push_modifier(shipyard_slot_modifier("test_shipyards", shipyards));
    }
    if speed_extra_units != 0 {
        mods.shipyard_build_speed
            .push_modifier(shipyard_speed_modifier(
                "test_speed_boost",
                speed_extra_units,
            ));
    }
    world.entity_mut(sys).insert(mods);
}

/// Like `install_shipyard_mods` but takes the speed offset in milli-units
/// so sub-1.0 speeds can be exercised (e.g. `speed_extra_millis = -500`
/// for 0.5×).
fn install_shipyard_mods_milli(
    world: &mut World,
    sys: Entity,
    shipyards: i64,
    speed_extra_millis: i64,
) {
    let mut mods = SystemModifiers::default();
    if shipyards > 0 {
        mods.shipyard_build_parallel_slots
            .push_modifier(shipyard_slot_modifier("test_shipyards", shipyards));
    }
    if speed_extra_millis != 0 {
        mods.shipyard_build_speed
            .push_modifier(shipyard_speed_modifier_milli(
                "test_speed_milli",
                speed_extra_millis,
            ));
    }
    world.entity_mut(sys).insert(mods);
}

/// Helper: replace the stockpile on `sys` with a fully-stocked one so
/// build orders never starve.
fn set_full_stockpile(world: &mut World, sys: Entity) {
    world.entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::units(1_000_000),
            energy: Amt::units(1_000_000),
            research: Amt::ZERO,
            food: Amt::units(10_000),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));
}

/// Construct a BuildOrder with the given resource cost and build_time.
/// `minerals_invested` / `energy_invested` start at zero — the tick loop
/// transfers stockpile resources into them before checking completion.
fn make_order(name: &str, minerals: u64, energy: u64, build_time: i64) -> BuildOrder {
    BuildOrder {
        order_id: 0,
        kind: BuildKind::default(),
        design_id: "explorer_mk1".to_string(),
        display_name: name.to_string(),
        minerals_cost: Amt::units(minerals),
        minerals_invested: Amt::ZERO,
        energy_cost: Amt::units(energy),
        energy_invested: Amt::ZERO,
        build_time_total: build_time,
        build_time_remaining: build_time,
    }
}

/// Spawn a colony with the given `BuildQueue` already populated and return
/// the colony entity. The colony has no Buildings (only Shipyard gating via
/// SystemModifiers matters for `tick_build_queue`).
fn spawn_colony_with_queue(
    world: &mut World,
    sys: Entity,
    empire: Entity,
    queue: Vec<BuildOrder>,
) -> Entity {
    let planet = find_planet(world, sys);
    let mut bq = BuildQueue::default();
    // `push_order` assigns monotonic `order_id`s so the in-place
    // `is_complete()` check downstream behaves identically to a real
    // queue. Using raw `queue =` here would leave order_id=0 which is
    // fine but inconsistent with the production push path.
    for order in queue {
        bq.push_order(order);
    }
    world
        .spawn((
            Colony {
                planet,
                growth_rate: 0.0,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
                energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
                research_per_hexadies: ModifiedValue::new(Amt::ZERO),
                food_per_hexadies: ModifiedValue::new(Amt::ZERO),
            },
            bq,
            Buildings { slots: vec![None] },
            macrocosmo::colony::BuildingQueue::default(),
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
            FactionOwner(empire),
            Position::from([0.0, 0.0, 0.0]),
        ))
        .id()
}

/// Read the current `(build_time_remaining)` for each queued order at
/// `colony`. Order matches the queue order.
fn collect_build_times(world: &mut World, colony: Entity) -> Vec<i64> {
    world
        .get::<BuildQueue>(colony)
        .map(|bq| bq.queue.iter().map(|o| o.build_time_remaining).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// #1 — two shipyards advance two head orders simultaneously
// ---------------------------------------------------------------------------

#[test]
fn shipyard_two_parallel_orders_progress_simultaneously() {
    let mut app = test_app();
    let empire = spawn_test_empire(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Sys-2-Yards",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    set_full_stockpile(app.world_mut(), sys);
    install_shipyard_mods(app.world_mut(), sys, 2, 0);

    // Three orders, each with build_time=60. After 1 tick the first
    // two should decrement to 59 while the third stays at 60.
    let queue = vec![
        make_order("a", 100, 50, 60),
        make_order("b", 100, 50, 60),
        make_order("c", 100, 50, 60),
    ];
    let colony = spawn_colony_with_queue(app.world_mut(), sys, empire, queue);

    advance_time(&mut app, 1);

    let times = collect_build_times(app.world_mut(), colony);
    assert_eq!(
        times,
        vec![59, 59, 60],
        "first two head orders should each decrement by 1; the third (no parallel slot left) must stay at 60"
    );
}

// ---------------------------------------------------------------------------
// #2 — single shipyard preserves single-order behaviour
// ---------------------------------------------------------------------------

#[test]
fn shipyard_one_parallel_order_only_head_progresses() {
    let mut app = test_app();
    let empire = spawn_test_empire(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Sys-1-Yard",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    set_full_stockpile(app.world_mut(), sys);
    install_shipyard_mods(app.world_mut(), sys, 1, 0);

    let queue = vec![
        make_order("head", 100, 50, 60),
        make_order("mid", 100, 50, 60),
        make_order("tail", 100, 50, 60),
    ];
    let colony = spawn_colony_with_queue(app.world_mut(), sys, empire, queue);

    advance_time(&mut app, 1);

    let times = collect_build_times(app.world_mut(), colony);
    assert_eq!(
        times,
        vec![59, 60, 60],
        "single shipyard should only progress the head order — regression: pre-#445 behavior"
    );
}

// ---------------------------------------------------------------------------
// #3 — zero shipyards: queue is left untouched
// ---------------------------------------------------------------------------

#[test]
fn shipyard_zero_no_progress() {
    let mut app = test_app();
    let empire = spawn_test_empire(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Sys-0-Yards",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    set_full_stockpile(app.world_mut(), sys);
    // shipyards=0 — install default SystemModifiers without seeding the
    // parallel-slots modifier. The default `0` value should hit the
    // early-return in `tick_build_queue`.
    install_shipyard_mods(app.world_mut(), sys, 0, 0);

    let queue = vec![make_order("orphan", 100, 50, 60)];
    let colony = spawn_colony_with_queue(app.world_mut(), sys, empire, queue);

    advance_time(&mut app, 10);

    let times = collect_build_times(app.world_mut(), colony);
    assert_eq!(
        times,
        vec![60],
        "no shipyard ⇒ no progress; queue retained at full build_time"
    );
}

// ---------------------------------------------------------------------------
// #4 — speed modifier default 1.0 is identity
// ---------------------------------------------------------------------------

#[test]
fn shipyard_build_speed_default_1_0() {
    let mut app = test_app();
    let empire = spawn_test_empire(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Sys-Speed-1x",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    set_full_stockpile(app.world_mut(), sys);
    // Single shipyard, speed untouched (= 1.0 base default).
    install_shipyard_mods(app.world_mut(), sys, 1, 0);

    let queue = vec![make_order("a", 100, 50, 60)];
    let colony = spawn_colony_with_queue(app.world_mut(), sys, empire, queue);

    advance_time(&mut app, 10);

    let times = collect_build_times(app.world_mut(), colony);
    assert_eq!(
        times,
        vec![50],
        "default speed=1.0 ⇒ 10 ticks decrement build_time by 10"
    );
}

// ---------------------------------------------------------------------------
// #5 — speed multiplier 2x doubles per-tick decrement
// ---------------------------------------------------------------------------

#[test]
fn shipyard_build_speed_multiplier_2x_speeds_progress() {
    let mut app = test_app();
    let empire = spawn_test_empire(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Sys-Speed-2x",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    set_full_stockpile(app.world_mut(), sys);
    // Single shipyard, +1.0 speed (base 1.0 + 1.0 = 2.0× total).
    install_shipyard_mods(app.world_mut(), sys, 1, 1);

    let queue = vec![make_order("a", 100, 50, 60)];
    let colony = spawn_colony_with_queue(app.world_mut(), sys, empire, queue);

    advance_time(&mut app, 1);

    let times = collect_build_times(app.world_mut(), colony);
    // effective_delta = 1 * 2 = 2 → build_time decrements by 2 in a
    // single tick.
    assert_eq!(
        times,
        vec![58],
        "speed=2.0 ⇒ 1 tick decrements build_time by 2"
    );
}

// ---------------------------------------------------------------------------
// #6 (HIGH fold-in) — parallel cost is prorated per slot per tick
// ---------------------------------------------------------------------------
//
// Pre fold-in: `tick_build_queue` transferred up to `(cost - invested)`
// per slot per tick, greedily draining the stockpile into slot 0 and
// leaving slots 1+ with zero invested resources while still decrementing
// their build_time. Once resources arrived later, slots 1+ would
// instant-complete.
//
// Post fold-in: each slot needs `cost / build_time_total` per tick.
// With `parallel_slots=2` and `build_time_total=60` per order, a single
// tick should withdraw `2 * (100 / 60) ≈ 3.33` minerals and
// `2 * (50 / 60) ≈ 1.67` energy total from the stockpile, leaving each
// of the first two orders with their respective per-tick share invested.

#[test]
fn shipyard_parallel_cost_consumption_matches_slot_count() {
    use macrocosmo::colony::building_queue::BuildQueue;

    let mut app = test_app();
    let empire = spawn_test_empire(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Sys-Parallel-Cost",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    // Stock just enough to fund a few ticks comfortably but far less
    // than the full per-order cost.
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::units(1_000),
            energy: Amt::units(1_000),
            research: Amt::ZERO,
            food: Amt::units(1_000),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));
    install_shipyard_mods(app.world_mut(), sys, 2, 0);

    let queue = vec![
        make_order("a", 100, 100, 60),
        make_order("b", 100, 100, 60),
        make_order("c", 100, 100, 60),
    ];
    let colony = spawn_colony_with_queue(app.world_mut(), sys, empire, queue);

    advance_time(&mut app, 1);

    // 100 / 60 = 1.667 (Amt::div_amt truncates to milli precision).
    let per_tick_share = Amt::units(100).div_amt(Amt::units(60));
    let two_slot_consumption = per_tick_share.mul_u64(2);

    // Stockpile delta — slot 0 and slot 1 each pulled one share.
    let stockpile = app
        .world()
        .get::<ResourceStockpile>(sys)
        .expect("stockpile");
    assert_eq!(
        stockpile.minerals,
        Amt::units(1_000).sub(two_slot_consumption),
        "two parallel slots withdraw 2x per-tick share of minerals"
    );
    assert_eq!(
        stockpile.energy,
        Amt::units(1_000).sub(two_slot_consumption),
        "two parallel slots withdraw 2x per-tick share of energy"
    );

    // Per-order ledgers — first two orders each accumulated a single share.
    let bq = app.world().get::<BuildQueue>(colony).expect("bq");
    assert_eq!(
        bq.queue[0].minerals_invested, per_tick_share,
        "slot 0 invested its prorated minerals share"
    );
    assert_eq!(
        bq.queue[1].minerals_invested, per_tick_share,
        "slot 1 invested its prorated minerals share (no longer greedy-starved)"
    );
    assert_eq!(
        bq.queue[2].minerals_invested,
        Amt::ZERO,
        "slot 2 outside the parallel window stays untouched"
    );
    assert_eq!(
        bq.queue[0].energy_invested, per_tick_share,
        "slot 0 invested its prorated energy share"
    );
    assert_eq!(
        bq.queue[1].energy_invested, per_tick_share,
        "slot 1 invested its prorated energy share"
    );
}

// ---------------------------------------------------------------------------
// #7 (BLOCKER fold-in) — sub-1.0 speed accumulates across ticks
// ---------------------------------------------------------------------------
//
// Pre fold-in: `delta=1 * speed=0.5 = 0.5` rounded to 0, then the floor-1
// fallback bumped `effective_delta` back to 1 — nullifying the 0.5×
// debuff at GameSpeed=1. Post fold-in: the fractional accumulator
// carries the 0.5 forward to tick 2, where it adds to another 0.5 and
// crosses the integer threshold.

#[test]
fn shipyard_build_speed_half_progress_accumulates_over_2_ticks() {
    let mut app = test_app();
    let empire = spawn_test_empire(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Sys-Speed-Half",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    set_full_stockpile(app.world_mut(), sys);
    // Single shipyard, base_add = -500 millis → final speed = 0.5×.
    install_shipyard_mods_milli(app.world_mut(), sys, 1, -500);

    let queue = vec![make_order("half", 100, 50, 10)];
    let colony = spawn_colony_with_queue(app.world_mut(), sys, empire, queue);

    // Tick 1: accumulator = 0.5, effective_delta = 0, queue untouched.
    advance_time(&mut app, 1);
    let times_after_t1 = collect_build_times(app.world_mut(), colony);
    assert_eq!(
        times_after_t1,
        vec![10],
        "0.5× speed: tick 1 accrues 0.5 hexadie (sub-unit) — no whole hexadie of progress yet"
    );

    // Tick 2: accumulator = 0.5 + 0.5 = 1.0, effective_delta = 1, queue progresses by 1.
    advance_time(&mut app, 1);
    let times_after_t2 = collect_build_times(app.world_mut(), colony);
    assert_eq!(
        times_after_t2,
        vec![9],
        "0.5× speed: tick 2 crosses the integer threshold and decrements build_time by 1 \
         (= 1 hexadie of progress accumulated across 2 ticks, matching the displayed multiplier)"
    );
}

// ---------------------------------------------------------------------------
// #8 (HIGH fold-in) — starved parallel slot does NOT advance build_time
// ---------------------------------------------------------------------------
//
// Pre fold-in: with `parallel_slots=2` and a stockpile that could only
// fund slot 0, slot 1 still had its `build_time_remaining` decremented
// while its `minerals_invested` stayed at zero. Once resources caught
// up later, slot 1 would instant-complete. Post fold-in: slot 1 stalls
// entirely (no resource transfer, no build_time decrement).

#[test]
fn shipyard_parallel_starvation_does_not_advance_build_time() {
    use macrocosmo::colony::building_queue::BuildQueue;

    let mut app = test_app();
    let empire = spawn_test_empire(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Sys-Starve-Slot",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    // Two-slot setup. Each order's per-tick share of minerals is
    // 100/60 = 1.667. Stockpile is funded for exactly one slot's share
    // (= 2 minerals, well under 2 × 1.667 = 3.333) — slot 0 funds,
    // slot 1 stalls.
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::units(2),
            energy: Amt::units(1_000),
            research: Amt::ZERO,
            food: Amt::units(1_000),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));
    install_shipyard_mods(app.world_mut(), sys, 2, 0);

    let queue = vec![
        make_order("funded", 100, 100, 60),
        make_order("starved", 100, 100, 60),
    ];
    let colony = spawn_colony_with_queue(app.world_mut(), sys, empire, queue);

    advance_time(&mut app, 1);

    let bq = app.world().get::<BuildQueue>(colony).expect("bq");
    let per_tick_share = Amt::units(100).div_amt(Amt::units(60));

    // Slot 0: funded, build_time decremented, share invested.
    assert_eq!(
        bq.queue[0].build_time_remaining, 59,
        "funded slot 0 decrements build_time as normal"
    );
    assert_eq!(
        bq.queue[0].minerals_invested, per_tick_share,
        "funded slot 0 accumulates one share of minerals"
    );
    // Slot 1: starved (stockpile drained to ~0.33 after slot 0's pull),
    // build_time stays at 60, no resources invested.
    assert_eq!(
        bq.queue[1].build_time_remaining, 60,
        "starved slot 1 must NOT decrement build_time (regression: \
         pre-fold-in this would have been 59, leading to instant-complete \
         on resource refill)"
    );
    assert_eq!(
        bq.queue[1].minerals_invested,
        Amt::ZERO,
        "starved slot 1 invested zero — stockpile lacked its per-tick share"
    );
}
