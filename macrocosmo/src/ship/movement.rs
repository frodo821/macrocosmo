use bevy::prelude::*;

use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::StarSystem;
use crate::knowledge::{
    record_world_event_fact, FactSysParam, KnowledgeFact, PlayerVantage,
};
use crate::physics::{distance_ly, distance_ly_arr, sublight_travel_hexadies};
use crate::player::{AboardShip, Player, StationedAt};
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};

use super::{Ship, ShipState, INITIAL_FTL_SPEED_C};

/// Default port FTL range bonus in light-years (#46).
/// Used as fallback when BuildingRegistry is unavailable; canonical values live in Lua.
/// #160: canonical value is `GameBalance.port_ftl_range_bonus`.
pub const DEFAULT_PORT_FTL_RANGE_BONUS_LY: f64 = 10.0;

/// Default port FTL travel time reduction factor (#46): 20% reduction.
/// Used as fallback when BuildingRegistry is unavailable; canonical values live in Lua.
/// #160: canonical value is `GameBalance.port_travel_time_factor`.
pub const DEFAULT_PORT_TRAVEL_TIME_FACTOR: f64 = 0.8;

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

#[allow(clippy::too_many_arguments)]
pub fn sublight_movement_system(
    clock: Res<GameClock>,
    mut query: Query<(&mut ShipState, &mut Position, &Ship)>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
    mut events: MessageWriter<GameEvent>,
    player_q: Query<&StationedAt, Without<Ship>>,
    player_aboard_q: Query<&AboardShip, With<Player>>,
    mut fact_sys: FactSysParam,
) {
    let player_system = player_q.iter().next().map(|s| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| systems.get(s).ok())
        .map(|(_, p)| p.as_array());
    let player_aboard = player_aboard_q.iter().next().is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });
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
            write_ship_arrived_dual(
                target_system,
                ship,
                destination,
                clock.elapsed,
                &mut events,
                vantage.as_ref(),
                &systems,
                &mut fact_sys,
            );
            if let Some(system) = target_system {
                *state = ShipState::Docked { system };
            } else {
                *state = ShipState::Loitering { position: destination };
            }
            continue;
        }

        let progress = (clock.elapsed - departed_at) as f64 / total;

        if progress >= 1.0 {
            pos.x = destination[0];
            pos.y = destination[1];
            pos.z = destination[2];
            write_ship_arrived_dual(
                target_system,
                ship,
                destination,
                clock.elapsed,
                &mut events,
                vantage.as_ref(),
                &systems,
                &mut fact_sys,
            );
            if let Some(system) = target_system {
                *state = ShipState::Docked { system };
            } else {
                *state = ShipState::Loitering { position: destination };
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
    start_ftl_travel_with_bonus(ship_state, ship, origin_system, destination_system, origin_pos, dest_pos, current_time, 0.0, 1.0, PortParams::NONE)
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
    port_params: PortParams,
) -> Result<(), &'static str> {
    start_ftl_travel_full(
        ship_state, ship, origin_system, destination_system, origin_pos, dest_pos,
        current_time, ftl_range_bonus, ftl_speed_multiplier, port_params,
        INITIAL_FTL_SPEED_C,
    )
}

/// #160: Full FTL start-travel with an explicit base FTL speed (in units of c).
/// `base_ftl_speed_c` comes from `GameBalance.initial_ftl_speed_c()`.
#[allow(clippy::too_many_arguments)]
pub fn start_ftl_travel_full(
    ship_state: &mut ShipState,
    ship: &Ship,
    origin_system: Entity,
    destination_system: Entity,
    origin_pos: &Position,
    dest_pos: &Position,
    current_time: i64,
    ftl_range_bonus: f64,
    ftl_speed_multiplier: f64,
    port_params: PortParams,
    base_ftl_speed_c: f64,
) -> Result<(), &'static str> {
    if ship.ftl_range <= 0.0 {
        return Err("Ship has no FTL capability");
    }

    let effective_range = ship.ftl_range + ftl_range_bonus + port_params.ftl_range_bonus;
    let dist = distance_ly(origin_pos, dest_pos);
    if dist > effective_range {
        return Err("Destination is beyond FTL range");
    }

    let effective_ftl_speed = base_ftl_speed_c * ftl_speed_multiplier;
    let mut travel_hexadies = (dist * HEXADIES_PER_YEAR as f64 / effective_ftl_speed).ceil() as i64;
    if port_params.has_port {
        travel_hexadies = (travel_hexadies as f64 * port_params.travel_time_factor).ceil() as i64;
    }

    *ship_state = ShipState::InFTL {
        origin_system,
        destination_system,
        departed_at: current_time,
        arrival_at: current_time + travel_hexadies,
    };

    Ok(())
}

/// Port facility parameters extracted from building capabilities.
/// Encapsulates all port-related bonuses for FTL travel.
#[derive(Clone, Copy, Debug)]
pub struct PortParams {
    pub has_port: bool,
    pub ftl_range_bonus: f64,
    pub travel_time_factor: f64,
}

