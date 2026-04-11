use bevy::prelude::*;

use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::StarSystem;
use crate::physics::{distance_ly, distance_ly_arr, sublight_travel_hexadies};
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};

use super::{Ship, ShipState, INITIAL_FTL_SPEED_C};

/// Port FTL range bonus in light-years (#46)
pub const PORT_FTL_RANGE_BONUS_LY: f64 = 10.0;

/// Port FTL travel time reduction factor (#46): 20% reduction
pub const PORT_TRAVEL_TIME_FACTOR: f64 = 0.8;

// --- Sub-light travel ---

/// #45: Accepts optional sublight_speed_bonus from GlobalParams
pub fn start_sublight_travel(
    ship_state: &mut ShipState,
    ship_pos: &Position,
    ship: &Ship,
    destination: Position,
    target_system: Option<Entity>,
    current_time: i64,
) {
    start_sublight_travel_with_bonus(ship_state, ship_pos, ship, destination, target_system, current_time, 0.0);
}

pub fn start_sublight_travel_with_bonus(
    ship_state: &mut ShipState,
    ship_pos: &Position,
    ship: &Ship,
    destination: Position,
    target_system: Option<Entity>,
    current_time: i64,
    sublight_speed_bonus: f64,
) {
    let origin = ship_pos.as_array();
    let dest = destination.as_array();
    let dist = distance_ly_arr(origin, dest);
    let effective_speed = ship.sublight_speed + sublight_speed_bonus;
    let travel_time = sublight_travel_hexadies(dist, effective_speed);
    *ship_state = ShipState::SubLight {
        origin,
        destination: dest,
        target_system,
        departed_at: current_time,
        arrival_at: current_time + travel_time,
    };
}

pub fn sublight_movement_system(
    clock: Res<GameClock>,
    mut query: Query<(&mut ShipState, &mut Position, &Ship)>,
    systems: Query<&StarSystem, Without<Ship>>,
    mut events: MessageWriter<GameEvent>,
) {
    for (mut state, mut pos, ship) in query.iter_mut() {
        let (origin, destination, target_system, departed_at, arrival_at) = match *state {
            ShipState::SubLight {
                origin, destination, target_system, departed_at, arrival_at,
            } => (origin, destination, target_system, departed_at, arrival_at),
            _ => continue,
        };

        let total = (arrival_at - departed_at) as f64;
        if total <= 0.0 {
            pos.x = destination[0];
            pos.y = destination[1];
            pos.z = destination[2];
            if let Some(system) = target_system {
                *state = ShipState::Docked { system };
                let sys_name = systems.get(system).map(|s| s.name.clone()).unwrap_or_default();
                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::ShipArrived,
                    description: format!("{} arrived at {}", ship.name, sys_name),
                    related_system: Some(system),
                });
            }
            continue;
        }

        let progress = (clock.elapsed - departed_at) as f64 / total;

        if progress >= 1.0 {
            pos.x = destination[0];
            pos.y = destination[1];
            pos.z = destination[2];
            if let Some(system) = target_system {
                *state = ShipState::Docked { system };
                let sys_name = systems.get(system).map(|s| s.name.clone()).unwrap_or_default();
                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::ShipArrived,
                    description: format!("{} arrived at {}", ship.name, sys_name),
                    related_system: Some(system),
                });
            }
        } else {
            pos.x = origin[0] + (destination[0] - origin[0]) * progress;
            pos.y = origin[1] + (destination[1] - origin[1]) * progress;
            pos.z = origin[2] + (destination[2] - origin[2]) * progress;
        }
    }
}

// --- FTL travel ---

/// #45: Accepts optional ftl_range_bonus and ftl_speed_multiplier from GlobalParams
pub fn start_ftl_travel(
    ship_state: &mut ShipState,
    ship: &Ship,
    origin_system: Entity,
    destination_system: Entity,
    origin_pos: &Position,
    dest_pos: &Position,
    current_time: i64,
) -> Result<(), &'static str> {
    start_ftl_travel_with_bonus(ship_state, ship, origin_system, destination_system, origin_pos, dest_pos, current_time, 0.0, 1.0, false)
}

