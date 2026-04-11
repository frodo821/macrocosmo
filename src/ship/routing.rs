//! #128: Mixed multi-segment route planning with async computation.
//!
//! Provides an A* route planner that finds optimal mixed FTL/sublight routes,
//! runs as an async task off the main thread, and integrates with the ECS via
//! `PendingRoute` component and `poll_pending_routes` system.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use std::collections::BinaryHeap;
use std::cmp::Ordering;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::physics::distance_ly_arr;
use crate::time_system::HEXADIES_PER_YEAR;

use super::{
    CommandQueue, Ship, ShipState, QueuedCommand,
    start_ftl_travel_with_bonus, start_sublight_travel_with_bonus,
    INITIAL_FTL_SPEED_C, PORT_FTL_RANGE_BONUS_LY,
};

/// Maximum sublight edge distance in light-years (caps edge count in A*).
pub const MAX_SUBLIGHT_EDGE_LY: f64 = 30.0;

/// Snapshot of a star system for async route planning (no ECS references).
#[derive(Clone, Debug)]
pub struct RouteSystemSnapshot {
    pub index: usize,
    pub entity: Entity,
    pub pos: [f64; 3],
    pub surveyed: bool,
}

/// A single segment of a planned route.
#[derive(Clone, Debug)]
pub enum RouteSegment {
    /// FTL jump to a star system.
    FTL { to: Entity },
    /// Sub-light travel to a position, optionally associated with a star system.
    SubLight { to_pos: [f64; 3], to_system: Option<Entity> },
}

/// A complete planned route consisting of ordered segments.
#[derive(Clone, Debug)]
pub struct PlannedRoute {
    pub segments: Vec<RouteSegment>,
}

/// Component attached to a ship while its route is being computed asynchronously.
#[derive(Component)]
pub struct PendingRoute {
    pub task: Task<Option<PlannedRoute>>,
    pub target_system: Entity,
}

/// Resource: count of pending route computations. When > 0, game time is paused.
#[derive(Resource, Default)]
pub struct RouteCalculationsPending {
    pub count: u32,
}

// --- A* internals ---

#[derive(Clone)]
struct AStarNode {
    /// Index into the systems slice.
    system_index: usize,
    /// g-cost: actual travel time from origin (in hexadies-equivalent float).
    g_cost: f64,
    /// f-cost: g_cost + heuristic.
    f_cost: f64,
}

impl PartialEq for AStarNode {
    fn eq(&self, other: &Self) -> bool {
        self.f_cost == other.f_cost
    }
}

impl Eq for AStarNode {}

impl PartialOrd for AStarNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AStarNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap behavior with BinaryHeap (max-heap).
        other.f_cost.partial_cmp(&self.f_cost).unwrap_or(Ordering::Equal)
    }
}

/// Whether the edge from `from` to `to` should be FTL or sublight.
#[derive(Clone, Copy, Debug, PartialEq)]
enum EdgeKind {
    FTL,
    SubLight,
}