impl PortParams {
    /// No port — zero bonuses.
    pub const NONE: PortParams = PortParams {
        has_port: false,
        ftl_range_bonus: 0.0,
        travel_time_factor: 1.0,
    };

    /// Create PortParams from SystemBuildings and BuildingRegistry.
    pub fn from_system_buildings(
        sb: &crate::colony::SystemBuildings,
        registry: &crate::scripting::building_api::BuildingRegistry,
    ) -> Self {
        if sb.has_port(registry) {
            PortParams {
                has_port: true,
                ftl_range_bonus: sb.port_ftl_range_bonus(registry),
                travel_time_factor: sb.port_travel_time_factor(registry),
            }
        } else {
            Self::NONE
        }
    }

    /// Create PortParams from a boolean (legacy compatibility, uses default constants).
    pub fn from_bool(origin_has_port: bool) -> Self {
        if origin_has_port {
            PortParams {
                has_port: true,
                ftl_range_bonus: DEFAULT_PORT_FTL_RANGE_BONUS_LY,
                travel_time_factor: DEFAULT_PORT_TRAVEL_TIME_FACTOR,
            }
        } else {
            Self::NONE
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn process_ftl_travel(
    clock: Res<GameClock>,
    mut ships: Query<(&Ship, &mut ShipState, &mut Position)>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
    mut events: MessageWriter<GameEvent>,
    player_q: Query<&StationedAt, Without<Ship>>,
    player_aboard_q: Query<&AboardShip, With<Player>>,
    mut fact_sys: FactSysParam,
) {
    let player_system = player_q.iter().next().map(|s| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| systems.get(s).ok())
        .map(|(_, p)| p.as_array());
    let player_aboard = player_aboard_q.iter().next().is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });

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
                // #249: Dual-write FTL arrival.
                let event_id = fact_sys.allocate_event_id();
                let desc = format!("{} arrived at {} via FTL", ship.name, star.name);
                events.write(GameEvent {
                    id: event_id,
                    timestamp: clock.elapsed,
                    kind: GameEventKind::ShipArrived,
                    description: desc.clone(),
                    related_system: Some(destination_system),
                });
                if let Some(v) = vantage {
                    let comms = fact_sys
                        .empire_comms
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_default();
                    let relays = fact_sys.relay_network.relays.clone();
                    let fact = KnowledgeFact::ShipArrived {
                        event_id: Some(event_id),
                        system: Some(destination_system),
                        name: ship.name.clone(),
                        detail: desc,
                    };
                    record_world_event_fact(
                        fact,
                        dest_pos.as_array(),
                        clock.elapsed,
                        &v,
                        &mut fact_sys.fact_queue,
                        &mut fact_sys.notifications,
                        &mut fact_sys.notified_ids,
                        &relays,
                        &comms,
                    );
                }
                info!("Ship {} arrived at {} via FTL", ship.name, star.name);
            } else {
                warn!("Ship {} FTL destination entity no longer exists", ship.name);
            }
        }
    }
}

/// #249 helper — shared dual-write for sublight `ShipArrived` events.
#[allow(clippy::too_many_arguments)]
fn write_ship_arrived_dual(
    target_system: Option<Entity>,
    ship: &Ship,
    destination: [f64; 3],
    now: i64,
    events: &mut MessageWriter<GameEvent>,
    vantage: Option<&PlayerVantage>,
    systems: &Query<(&StarSystem, &Position), Without<Ship>>,
    fact_sys: &mut FactSysParam,
) {
    let (event_id, desc, related, origin_pos) = if let Some(system) = target_system {
        let (sys_name, sys_pos) = systems
            .get(system)
            .map(|(s, p)| (s.name.clone(), p.as_array()))
            .unwrap_or_default();
        let eid = fact_sys.allocate_event_id();
        let d = format!("{} arrived at {}", ship.name, sys_name);
        (eid, d, Some(system), sys_pos)
    } else {
        let eid = fact_sys.allocate_event_id();
        let d = format!(
            "{} arrived at deep-space coordinates ({:.2}, {:.2}, {:.2})",
            ship.name, destination[0], destination[1], destination[2]
        );
        (eid, d, None, destination)
    };

    events.write(GameEvent {
        id: event_id,
        timestamp: now,
        kind: GameEventKind::ShipArrived,
        description: desc.clone(),
        related_system: related,
    });

    if let Some(v) = vantage {
        let comms = fact_sys
            .empire_comms
            .iter()
            .next()
            .cloned()
            .unwrap_or_default();
        let relays = fact_sys.relay_network.relays.clone();
        let fact = KnowledgeFact::ShipArrived {
            event_id: Some(event_id),
            system: related,
            name: ship.name.clone(),
            detail: desc,
        };
        record_world_event_fact(
            fact,
            origin_pos,
            now,
            v,
            &mut fact_sys.fact_queue,
            &mut fact_sys.notifications,
            &mut fact_sys.notified_ids,
            &relays,
            &comms,
        );
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
