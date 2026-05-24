//! Regression tests for #469 — `rank_survey_targets_for_ship` must
//! score candidate unsurveyed systems by ship-relative ETA
//! (FTL-to-waypoint + sublight remainder), not raw 3D distance.
//!
//! Coverage matches the issue's four acceptance criteria:
//!
//! 1. **FTL hub preference** — two equidistant targets, one with a
//!    surveyed waypoint inside the ship's FTL range, the other pure
//!    sublight. The FTL-reachable target wins.
//! 2. **Ship-relative ranking** — a ship at the frontier picks the
//!    target nearest itself, not the target nearest its ruler.
//! 3. **Per-ship greedy** — two ships, two targets each ship-nearest
//!    to one. Each ship gets its own nearest; no double-assignment.
//! 4. **Deterministic tie-break** — two same-ETA targets resolve by
//!    `Entity::index()`, stable across repeated calls.

use bevy::prelude::*;
use macrocosmo::ai::npc_decision::{rank_survey_targets_for_ship, score_survey_target_eta};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Make a target with a real `Entity` id and a position.
fn make_target(world: &mut World, pos: [f64; 3]) -> (Entity, [f64; 3]) {
    (world.spawn_empty().id(), pos)
}

// ---------------------------------------------------------------------------
// #1 FTL hub preference
// ---------------------------------------------------------------------------

/// Two unsurveyed targets equidistant from the ship in raw 3D, but
/// target_a sits 1ly from a surveyed waypoint inside the ship's FTL
/// range, while target_b is in deep space far from any waypoint. ETA
/// ranking must put target_a first because the FTL jump collapses
/// most of its travel time.
#[test]
fn rank_prefers_target_reachable_via_ftl_hub_over_pure_sublight() {
    let mut world = World::new();

    // Ship at origin. Two targets equidistant (~30ly), but a surveyed
    // waypoint sits 20ly out close to target_a:
    //   ship: [0, 0, 0]
    //   waypoint (surveyed): [20, 0, 0]
    //   target_a (unsurveyed): [21, 0, 0]   -> 21ly raw; 20ly FTL + 1ly sublight
    //   target_b (unsurveyed): [0, 21, 0]   -> 21ly raw, no waypoint near it
    let ship_pos = [0.0, 0.0, 0.0];
    let ruler_pos = [0.0, 0.0, 0.0]; // co-located → courier_delay = 0
    let surveyed = vec![[20.0, 0.0, 0.0]];

    let (target_a, pos_a) = make_target(&mut world, [21.0, 0.0, 0.0]);
    let (target_b, pos_b) = make_target(&mut world, [0.0, 21.0, 0.0]);
    let candidates = vec![(target_a, pos_a), (target_b, pos_b)];

    let ship_ftl_range = 25.0; // can reach the waypoint
    let ship_sublight = 0.5;

    let ranked = rank_survey_targets_for_ship(
        &candidates,
        &surveyed,
        ship_pos,
        ship_ftl_range,
        ship_sublight,
        ruler_pos,
    );

    assert_eq!(ranked.len(), 2);
    assert_eq!(
        ranked[0].0, target_a,
        "FTL-via-hub target must rank ahead of pure-sublight equidistant target"
    );
    assert_eq!(ranked[1].0, target_b);
    assert!(
        ranked[0].1 < ranked[1].1,
        "target_a ETA ({}) must be < target_b ETA ({})",
        ranked[0].1,
        ranked[1].1
    );
}

// ---------------------------------------------------------------------------
// #2 Ship-relative ranking
// ---------------------------------------------------------------------------

