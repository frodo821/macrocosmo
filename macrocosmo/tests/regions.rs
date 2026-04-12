//! #145: Regression tests for forbidden regions (nebulae, subspace storms).
//!
//! These tests exercise the public API directly (region math, placement,
//! A* edge blocking) rather than going through a full Bevy app — that keeps
//! them fast and deterministic.

use std::collections::HashMap;

use bevy::prelude::Entity;
use macrocosmo::galaxy::region::{
    build_ftl_adjacency, effective_radius, place_regions, segment_sphere_intersects,
    validate_constraints, CapabilityParams, ForbiddenRegion, PlacementInputs,
    RegionBlockSnapshot, RegionSpec, RegionTypeDefinition,
};
use macrocosmo::ship::routing::{plan_route_full, RouteSystemSnapshot};
use macrocosmo::ship::RulesOfEngagement;

fn caps(entries: &[(&str, f64)]) -> HashMap<String, CapabilityParams> {
    entries
        .iter()
        .map(|(k, s)| {
            (
                (*k).to_string(),
                CapabilityParams { strength: *s },
            )
        })
        .collect()
}

fn test_region_type(id: &str, entries: &[(&str, f64)]) -> RegionTypeDefinition {
    RegionTypeDefinition {
        id: id.into(),
        name: id.into(),
        capabilities: caps(entries),
        visual_color: [0.3, 0.1, 0.5],
        visual_density: 0.7,
    }
}

fn entity(n: u64) -> Entity {
    Entity::from_bits(n + 1)
}

fn snapshot(i: usize, e: Entity, p: [f64; 3], surveyed: bool) -> RouteSystemSnapshot {
    RouteSystemSnapshot {
        index: i,
        entity: e,
        pos: p,
        surveyed,
        hostile_known: false,
    }
}

#[test]
fn effective_radius_matches_metaball_isosurface() {
    // F(d) = r² / d² ; at threshold t, d_eff = r / sqrt(t).
    assert!((effective_radius(10.0, 1.0) - 10.0).abs() < 1e-9);
    assert!((effective_radius(10.0, 4.0) - 5.0).abs() < 1e-9);
    assert!((effective_radius(6.0, 9.0) - 2.0).abs() < 1e-9);
}