/// Pure A* route planner. No ECS references — safe to call from async tasks.
///
/// Finds the fastest mixed FTL/sublight route from `origin_pos` to the system
/// at `destination_index` in the `systems` slice.
///
/// - FTL edges: to any *surveyed* system within `ftl_range`.
/// - SubLight edges: to any system within `MAX_SUBLIGHT_EDGE_LY`.
/// - Heuristic: Euclidean distance / ftl_speed (admissible).
///
/// Returns `None` if no route exists.
pub fn plan_route(
    origin_pos: [f64; 3],
    destination_index: usize,
    ftl_range: f64,
    sublight_speed: f64,
    ftl_speed: f64,
    systems: &[RouteSystemSnapshot],
) -> Option<PlannedRoute> {
    if destination_index >= systems.len() {
        return None;
    }

    let dest_pos = systems[destination_index].pos;
    let n = systems.len();

    // Heuristic: straight-line distance / max_speed (admissible lower bound).
    let max_speed = if ftl_speed > 0.0 { ftl_speed } else { sublight_speed };
    if max_speed <= 0.0 {
        return None;
    }
    let heuristic = |pos: [f64; 3]| -> f64 {
        distance_ly_arr(pos, dest_pos) * HEXADIES_PER_YEAR as f64 / max_speed
    };

    // Find which system index the ship starts at (if any). We treat origin as
    // a virtual node with index `n` that has edges to reachable systems.
    let origin_at_system: Option<usize> = systems.iter().position(|s| {
        distance_ly_arr(origin_pos, s.pos) < 1e-9
    });

    // If the ship is already at the destination, return empty route.
    if origin_at_system == Some(destination_index) {
        return Some(PlannedRoute { segments: vec![] });
    }

    // g_costs[i] = best known g-cost to system i. Index n = origin virtual node.
    let mut g_costs = vec![f64::MAX; n + 1];
    // came_from[i] = (parent_index, edge_kind). Index n = origin.
    let mut came_from: Vec<Option<(usize, EdgeKind)>> = vec![None; n + 1];

    let origin_index = origin_at_system.unwrap_or(n);
    g_costs[origin_index] = 0.0;

    let mut open = BinaryHeap::new();
    open.push(AStarNode {
        system_index: origin_index,
        g_cost: 0.0,
        f_cost: heuristic(origin_pos),
    });

    while let Some(current) = open.pop() {
        let ci = current.system_index;

        // Skip stale entries.
        if current.g_cost > g_costs[ci] {
            continue;
        }

        // Reached destination?
        if ci == destination_index {
            break;
        }

        let current_pos = if ci == n { origin_pos } else { systems[ci].pos };

        // Generate edges to all systems.
        for j in 0..n {
            if j == ci {
                continue;
            }
            // Skip origin virtual node as a target (it's not a real system).
            if j == n {
                continue;
            }

            let target = &systems[j];
            let dist = distance_ly_arr(current_pos, target.pos);

            // Determine possible edge kinds.
            let mut edges: Vec<(EdgeKind, f64)> = Vec::new();

            // FTL edge: only to surveyed systems within range.
            // If current node is the origin virtual node (not at a system),
            // FTL is only available if origin_at_system is Some (ship is docked).
            let can_ftl = if ci == n {
                origin_at_system.is_some()
            } else {
                true
            };

            if can_ftl && ftl_range > 0.0 && target.surveyed && dist <= ftl_range {
                let cost = dist * HEXADIES_PER_YEAR as f64 / ftl_speed;
                edges.push((EdgeKind::FTL, cost));
            }

            // SubLight edge: to any system within MAX_SUBLIGHT_EDGE_LY.
            if sublight_speed > 0.0 && dist <= MAX_SUBLIGHT_EDGE_LY {
                let cost = dist * HEXADIES_PER_YEAR as f64 / sublight_speed;
                edges.push((EdgeKind::SubLight, cost));
            }

            for (kind, cost) in edges {
                let new_g = g_costs[ci] + cost;
                if new_g < g_costs[j] {
                    g_costs[j] = new_g;
                    came_from[j] = Some((ci, kind));
                    open.push(AStarNode {
                        system_index: j,
                        g_cost: new_g,
                        f_cost: new_g + heuristic(target.pos),
                    });
                }
            }
        }
    }

    // Reconstruct path from destination back to origin.
    if g_costs[destination_index] == f64::MAX {
        return None;
    }

    let mut path: Vec<(usize, EdgeKind)> = Vec::new();
    let mut current = destination_index;
    while let Some((parent, kind)) = came_from[current] {
        path.push((current, kind));
        if parent == origin_index {
            break;
        }
        current = parent;
    }
    path.reverse();

    let segments = path
        .into_iter()
        .map(|(sys_idx, kind)| {
            let snap = &systems[sys_idx];
            match kind {
                EdgeKind::FTL => RouteSegment::FTL { to: snap.entity },
                EdgeKind::SubLight => RouteSegment::SubLight {
                    to_pos: snap.pos,
                    to_system: Some(snap.entity),
                },
            }
        })
        .collect();

    Some(PlannedRoute { segments })
}

/// Collect system snapshots from the ECS for async route planning.
pub fn collect_route_snapshots(
    systems: &Query<(Entity, &StarSystem, &Position), Without<Ship>>,
) -> Vec<RouteSystemSnapshot> {
    systems
        .iter()
        .enumerate()
        .map(|(i, (entity, star, pos))| RouteSystemSnapshot {
            index: i,
            entity,
            pos: pos.as_array(),
            surveyed: star.surveyed,
        })
        .collect()
}