pub fn start_ftl_travel_with_bonus(
    ship_state: &mut ShipState,
    ship: &Ship,
    origin_system: Entity,
    destination_system: Entity,
    origin_pos: &Position,
    dest_pos: &Position,
    current_time: i64,
    ftl_range_bonus: f64,
    ftl_speed_multiplier: f64,
    origin_has_port: bool,
) -> Result<(), &'static str> {
    if ship.ftl_range <= 0.0 {
        return Err("Ship has no FTL capability");
    }

    let port_range_bonus = if origin_has_port { PORT_FTL_RANGE_BONUS_LY } else { 0.0 };
    let effective_range = ship.ftl_range + ftl_range_bonus + port_range_bonus;
    let dist = distance_ly(origin_pos, dest_pos);
    if dist > effective_range {
        return Err("Destination is beyond FTL range");
    }

    let effective_ftl_speed = INITIAL_FTL_SPEED_C * ftl_speed_multiplier;
    let mut travel_hexadies = (dist * HEXADIES_PER_YEAR as f64 / effective_ftl_speed).ceil() as i64;
    if origin_has_port {
        travel_hexadies = (travel_hexadies as f64 * PORT_TRAVEL_TIME_FACTOR).ceil() as i64;
    }

    *ship_state = ShipState::InFTL {
        origin_system,
        destination_system,
        departed_at: current_time,
        arrival_at: current_time + travel_hexadies,
    };

    Ok(())
}

pub fn process_ftl_travel(
    clock: Res<GameClock>,
    mut ships: Query<(&Ship, &mut ShipState, &mut Position)>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
    mut events: MessageWriter<GameEvent>,
) {
    for (ship, mut state, mut ship_pos) in ships.iter_mut() {
        let (destination_system, arrival_at) = match *state {
            ShipState::InFTL { destination_system, arrival_at, .. } => {
                (destination_system, arrival_at)
            }
            _ => continue,
        };

        if clock.elapsed >= arrival_at {
            if let Ok((star, dest_pos)) = systems.get(destination_system) {
                *ship_pos = *dest_pos;
                *state = ShipState::Docked { system: destination_system };
                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::ShipArrived,
                    description: format!("{} arrived at {} via FTL", ship.name, star.name),
                    related_system: Some(destination_system),
                });
                info!("Ship {} arrived at {} via FTL", ship.name, star.name);
            } else {
                warn!("Ship {} FTL destination entity no longer exists", ship.name);
            }
        }
    }
}

// --- Auto-route planning (#49) ---

/// Plan an FTL route from a starting position to a destination system.
///
/// Uses a greedy algorithm: at each hop, pick the surveyed system within FTL range
/// that is closest to the final destination. Returns `None` if no route can be found.
///
/// The returned `Vec<Entity>` lists every system to jump to, in order, ending with
/// the destination itself.
pub fn plan_ftl_route(
    from_pos: [f64; 3],
    to: Entity,
    ftl_range: f64,
    systems: &Query<(Entity, &StarSystem, &Position), Without<Ship>>,
) -> Option<Vec<Entity>> {
    let Ok((_, dest_star, dest_pos)) = systems.get(to) else {
        return None;
    };

    // FTL requires destination to be surveyed
    if !dest_star.surveyed {
        return None;
    }

    let dest_arr = dest_pos.as_array();

    // Direct jump possible?
    if distance_ly_arr(from_pos, dest_arr) <= ftl_range {
        return Some(vec![to]);
    }

    let mut route: Vec<Entity> = Vec::new();
    let mut current_pos = from_pos;
    let mut visited: Vec<Entity> = Vec::new();
    let max_hops = 50; // safety valve

    for _ in 0..max_hops {
        // Among surveyed systems within range, pick the one closest to destination
        let mut best: Option<(Entity, [f64; 3], f64)> = None;

        for (entity, star, pos) in systems.iter() {
            if !star.surveyed {
                continue;
            }
            if visited.contains(&entity) {
                continue;
            }
            let pos_arr = pos.as_array();
            let dist_from_current = distance_ly_arr(current_pos, pos_arr);
            if dist_from_current > ftl_range || dist_from_current < 1e-9 {
                continue;
            }
            let dist_to_dest = distance_ly_arr(pos_arr, dest_arr);
            match &best {
                Some((_, _, best_dist)) if dist_to_dest >= *best_dist => {}
                _ => {
                    best = Some((entity, pos_arr, dist_to_dest));
                }
            }
        }

        let Some((best_entity, best_pos, best_dist)) = best else {
            return None; // stuck
        };

        route.push(best_entity);
        visited.push(best_entity);
        current_pos = best_pos;

        // Can we reach the final destination from here?
        if best_entity == to || best_dist <= ftl_range {
            if best_entity != to {
                route.push(to);
            }
            return Some(route);
        }
    }

    None // exceeded max hops
}
