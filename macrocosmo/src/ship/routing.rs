//! #128: Mixed multi-segment route planning with async computation.
//!
//! Provides an A* route planner that finds optimal mixed FTL/sublight routes,
//! runs as an async task off the main thread, and integrates with the ECS via
//! `PendingRoute` component and `poll_pending_routes` system.
//!
//! #187: Adds ROE-aware weighting so that `RulesOfEngagement::Retreat`
//! ships route around star systems known to contain hostile factions. The
//! avoidance uses the player's [`KnowledgeStore`](crate::knowledge::KnowledgeStore)
//! — unknown hostiles cannot be avoided (light-speed delayed info). If the
//! only available path passes through a hostile system, the planner still
//! returns it (penalty applies but doesn't make the edge infinite) to avoid
//! stuck ships.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::components::Position;
use crate::galaxy::region::RegionBlockSnapshot;
use crate::galaxy::{ForbiddenRegion, StarSystem};
use crate::physics::distance_ly_arr;
use crate::time_system::HEXADIES_PER_YEAR;

use super::{
    CommandQueue, PortParams, QueuedCommand, RulesOfEngagement, Ship, ShipState,
    start_sublight_travel_with_bonus,
};

/// Maximum sublight edge distance in light-years (caps edge count in A*).
pub const MAX_SUBLIGHT_EDGE_LY: f64 = 30.0;

/// #187: Cost multiplier applied when entering a system the ship's empire
/// *knows* is hostile (via KnowledgeStore snapshot, gated by
/// `FactionRelations::can_attack_aggressive`). Only active for
/// [`RulesOfEngagement::Retreat`]; base cost is already in time units (hexadies).
pub const HOSTILE_PENALTY_MULTIPLIER: f64 = 10.0;

/// #187: Snapshot of a star system for async route planning (no ECS references).
#[derive(Clone, Debug)]
pub struct RouteSystemSnapshot {
    pub index: usize,
    pub entity: Entity,
    pub pos: [f64; 3],
    pub surveyed: bool,
    /// `true` if this system is *known* to the ship's empire to host a hostile
    /// faction (KnowledgeStore snapshot + `FactionRelations::can_attack_aggressive`).
    /// Unknown hostiles are `false` — light-speed delay means the ship cannot
    /// avoid what it does not yet know about.
    pub hostile_known: bool,
}

/// A single segment of a planned route.
#[derive(Clone, Debug)]
pub enum RouteSegment {
    /// FTL jump to a star system.
    FTL { to: Entity },
    /// Sub-light travel to a position, optionally associated with a star system.
    SubLight {
        to_pos: [f64; 3],
        to_system: Option<Entity>,
    },
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
    /// #334: `CommandId` of the dispatched `MoveRequested` that spawned
    /// this async route. Threaded here so that when `poll_pending_routes`
    /// reaches a terminal state (Ok / Rejected) it can emit the matching
    /// [`crate::ship::command_events::CommandExecuted`] for `CommandLog`
    /// and future gamestate bridges to key by. `None` only for legacy
    /// in-flight ships that predate the dispatcher refactor (should
    /// never occur post-Phase 1, but kept optional for safety).
    pub command_id: Option<super::command_events::CommandId>,
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
        other
            .f_cost
            .partial_cmp(&self.f_cost)
            .unwrap_or(Ordering::Equal)
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
/// - #187: if `roe == Retreat`, entering a system with `hostile_known = true`
///   multiplies that edge's cost by [`HOSTILE_PENALTY_MULTIPLIER`]. The
///   destination system itself is exempt from the penalty (the player
///   explicitly asked to go there), so that `MoveTo` never becomes impossible.
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
    plan_route_full(
        origin_pos,
        destination_index,
        ftl_range,
        sublight_speed,
        ftl_speed,
        systems,
        // Default ROE = Defensive = no avoidance, behavioural parity with
        // pre-#187 callers that haven't been updated to pass an ROE.
        RulesOfEngagement::Defensive,
        &[],
    )
}