/// Spawn an async route computation task.
pub fn spawn_route_task(
    origin_pos: [f64; 3],
    destination: Entity,
    ftl_range: f64,
    sublight_speed: f64,
    ftl_speed: f64,
    systems: Vec<RouteSystemSnapshot>,
) -> Task<Option<PlannedRoute>> {
    let pool = AsyncComputeTaskPool::get();
    let dest_index = systems.iter().position(|s| s.entity == destination);
    pool.spawn(async move {
        let dest_idx = dest_index?;
        plan_route(origin_pos, dest_idx, ftl_range, sublight_speed, ftl_speed, &systems)
    })
}

/// System that polls pending route computations and applies completed routes.
///
/// When a route completes:
/// 1. Removes the `PendingRoute` component.
/// 2. Consumes the head `MoveTo` command from the queue.
/// 3. Starts executing the first segment (FTL or sublight).
/// 4. Prepends remaining segments as `MoveTo` commands.
pub fn poll_pending_routes(
    mut commands: Commands,
    clock: Res<crate::time_system::GameClock>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    mut ships: Query<(
        Entity,
        &Ship,
        &mut ShipState,
        &mut CommandQueue,
        &Position,
    ), With<PendingRoute>>,
    mut pending_q: Query<&mut PendingRoute>,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    system_buildings: Query<&crate::colony::SystemBuildings>,
    mut pending_count: ResMut<RouteCalculationsPending>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };

    // Collect ship entities first to avoid borrow conflicts.
    let ship_entities: Vec<Entity> = ships.iter().map(|(e, ..)| e).collect();

    for ship_entity in ship_entities {
        let Ok(mut pending) = pending_q.get_mut(ship_entity) else {
            continue;
        };

        // Poll the task.
        let result = block_on(poll_once(&mut pending.task));
        let Some(route_option) = result else {
            // Task not finished yet.
            continue;
        };

        let target_system = pending.target_system;

        // Remove PendingRoute component and decrement counter.
        commands.entity(ship_entity).remove::<PendingRoute>();
        pending_count.count = pending_count.count.saturating_sub(1);

        let Ok((_, ship, mut state, mut queue, ship_pos)) = ships.get_mut(ship_entity) else {
            continue;
        };

        // Ensure ship is still docked (may have changed state while waiting).
        let ShipState::Docked { system: docked_system } = *state else {
            // Ship moved; discard the route, remove the MoveTo from queue head if still there.
            if matches!(queue.commands.first(), Some(QueuedCommand::MoveTo { system }) if *system == target_system) {
                queue.commands.remove(0);
            }
            queue.sync_prediction(ship_pos.as_array(), None);
            continue;
        };

        // Consume the MoveTo from the queue head.
        if matches!(queue.commands.first(), Some(QueuedCommand::MoveTo { system }) if *system == target_system) {
            queue.commands.remove(0);
        }

        let Some(route) = route_option else {
            // No route found — fall back to direct sublight.
            if let Ok((_, _target_star, target_pos)) = systems.get(target_system) {
                let Ok((_, _, origin_pos)) = systems.get(docked_system) else {
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };
                start_sublight_travel_with_bonus(
                    &mut state,
                    origin_pos,
                    ship,
                    *target_pos,
                    Some(target_system),
                    clock.elapsed,
                    global_params.sublight_speed_bonus,
                );
                info!("Route planner: no route found for {}, falling back to sublight", ship.name);
            }
            queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
            continue;
        };

        if route.segments.is_empty() {
            // Already at destination.
            queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
            continue;
        }

        // Execute first segment, push remaining as MoveTo commands.
        let first = &route.segments[0];
        let remaining = &route.segments[1..];

        // Prepend remaining segments as MoveTo commands (in reverse order to maintain order).
        for seg in remaining.iter().rev() {
            match seg {
                RouteSegment::FTL { to } => {
                    queue.commands.insert(0, QueuedCommand::MoveTo { system: *to });
                }
                RouteSegment::SubLight { to_system: Some(sys), .. } => {
                    queue.commands.insert(0, QueuedCommand::MoveTo { system: *sys });
                }
                RouteSegment::SubLight { to_pos, to_system: None } => {
                    // Sublight to a non-system position — unusual, but handle gracefully.
                    // For now this shouldn't happen since all snapshots are systems.
                    warn!("Route segment to non-system position {:?}, skipping", to_pos);
                }
            }
        }

        // Execute the first segment.
        match first {
            RouteSegment::FTL { to } => {
                let Ok((_, first_star, first_pos)) = systems.get(*to) else {
                    warn!("Route planner: FTL target no longer exists for {}", ship.name);
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };
                let Ok((_, _, origin_pos)) = systems.get(docked_system) else {
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };
                let origin_has_port = system_buildings.get(docked_system).is_ok_and(|sb| sb.has_port());
                match start_ftl_travel_with_bonus(
                    &mut state,
                    ship,
                    docked_system,
                    *to,
                    origin_pos,
                    first_pos,
                    clock.elapsed,
                    global_params.ftl_range_bonus,
                    global_params.ftl_speed_multiplier,
                    origin_has_port,
                ) {
                    Ok(()) => {
                        info!(
                            "Route planner: {} FTL jumping to {} ({} segments remaining)",
                            ship.name, first_star.name, remaining.len()
                        );
                    }
                    Err(e) => {
                        warn!("Route planner: FTL hop failed for {}: {}, falling back to sublight", ship.name, e);
                        // Fall back to sublight for this segment.
                        start_sublight_travel_with_bonus(
                            &mut state,
                            origin_pos,
                            ship,
                            *first_pos,
                            Some(*to),
                            clock.elapsed,
                            global_params.sublight_speed_bonus,
                        );
                    }
                }
            }
            RouteSegment::SubLight { to_pos, to_system } => {
                let Ok((_, _, origin_pos)) = systems.get(docked_system) else {
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };
                let dest_pos = Position::from(*to_pos);
                start_sublight_travel_with_bonus(
                    &mut state,
                    origin_pos,
                    ship,
                    dest_pos,
                    *to_system,
                    clock.elapsed,
                    global_params.sublight_speed_bonus,
                );
                info!(
                    "Route planner: {} sublight to {:?} ({} segments remaining)",
                    ship.name, to_system, remaining.len()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create test entities with distinct bit patterns.
    fn test_entity(n: u64) -> Entity {
        // Use n+1 as index (must be non-zero for EntityIndex), generation 0.
        Entity::from_bits(n + 1)
    }

    fn make_snapshot(index: usize, entity: Entity, pos: [f64; 3], surveyed: bool) -> RouteSystemSnapshot {
        RouteSystemSnapshot { index, entity, pos, surveyed }
    }

    #[test]
    fn direct_ftl_route() {
        let e0 = test_entity(0);
        let e1 = test_entity(1);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
            make_snapshot(1, e1, [5.0, 0.0, 0.0], true),
        ];
        let result = plan_route([0.0, 0.0, 0.0], 1, 10.0, 0.1, 10.0, &systems);
        assert!(result.is_some());
        let route = result.unwrap();
        assert_eq!(route.segments.len(), 1);
        assert!(matches!(route.segments[0], RouteSegment::FTL { to } if to == e1));
    }

    #[test]
    fn multi_hop_ftl_chain() {
        let e0 = test_entity(0);
        let e1 = test_entity(1);
        let e2 = test_entity(2);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
            make_snapshot(1, e1, [5.0, 0.0, 0.0], true),
            make_snapshot(2, e2, [10.0, 0.0, 0.0], true),
        ];
        let result = plan_route([0.0, 0.0, 0.0], 2, 6.0, 0.1, 10.0, &systems);
        assert!(result.is_some());
        let route = result.unwrap();
        assert_eq!(route.segments.len(), 2);
        assert!(matches!(route.segments[0], RouteSegment::FTL { to } if to == e1));
        assert!(matches!(route.segments[1], RouteSegment::FTL { to } if to == e2));
    }

    #[test]
    fn sublight_to_ftl_mixed_route() {
        let e0 = test_entity(0);
        let e1 = test_entity(1);
        let e2 = test_entity(2);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
            make_snapshot(1, e1, [5.0, 0.0, 0.0], false), // unsurveyed — can't FTL here
            make_snapshot(2, e2, [10.0, 0.0, 0.0], true),  // surveyed — FTL from e1
        ];
        let result = plan_route([0.0, 0.0, 0.0], 2, 6.0, 0.5, 10.0, &systems);
        assert!(result.is_some());
        let route = result.unwrap();
        assert_eq!(route.segments.len(), 2);
        // First hop: sublight to e1 (unsurveyed, can't FTL).
        assert!(matches!(route.segments[0], RouteSegment::SubLight { to_system: Some(sys), .. } if sys == e1));
        // Second hop: FTL to e2.
        assert!(matches!(route.segments[1], RouteSegment::FTL { to } if to == e2));
    }

    #[test]
    fn ftl_to_sublight_mixed_route() {
        let e0 = test_entity(0);
        let e1 = test_entity(1);
        let e2 = test_entity(2);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
            make_snapshot(1, e1, [5.0, 0.0, 0.0], true),
            make_snapshot(2, e2, [10.0, 0.0, 0.0], false), // unsurveyed — must sublight
        ];
        let result = plan_route([0.0, 0.0, 0.0], 2, 6.0, 0.5, 10.0, &systems);
        assert!(result.is_some());
        let route = result.unwrap();
        assert_eq!(route.segments.len(), 2);
        assert!(matches!(route.segments[0], RouteSegment::FTL { to } if to == e1));
        assert!(matches!(route.segments[1], RouteSegment::SubLight { to_system: Some(sys), .. } if sys == e2));
    }

    #[test]
    fn no_route_returns_none() {
        let e0 = test_entity(0);
        let e1 = test_entity(1);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
            make_snapshot(1, e1, [100.0, 0.0, 0.0], true),
        ];
        let result = plan_route([0.0, 0.0, 0.0], 1, 6.0, 0.5, 10.0, &systems);
        assert!(result.is_none());
    }

    #[test]
    fn already_at_destination() {
        let e0 = test_entity(0);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
        ];
        let result = plan_route([0.0, 0.0, 0.0], 0, 6.0, 0.5, 10.0, &systems);
        assert!(result.is_some());
        assert!(result.unwrap().segments.is_empty());
    }

    #[test]
    fn prefers_ftl_over_sublight_when_faster() {
        let e0 = test_entity(0);
        let e1 = test_entity(1);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
            make_snapshot(1, e1, [5.0, 0.0, 0.0], true),
        ];
        let result = plan_route([0.0, 0.0, 0.0], 1, 10.0, 0.1, 10.0, &systems);
        assert!(result.is_some());
        let route = result.unwrap();
        assert_eq!(route.segments.len(), 1);
        // Should pick FTL (cost = 5*60/10 = 30) over sublight (cost = 5*60/0.1 = 3000).
        assert!(matches!(route.segments[0], RouteSegment::FTL { .. }));
    }

    #[test]
    fn non_ftl_ship_uses_sublight_only() {
        let e0 = test_entity(0);
        let e1 = test_entity(1);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
            make_snapshot(1, e1, [5.0, 0.0, 0.0], true),
        ];
        let result = plan_route([0.0, 0.0, 0.0], 1, 0.0, 0.5, 10.0, &systems);
        assert!(result.is_some());
        let route = result.unwrap();
        assert_eq!(route.segments.len(), 1);
        assert!(matches!(route.segments[0], RouteSegment::SubLight { .. }));
    }

    #[test]
    fn unsurveyed_destination_uses_sublight() {
        let e0 = test_entity(0);
        let e1 = test_entity(1);
        let systems = vec![
            make_snapshot(0, e0, [0.0, 0.0, 0.0], true),
            make_snapshot(1, e1, [5.0, 0.0, 0.0], false), // unsurveyed
        ];
        let result = plan_route([0.0, 0.0, 0.0], 1, 10.0, 0.5, 10.0, &systems);
        assert!(result.is_some());
        let route = result.unwrap();
        assert_eq!(route.segments.len(), 1);
        assert!(matches!(route.segments[0], RouteSegment::SubLight { .. }));
    }
}