#[test]
fn segment_sphere_tangent_not_blocked() {
    // Segment tangent to sphere (closest dist == radius): treat as non-blocking.
    // (We use strict inequality in the source; tangent = no overlap.)
    assert!(!segment_sphere_intersects(
        [-5.0, 1.0, 0.0],
        [5.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
        1.0,
    ));
}

#[test]
fn test_region_blocks_ftl_route() {
    // Three systems in a line: 0 → blocker wall at origin → 2.
    // FTL from 0 to 2 must fail if we wall the middle; plan_route_full should
    // still find a detour via an un-blocked midpoint.
    let e0 = entity(0);
    let e1 = entity(1); // wall location — detour available above it
    let e2 = entity(2);
    let e3 = entity(3); // detour node off-axis
    let systems = vec![
        snapshot(0, e0, [0.0, 0.0, 0.0], true),
        snapshot(1, e1, [10.0, 0.0, 0.0], true),
        snapshot(2, e2, [20.0, 0.0, 0.0], true),
        snapshot(3, e3, [10.0, 10.0, 0.0], true),
    ];

    // Direct route with no blockers: should be 1 FTL hop (if range >= 20).
    let plain = plan_route_full(
        [0.0, 0.0, 0.0],
        2,
        25.0,
        0.5,
        10.0,
        &systems,
        RulesOfEngagement::Defensive,
        &[],
    );
    assert!(plain.is_some());

    // With a blocker at (10, 0) covering ±3 ly on the x-axis,
    // the direct 0→2 FTL edge is severed. A detour via system 3 is allowed
    // (edges 0→3 and 3→2 go off-axis and clear the region).
    let mut blocker_caps = HashMap::new();
    blocker_caps.insert("blocks_ftl".into(), CapabilityParams { strength: 1.0 });
    let region = ForbiddenRegion {
        id: 1,
        type_id: "dark_nebula".into(),
        spheres: vec![([10.0, 0.0, 0.0], 3.0)],
        threshold: 1.0,
        capabilities: blocker_caps,
    };
    let blockers = vec![RegionBlockSnapshot::from_region(&region)];

    let detour = plan_route_full(
        [0.0, 0.0, 0.0],
        2,
        25.0,
        0.5,
        10.0,
        &systems,
        RulesOfEngagement::Defensive,
        &blockers,
    );
    assert!(detour.is_some(), "should still find a detour");
    let route = detour.unwrap();
    // We expect at least 2 segments (detour via e3) rather than a direct FTL hop.
    // Direct 0→2 FTL is blocked; 0→3→2 FTL path avoids the region.
    assert!(
        route.segments.len() >= 2,
        "expected detour via >=2 hops, got {} segments",
        route.segments.len()
    );
}

#[test]
fn test_region_does_not_break_connectivity() {
    // Grid galaxy; placement must keep connectivity.
    use rand::SeedableRng;
    let mut rng = rand::rngs::SmallRng::seed_from_u64(123);
    let mut systems = Vec::new();
    for i in -4..=4 {
        for j in -4..=4 {
            systems.push([i as f64 * 4.0, j as f64 * 4.0, 0.0]);
        }
    }
    let capital_idx = systems
        .iter()
        .position(|p| p[0] == 0.0 && p[1] == 0.0)
        .unwrap();
    let inputs = PlacementInputs::new(&systems, capital_idx, 30.0);

    let mut types = HashMap::new();
    types.insert(
        "dark_nebula".into(),
        test_region_type("dark_nebula", &[("blocks_ftl", 1.0)]),
    );

    let specs = vec![RegionSpec {
        type_id: "dark_nebula".into(),
        count_range: (3, 6),
        sphere_count_range: (2, 4),
        sphere_radius_range: (2.0, 4.0),
        min_distance_from_capital: 15.0,
        threshold: 1.0,
    }];

    let output = place_regions(&mut rng, &inputs, &types, &specs);
    assert!(validate_constraints(&output.regions, &inputs));

    // C3 sanity: the full adjacency still has a single dominant component.
    let blockers: Vec<_> = output
        .regions
        .iter()
        .filter(|r| r.has_capability("blocks_ftl"))
        .map(RegionBlockSnapshot::from_region)
        .collect();
    let adj = build_ftl_adjacency(&systems, 6.0, &blockers);
    let reachable = {
        let mut visited = vec![false; systems.len()];
        let mut stack = vec![capital_idx];
        visited[capital_idx] = true;
        let mut count = 1;
        while let Some(u) = stack.pop() {
            for &v in &adj[u] {
                if !visited[v] {
                    visited[v] = true;
                    count += 1;
                    stack.push(v);
                }
            }
        }
        count
    };
    assert!(
        (reachable as f64 / systems.len() as f64) >= 0.9,
        "capital reaches {}/{} systems — not ≥ 90%",
        reachable,
        systems.len()
    );
}

#[test]
fn test_region_placement_constraints_100_samples() {
    use rand::SeedableRng;

    let mut systems = Vec::new();
    // Denser galaxy for this stress test — 9x9 grid with spacing 4.
    for i in -4..=4 {
        for j in -4..=4 {
            systems.push([i as f64 * 4.0, j as f64 * 4.0, 0.0]);
        }
    }
    let capital_idx = systems
        .iter()
        .position(|p| p[0] == 0.0 && p[1] == 0.0)
        .unwrap();
    let inputs = PlacementInputs::new(&systems, capital_idx, 30.0);

    let mut types = HashMap::new();
    types.insert(
        "dark_nebula".into(),
        test_region_type("dark_nebula", &[("blocks_ftl", 1.0)]),
    );

    let specs = vec![RegionSpec {
        type_id: "dark_nebula".into(),
        count_range: (2, 4),
        sphere_count_range: (2, 3),
        sphere_radius_range: (2.0, 4.0),
        min_distance_from_capital: 15.0,
        threshold: 1.0,
    }];

    for seed in 0..100 {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(seed);
        let output = place_regions(&mut rng, &inputs, &types, &specs);
        assert!(
            validate_constraints(&output.regions, &inputs),
            "seed {}: validation failed with {} regions",
            seed,
            output.regions.len()
        );
    }
}

#[test]
fn test_blocks_ftl_comm_independent_from_blocks_ftl() {
    // A region that only blocks FTL comm must NOT cause the routing planner
    // to drop FTL edges.
    let e0 = entity(0);
    let e1 = entity(1);
    let systems = vec![
        snapshot(0, e0, [0.0, 0.0, 0.0], true),
        snapshot(1, e1, [10.0, 0.0, 0.0], true),
    ];

    let mut comm_caps = HashMap::new();
    comm_caps.insert("blocks_ftl_comm".into(), CapabilityParams { strength: 1.0 });
    let region = ForbiddenRegion {
        id: 1,
        type_id: "comm_jammer".into(),
        spheres: vec![([5.0, 0.0, 0.0], 3.0)],
        threshold: 1.0,
        capabilities: comm_caps,
    };

    // Simulate the routing collector: only `blocks_ftl` regions become blockers.
    let blockers: Vec<RegionBlockSnapshot> = std::iter::once(&region)
        .filter(|r| r.has_capability("blocks_ftl"))
        .map(RegionBlockSnapshot::from_region)
        .collect();
    assert!(blockers.is_empty());

    let route = plan_route_full(
        [0.0, 0.0, 0.0],
        1,
        15.0,
        0.5,
        10.0,
        &systems,
        RulesOfEngagement::Defensive,
        &blockers,
    );
    assert!(route.is_some());
    assert_eq!(route.unwrap().segments.len(), 1);
}