/// Ship is far from the ruler (out at the frontier). One target is
/// nearer to the ruler, another is nearer to the ship. The ETA-based
/// ranker must prefer the ship-nearest target — the old raw-distance +
/// home-tiebreak ranker would have picked the ruler-nearest one.
#[test]
fn rank_prefers_ship_nearest_target_when_ship_far_from_ruler() {
    let mut world = World::new();

    let ship_pos = [100.0, 0.0, 0.0]; // ship at the frontier
    let ruler_pos = [0.0, 0.0, 0.0]; // home base

    // c1 near ruler / far from ship; c2 near ship / far from ruler.
    let (c1, pos_c1) = make_target(&mut world, [3.0, 0.0, 0.0]);
    let (c2, pos_c2) = make_target(&mut world, [103.0, 0.0, 0.0]);
    let candidates = vec![(c1, pos_c1), (c2, pos_c2)];
    // No surveyed waypoints — every score collapses to pure sublight
    // from ship_pos, which is what we want to assert.
    let surveyed: Vec<[f64; 3]> = vec![];

    let ship_ftl_range = 0.0; // no FTL → only sublight matters
    let ship_sublight = 0.5;

    let ranked = rank_survey_targets_for_ship(
        &candidates,
        &surveyed,
        ship_pos,
        ship_ftl_range,
        ship_sublight,
        ruler_pos,
    );

    assert_eq!(ranked.len(), 2);
    assert_eq!(
        ranked[0].0, c2,
        "ship-nearest target must rank first (was selecting ruler-nearest pre-#469)"
    );
    assert_eq!(ranked[1].0, c1);
}

// ---------------------------------------------------------------------------
// #3 Per-ship greedy (no double-assignment)
// ---------------------------------------------------------------------------

/// Two ships at different positions, two targets each ship-nearest to
/// one. The greedy 1-pass assignment must give each ship its own
/// nearest target — never both ships to the same target.
///
/// This exercises the greedy logic the way `npc_decision_tick` runs
/// it: rank per ship, claim the head, drop it from the pool, rank the
/// next ship.
#[test]
fn greedy_per_ship_assigns_each_ship_to_its_nearest_target() {
    let mut world = World::new();

    let ruler_pos = [0.0, 0.0, 0.0];
    let ship_alpha_pos = [-50.0, 0.0, 0.0];
    let ship_bravo_pos = [50.0, 0.0, 0.0];

    let (target_left, pos_left) = make_target(&mut world, [-55.0, 0.0, 0.0]);
    let (target_right, pos_right) = make_target(&mut world, [55.0, 0.0, 0.0]);
    let candidates = vec![(target_left, pos_left), (target_right, pos_right)];
    let surveyed: Vec<[f64; 3]> = vec![];

    let ftl_range = 0.0;
    let sublight = 0.5;

    // Greedy 1-pass mirroring `npc_decision_tick`'s inner loop.
    let mut claimed: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    let mut assignments: Vec<(Entity, Entity)> = Vec::new();
    // Use real entities so the greedy step has stable identity.
    let ship_alpha = world.spawn_empty().id();
    let ship_bravo = world.spawn_empty().id();
    let ships = [(ship_alpha, ship_alpha_pos), (ship_bravo, ship_bravo_pos)];

    for (ship, pos) in ships {
        let remaining: Vec<(Entity, [f64; 3])> = candidates
            .iter()
            .copied()
            .filter(|(t, _)| !claimed.contains(t))
            .collect();
        let ranked = rank_survey_targets_for_ship(
            &remaining, &surveyed, pos, ftl_range, sublight, ruler_pos,
        );
        let (best, _) = ranked.first().copied().expect("at least one target");
        assignments.push((ship, best));
        claimed.insert(best);
    }

    assert_eq!(assignments.len(), 2);
    // Alpha → target_left (its nearest); Bravo → target_right.
    assert!(
        assignments.contains(&(ship_alpha, target_left)),
        "alpha must claim target_left (its ship-nearest)",
    );
    assert!(
        assignments.contains(&(ship_bravo, target_right)),
        "bravo must claim target_right (its ship-nearest)",
    );
    // And neither target is double-claimed.
    let targets: Vec<Entity> = assignments.iter().map(|(_, t)| *t).collect();
    let unique: std::collections::HashSet<_> = targets.iter().copied().collect();
    assert_eq!(
        unique.len(),
        targets.len(),
        "no target may be double-claimed"
    );
}

// ---------------------------------------------------------------------------
// #4 Deterministic tie-break
// ---------------------------------------------------------------------------