/// #187: ROE-aware variant of [`plan_route`]. For `RulesOfEngagement::Retreat`,
/// entering a *known* hostile system multiplies that edge's cost by
/// [`HOSTILE_PENALTY_MULTIPLIER`]. For all other ROEs this behaves identically
/// to [`plan_route`] (no penalty applied).
pub fn plan_route_with_roe(
    origin_pos: [f64; 3],
    destination_index: usize,
    ftl_range: f64,
    sublight_speed: f64,
    ftl_speed: f64,
    systems: &[RouteSystemSnapshot],
    roe: RulesOfEngagement,
) -> Option<PlannedRoute> {
    plan_route_full(
        origin_pos,
        destination_index,
        ftl_range,
        sublight_speed,
        ftl_speed,
        systems,
        roe,
        &[],
    )
}

/// #145: Full-featured variant. In addition to [`plan_route_with_roe`], takes
/// a list of FTL-blocking [`RegionBlockSnapshot`]s. FTL edges whose segment
/// intersects any blocking region's effective bounding spheres are dropped.
/// SubLight edges are unaffected (ships can still crawl through at sublight).
pub fn plan_route_full(
    origin_pos: [f64; 3],
    destination_index: usize,
    ftl_range: f64,
    sublight_speed: f64,
    ftl_speed: f64,
    systems: &[RouteSystemSnapshot],
    roe: RulesOfEngagement,
    ftl_blockers: &[RegionBlockSnapshot],
) -> Option<PlannedRoute> {
    if destination_index >= systems.len() {
        return None;
    }

    let dest_pos = systems[destination_index].pos;
    let n = systems.len();

    // Heuristic: straight-line distance / max_speed (admissible lower bound).
    let max_speed = if ftl_speed > 0.0 {
        ftl_speed
    } else {
        sublight_speed
    };
    if max_speed <= 0.0 {
        return None;
    }
    let heuristic = |pos: [f64; 3]| -> f64 {
        distance_ly_arr(pos, dest_pos) * HEXADIES_PER_YEAR as f64 / max_speed
    };

    // Find which system index the ship starts at (if any). We treat origin as
    // a virtual node with index `n` that has edges to reachable systems.
    let origin_at_system: Option<usize> = systems
        .iter()
        .position(|s| distance_ly_arr(origin_pos, s.pos) < 1e-9);

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
                // #145: reject FTL edges that cross a forbidden region's
                // bounding sphere union (sublight edges still permitted).
                let blocked_by_region = ftl_blockers
                    .iter()
                    .any(|b| b.blocks_segment(current_pos, target.pos));
                if !blocked_by_region {
                    let cost = dist * HEXADIES_PER_YEAR as f64 / ftl_speed;
                    edges.push((EdgeKind::FTL, cost));
                }
            }

            // SubLight edge: to any system within MAX_SUBLIGHT_EDGE_LY.
            if sublight_speed > 0.0 && dist <= MAX_SUBLIGHT_EDGE_LY {
                let cost = dist * HEXADIES_PER_YEAR as f64 / sublight_speed;
                edges.push((EdgeKind::SubLight, cost));
            }

            // #187: Retreat ships apply a hostile penalty when entering a
            // known-hostile system. The destination is exempted — the player
            // explicitly requested it — so MoveTo into a hostile system still
            // succeeds.
            let apply_hostile_penalty =
                roe == RulesOfEngagement::Retreat && target.hostile_known && j != destination_index;

            for (kind, base_cost) in edges {
                let cost = if apply_hostile_penalty {
                    base_cost * HOSTILE_PENALTY_MULTIPLIER
                } else {
                    base_cost
                };
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
///
/// #187: Populates `hostile_known` using the player empire's
/// [`KnowledgeStore`](crate::knowledge::KnowledgeStore) and
/// [`FactionRelations`](crate::faction::FactionRelations). A system is flagged
/// hostile iff:
/// 1. The empire has a knowledge snapshot showing `has_hostile == true`, AND
/// 2. The empire's view of that hostile faction satisfies
///    `can_attack_aggressive()` (so Peace/Alliance factions are *not* avoided).
///
/// `hostile_faction_map` supplies the faction entity of the hostile garrisoning
/// each system (derived from `(AtSystem, FactionOwner, With<Hostile>)` — #293).
pub fn collect_route_snapshots(
    systems: &Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    knowledge: Option<&crate::knowledge::KnowledgeStore>,
    relations: &crate::faction::FactionRelations,
    ship_faction: Option<Entity>,
    hostile_faction_map: &std::collections::HashMap<Entity, Entity>,
) -> Vec<RouteSystemSnapshot> {
    systems
        .iter()
        .enumerate()
        .map(|(i, (entity, star, pos))| {
            // Resolve whether the player's empire *knows* this system is hostile,
            // and whether the ship's faction actually treats that hostile as
            // an enemy. Unknown / un-owned hostiles are ignored (light-speed delay).
            let hostile_known = match (ship_faction, knowledge) {
                (Some(from), Some(store)) => {
                    store
                        .get(entity)
                        .map(|k| k.data.has_hostile)
                        .unwrap_or(false)
                        && hostile_faction_map
                            .get(&entity)
                            .map(|&hostile| {
                                relations
                                    .get_or_default(from, hostile)
                                    .can_attack_aggressive()
                            })
                            .unwrap_or(false)
                }
                _ => false,
            };
            RouteSystemSnapshot {
                index: i,
                entity,
                pos: pos.as_array(),
                surveyed: star.surveyed,
                hostile_known,
            }
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
    // #187: ROE-less spawner retained for backward compatibility and tests.
    // Defaults to `Defensive` (no avoidance).
    spawn_route_task_with_roe(
        origin_pos,
        destination,
        ftl_range,
        sublight_speed,
        ftl_speed,
        systems,
        RulesOfEngagement::Defensive,
    )
}

/// #187: ROE-aware spawner. `RulesOfEngagement::Retreat` ships avoid
/// `hostile_known` systems; other ROEs behave identically to [`spawn_route_task`].
pub fn spawn_route_task_with_roe(
    origin_pos: [f64; 3],
    destination: Entity,
    ftl_range: f64,
    sublight_speed: f64,
    ftl_speed: f64,
    systems: Vec<RouteSystemSnapshot>,
    roe: RulesOfEngagement,
) -> Task<Option<PlannedRoute>> {
    spawn_route_task_full(
        origin_pos,
        destination,
        ftl_range,
        sublight_speed,
        ftl_speed,
        systems,
        roe,
        Vec::new(),
    )
}

/// #145: Full-featured async spawner. Accepts `ftl_blockers` as captured
/// `RegionBlockSnapshot` for FTL edge rejection. Equivalent to
/// [`spawn_route_task_with_roe`] when `ftl_blockers` is empty.
pub fn spawn_route_task_full(
    origin_pos: [f64; 3],
    destination: Entity,
    ftl_range: f64,
    sublight_speed: f64,
    ftl_speed: f64,
    systems: Vec<RouteSystemSnapshot>,
    roe: RulesOfEngagement,
    ftl_blockers: Vec<RegionBlockSnapshot>,
) -> Task<Option<PlannedRoute>> {
    let pool = AsyncComputeTaskPool::get();
    let dest_index = systems.iter().position(|s| s.entity == destination);
    pool.spawn(async move {
        let dest_idx = dest_index?;
        plan_route_full(
            origin_pos,
            dest_idx,
            ftl_range,
            sublight_speed,
            ftl_speed,
            &systems,
            roe,
            &ftl_blockers,
        )
    })
}

/// #145: Build a list of [`RegionBlockSnapshot`] from all forbidden regions
/// that carry the `blocks_ftl` capability. Pure ECS-to-snapshot conversion;
/// safe to call from sync systems and hand off to async tasks.
pub fn collect_ftl_blockers(regions: &Query<&ForbiddenRegion>) -> Vec<RegionBlockSnapshot> {
    regions
        .iter()
        .filter(|r| r.has_capability("blocks_ftl"))
        .map(RegionBlockSnapshot::from_region)
        .collect()
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
    balance: Res<crate::technology::GameBalance>,
    mut ships: Query<
        (Entity, &Ship, &mut ShipState, &mut CommandQueue, &Position),
        (With<PendingRoute>, Without<crate::colony::SlotAssignment>),
    >,
    mut pending_q: Query<&mut PendingRoute>,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    routing_station_ships: Query<(Entity, &Ship, &ShipState, &crate::colony::SlotAssignment)>,
    mut pending_count: ResMut<RouteCalculationsPending>,
    building_registry: Res<crate::colony::BuildingRegistry>,
    // #334 Phase 1: emit the terminal CommandExecuted for the MoveRequested
    // that spawned this async route. `CommandId` is threaded via
    // `PendingRoute.command_id`; `None` is tolerated for in-flight ships
    // that predate the refactor (emission is skipped in that case).
    mut executed: MessageWriter<super::command_events::CommandExecuted>,
) {
    use super::command_events::{CommandExecuted, CommandKind, CommandResult};
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };
    let base_ftl_speed = balance.initial_ftl_speed_c();

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
        // #334: preserve the CommandId for terminal `CommandExecuted` below.
        let maybe_cmd_id = pending.command_id;

        // Remove PendingRoute component and decrement counter.
        commands.entity(ship_entity).remove::<PendingRoute>();
        pending_count.count = pending_count.count.saturating_sub(1);

        let Ok((_, ship, mut state, mut queue, ship_pos)) = ships.get_mut(ship_entity) else {
            // Ship despawned while waiting — emit Rejected.
            if let Some(cid) = maybe_cmd_id {
                executed.write(CommandExecuted {
                    command_id: cid,
                    kind: CommandKind::Move,
                    ship: ship_entity,
                    result: CommandResult::Rejected {
                        reason: "ship despawned".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
            }
            continue;
        };

        // Ensure ship is still docked (may have changed state while waiting).
        let ShipState::InSystem {
            system: docked_system,
        } = *state
        else {
            // Ship moved; discard the route, remove the MoveTo from queue head if still there.
            if matches!(queue.commands.first(), Some(QueuedCommand::MoveTo { system }) if *system == target_system)
            {
                queue.commands.remove(0);
            }
            queue.sync_prediction(ship_pos.as_array(), None);
            if let Some(cid) = maybe_cmd_id {
                executed.write(CommandExecuted {
                    command_id: cid,
                    kind: CommandKind::Move,
                    ship: ship_entity,
                    result: CommandResult::Rejected {
                        reason: "ship no longer docked".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
            }
            continue;
        };

        // Consume the MoveTo from the queue head.
        if matches!(queue.commands.first(), Some(QueuedCommand::MoveTo { system }) if *system == target_system)
        {
            queue.commands.remove(0);
        }

        let Some(route) = route_option else {
            // No route found — fall back to direct sublight.
            let mut fallback_ok = false;
            if let Ok((_, _target_star, target_pos)) = systems.get(target_system) {
                if let Ok((_, _, origin_pos)) = systems.get(docked_system) {
                    if let Err(e) = start_sublight_travel_with_bonus(
                        &mut state,
                        origin_pos,
                        ship,
                        *target_pos,
                        Some(target_system),
                        clock.elapsed,
                        global_params.sublight_speed_bonus,
                    ) {
                        // #296: immobile ships never reach the planner (UI guard
                        // + command_queue), but defend-in-depth keeps the system
                        // tolerant.
                        warn!(
                            "Route planner: sublight fallback failed for {}: {}",
                            ship.name, e
                        );
                    } else {
                        info!(
                            "Route planner: no route found for {}, falling back to sublight",
                            ship.name
                        );
                        fallback_ok = true;
                    }
                }
            }
            queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
            if let Some(cid) = maybe_cmd_id {
                executed.write(CommandExecuted {
                    command_id: cid,
                    kind: CommandKind::Move,
                    ship: ship_entity,
                    result: if fallback_ok {
                        CommandResult::Ok
                    } else {
                        CommandResult::Rejected {
                            reason: "no route + sublight fallback failed".to_string(),
                        }
                    },
                    completed_at: clock.elapsed,
                });
            }
            continue;
        };

        if route.segments.is_empty() {
            // Already at destination.
            queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
            if let Some(cid) = maybe_cmd_id {
                executed.write(CommandExecuted {
                    command_id: cid,
                    kind: CommandKind::Move,
                    ship: ship_entity,
                    result: CommandResult::Ok,
                    completed_at: clock.elapsed,
                });
            }
            continue;
        }

        // Execute first segment, push remaining as MoveTo commands.
        let first = &route.segments[0];
        let remaining = &route.segments[1..];

        // Prepend remaining segments as MoveTo commands (in reverse order to maintain order).
        for seg in remaining.iter().rev() {
            match seg {
                RouteSegment::FTL { to } => {
                    queue
                        .commands
                        .insert(0, QueuedCommand::MoveTo { system: *to });
                }
                RouteSegment::SubLight {
                    to_system: Some(sys),
                    ..
                } => {
                    queue
                        .commands
                        .insert(0, QueuedCommand::MoveTo { system: *sys });
                }
                RouteSegment::SubLight {
                    to_pos,
                    to_system: None,
                } => {
                    // Sublight to a non-system position — unusual, but handle gracefully.
                    // For now this shouldn't happen since all snapshots are systems.
                    warn!(
                        "Route segment to non-system position {:?}, skipping",
                        to_pos
                    );
                }
            }
        }

        // Execute the first segment. On success emit `Ok`; on failure
        // (both FTL and sublight-fallback fail, or sublight segment fails)
        // emit `Rejected`.
        let mut segment_ok = false;
        let mut segment_reason = String::new();
        match first {
            RouteSegment::FTL { to } => {
                let Ok((_, first_star, first_pos)) = systems.get(*to) else {
                    warn!(
                        "Route planner: FTL target no longer exists for {}",
                        ship.name
                    );
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    if let Some(cid) = maybe_cmd_id {
                        executed.write(CommandExecuted {
                            command_id: cid,
                            kind: CommandKind::Move,
                            ship: ship_entity,
                            result: CommandResult::Rejected {
                                reason: "FTL target despawned".to_string(),
                            },
                            completed_at: clock.elapsed,
                        });
                    }
                    continue;
                };
                let Ok((_, _, origin_pos)) = systems.get(docked_system) else {
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    if let Some(cid) = maybe_cmd_id {
                        executed.write(CommandExecuted {
                            command_id: cid,
                            kind: CommandKind::Move,
                            ship: ship_entity,
                            result: CommandResult::Rejected {
                                reason: "origin system lost".to_string(),
                            },
                            completed_at: clock.elapsed,
                        });
                    }
                    continue;
                };
                let port_params = PortParams::from_station_ships(docked_system, &routing_station_ships, &building_registry);
                match crate::ship::movement::start_ftl_travel_full(
                    &mut state,
                    ship,
                    docked_system,
                    *to,
                    origin_pos,
                    first_pos,
                    clock.elapsed,
                    global_params.ftl_range_bonus,
                    global_params.ftl_speed_multiplier,
                    port_params,
                    base_ftl_speed,
                ) {
                    Ok(()) => {
                        info!(
                            "Route planner: {} FTL jumping to {} ({} segments remaining)",
                            ship.name,
                            first_star.name,
                            remaining.len()
                        );
                        segment_ok = true;
                    }
                    Err(e) => {
                        warn!(
                            "Route planner: FTL hop failed for {}: {}, falling back to sublight",
                            ship.name, e
                        );
                        // Fall back to sublight for this segment.
                        if let Err(e2) = start_sublight_travel_with_bonus(
                            &mut state,
                            origin_pos,
                            ship,
                            *first_pos,
                            Some(*to),
                            clock.elapsed,
                            global_params.sublight_speed_bonus,
                        ) {
                            warn!(
                                "Route planner: sublight fallback also failed for {}: {}",
                                ship.name, e2
                            );
                            segment_reason = format!("FTL + sublight both failed: {}", e2);
                        } else {
                            segment_ok = true;
                        }
                    }
                }
            }
            RouteSegment::SubLight { to_pos, to_system } => {
                let Ok((_, _, origin_pos)) = systems.get(docked_system) else {
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    if let Some(cid) = maybe_cmd_id {
                        executed.write(CommandExecuted {
                            command_id: cid,
                            kind: CommandKind::Move,
                            ship: ship_entity,
                            result: CommandResult::Rejected {
                                reason: "origin system lost".to_string(),
                            },
                            completed_at: clock.elapsed,
                        });
                    }
                    continue;
                };
                let dest_pos = Position::from(*to_pos);
                if let Err(e) = start_sublight_travel_with_bonus(
                    &mut state,
                    origin_pos,
                    ship,
                    dest_pos,
                    *to_system,
                    clock.elapsed,
                    global_params.sublight_speed_bonus,
                ) {
                    warn!(
                        "Route planner: sublight segment rejected for {}: {}",
                        ship.name, e
                    );
                    segment_reason = format!("sublight segment rejected: {}", e);
                } else {
                    info!(
                        "Route planner: {} sublight to {:?} ({} segments remaining)",
                        ship.name,
                        to_system,
                        remaining.len()
                    );
                    segment_ok = true;
                }
            }
        }

        // Emit terminal CommandExecuted keyed by the original MoveRequested.
        if let Some(cid) = maybe_cmd_id {
            executed.write(CommandExecuted {
                command_id: cid,
                kind: CommandKind::Move,
                ship: ship_entity,
                result: if segment_ok {
                    CommandResult::Ok
                } else {
                    CommandResult::Rejected {
                        reason: segment_reason,
                    }
                },
                completed_at: clock.elapsed,
            });
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

    fn make_snapshot(
        index: usize,
        entity: Entity,
        pos: [f64; 3],
        surveyed: bool,
    ) -> RouteSystemSnapshot {
        RouteSystemSnapshot {
            index,
            entity,
            pos,
            surveyed,
            hostile_known: false,
        }
    }

    fn make_snapshot_hostile(
        index: usize,
        entity: Entity,
        pos: [f64; 3],
        surveyed: bool,
        hostile_known: bool,
    ) -> RouteSystemSnapshot {
        RouteSystemSnapshot {
            index,
            entity,
            pos,
            surveyed,
            hostile_known,
        }
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
            make_snapshot(2, e2, [10.0, 0.0, 0.0], true), // surveyed — FTL from e1
        ];
        let result = plan_route([0.0, 0.0, 0.0], 2, 6.0, 0.5, 10.0, &systems);
        assert!(result.is_some());
        let route = result.unwrap();
        assert_eq!(route.segments.len(), 2);
        // First hop: sublight to e1 (unsurveyed, can't FTL).
        assert!(
            matches!(route.segments[0], RouteSegment::SubLight { to_system: Some(sys), .. } if sys == e1)
        );
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
        assert!(
            matches!(route.segments[1], RouteSegment::SubLight { to_system: Some(sys), .. } if sys == e2)
        );
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
        let systems = vec![make_snapshot(0, e0, [0.0, 0.0, 0.0], true)];
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