/// Two candidates with identical ETA: the rank order must be
/// determined by `Entity::index()` ascending, and must be stable
/// across repeated invocations of the same `World`.
#[test]
fn tiebreak_resolves_same_score_by_entity_index_and_is_stable() {
    let mut world = World::new();

    let ship_pos = [0.0, 0.0, 0.0];
    let ruler_pos = [0.0, 0.0, 0.0];
    let surveyed: Vec<[f64; 3]> = vec![];

    // Two targets at the same distance → identical ETA.
    let lo_index = world.spawn_empty().id();
    let hi_index = world.spawn_empty().id();
    assert!(
        lo_index.index() < hi_index.index(),
        "deterministic Entity allocation: spawn order = index order"
    );
    let candidates = vec![(hi_index, [5.0, 0.0, 0.0]), (lo_index, [-5.0, 0.0, 0.0])];

    let run1 = rank_survey_targets_for_ship(&candidates, &surveyed, ship_pos, 0.0, 0.5, ruler_pos);
    let run2 = rank_survey_targets_for_ship(&candidates, &surveyed, ship_pos, 0.0, 0.5, ruler_pos);

    assert_eq!(run1.len(), 2);
    assert_eq!(run1[0].1, run1[1].1, "test setup: ETAs must match");
    assert_eq!(
        run1[0].0, lo_index,
        "same-ETA tie must resolve to lower Entity::index() first"
    );
    assert_eq!(run1[1].0, hi_index);
    assert_eq!(run1, run2, "ranking must be stable across repeated calls");
}

// ---------------------------------------------------------------------------
// Sanity coverage for `score_survey_target_eta` directly
// ---------------------------------------------------------------------------

#[test]
fn score_pure_sublight_when_no_ftl() {
    // No FTL, 10ly at 0.5c → 1200 hexadies (10 / (1/60 * 0.5) = 1200).
    let eta = score_survey_target_eta([10.0, 0.0, 0.0], [0.0, 0.0, 0.0], 0.0, 0.5, &[]);
    assert_eq!(eta, Some(1200));
}

#[test]
fn score_returns_none_when_ship_immobile() {
    // No FTL, no sublight, no waypoint coincident with target → None.
    let eta = score_survey_target_eta([10.0, 0.0, 0.0], [0.0, 0.0, 0.0], 0.0, 0.0, &[]);
    assert_eq!(eta, None);
}

#[test]
fn score_ftl_beats_sublight_when_hub_available() {
    // 100ly target, ship has FTL 60ly + sublight 0.5c. Waypoint at
    // 60ly along the way.
    //
    // Pure sublight: 100 / (1/60 * 0.5) = 12000 hexadies.
    // FTL leg: 60ly at 10c → 60 / (60 * 10) yr = 0.1yr = 6 hex (ceil).
    // Sublight remainder: 40 / (1/60 * 0.5) = 4800 hex.
    // FTL-assisted total ≈ 4806.
    let eta_ftl = score_survey_target_eta(
        [100.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        60.0,
        0.5,
        &[[60.0, 0.0, 0.0]],
    )
    .expect("reachable");

    let eta_pure_sublight = score_survey_target_eta(
        [100.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        0.0,
        0.5,
        &[[60.0, 0.0, 0.0]],
    )
    .expect("reachable");

    assert!(
        eta_ftl < eta_pure_sublight,
        "FTL-assisted ETA ({}) must beat pure-sublight ETA ({})",
        eta_ftl,
        eta_pure_sublight,
    );
}

#[test]
fn rank_drops_unreachable_targets() {
    let mut world = World::new();
    let reachable = world.spawn_empty().id();
    let unreachable = world.spawn_empty().id();

    let candidates = vec![
        (reachable, [10.0, 0.0, 0.0]),
        (unreachable, [10.0, 0.0, 0.0]),
    ];
    // Ship has no propulsion → every target unreachable → both dropped.
    let ranked =
        rank_survey_targets_for_ship(&candidates, &[], [0.0, 0.0, 0.0], 0.0, 0.0, [0.0, 0.0, 0.0]);
    assert!(ranked.is_empty(), "no propulsion → no rankable targets");

    // Restore propulsion — both targets are reachable now (they're at
    // the same distance, so deterministic tie-break orders them by
    // Entity::index()).
    let ranked =
        rank_survey_targets_for_ship(&candidates, &[], [0.0, 0.0, 0.0], 0.0, 0.5, [0.0, 0.0, 0.0]);
    assert_eq!(ranked.len(), 2);
    let _ = (reachable, unreachable);
}
