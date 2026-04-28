use bevy::prelude::*;

use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::StarSystem;
use crate::knowledge::{FactSysParam, FactionVantage, FactionVantageQueries, KnowledgeFact};
use crate::physics::{distance_ly, distance_ly_arr, sublight_travel_hexadies};
use crate::player::{AboardShip, Player, StationedAt};
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};

use super::{INITIAL_FTL_SPEED_C, Ship, ShipState};

/// Default port FTL range bonus in light-years (#46).
/// Used as fallback when BuildingRegistry is unavailable; canonical values live in Lua.
/// #160: canonical value is `GameBalance.port_ftl_range_bonus`.
pub const DEFAULT_PORT_FTL_RANGE_BONUS_LY: f64 = 10.0;

/// Default port FTL travel time reduction factor (#46): 20% reduction.
/// Used as fallback when BuildingRegistry is unavailable; canonical values live in Lua.
/// #160: canonical value is `GameBalance.port_travel_time_factor`.
pub const DEFAULT_PORT_TRAVEL_TIME_FACTOR: f64 = 0.8;

// --- Sub-light travel ---

/// #45: Accepts optional sublight_speed_bonus from GlobalParams.
///
/// #296: Returns `Err("ship is immobile")` when the ship's design confers no
/// propulsion (both `sublight_speed` and `ftl_range` non-positive — see
/// [`Ship::is_immobile`]). On success the ship transitions to
/// [`ShipState::SubLight`]; on error `ship_state` is left unchanged.
pub fn start_sublight_travel(
    ship_state: &mut ShipState,
    ship_pos: &Position,
    ship: &Ship,
    destination: Position,
    target_system: Option<Entity>,
    current_time: i64,
) -> Result<(), &'static str> {
    start_sublight_travel_with_bonus(
        ship_state,
        ship_pos,
        ship,
        destination,
        target_system,
        current_time,
        0.0,
    )
}

pub fn start_sublight_travel_with_bonus(
    ship_state: &mut ShipState,
    ship_pos: &Position,
    ship: &Ship,
    destination: Position,
    target_system: Option<Entity>,
    current_time: i64,
    sublight_speed_bonus: f64,
) -> Result<(), &'static str> {
    // #296 (S-3): Gate immobile ships BEFORE writing SubLight state — otherwise
    // sublight_travel_hexadies(dist, 0.0) would divide by zero and stall the
    // ship mid-transit with no way to recover. Bonuses from global params
    // cannot rescue a hull that has no propulsion at all.
    if ship.is_immobile() {
        return Err("ship is immobile");
    }
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
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn sublight_movement_system(
    clock: Res<GameClock>,
    mut query: Query<(
        Entity,
        &mut ShipState,
        &mut Position,
        &Ship,
        Option<&mut super::transit_events::LastDockedSystem>,
    )>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
    mut events: MessageWriter<GameEvent>,
    mut fact_sys: FactSysParam,
    mut event_system: ResMut<crate::event_system::EventSystem>,
    // Round 9 PR #1 Step 3: per-faction routing.
    vantage_q: FactionVantageQueries,
) {
    let vantages = vantage_q.collect();
    for (ship_entity, mut state, mut pos, ship, mut last_docked) in query.iter_mut() {
        let (origin, destination, target_system, departed_at, arrival_at) = match *state {
            ShipState::SubLight {
                origin,
                destination,
                target_system,
                departed_at,
                arrival_at,
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
                ship_entity,
                ship,
                destination,
                clock.elapsed,
                &mut events,
                &vantages,
                &systems,
                &mut fact_sys,
            );
            if let Some(system) = target_system {
                *state = ShipState::InSystem { system };
                // #291: fire fleet_system_entered for sublight arrival.
                if let Some(fleet) = ship.fleet {
                    super::transit_events::fire_fleet_transit(
                        &mut event_system,
                        super::transit_events::TransitDirection::Entered,
                        clock.elapsed,
                        super::transit_events::TransitMode::Sublight,
                        system,
                        fleet,
                    );
                }
                if let Some(ref mut lds) = last_docked {
                    lds.0 = Some(system);
                }
            } else {
                *state = ShipState::Loitering {
                    position: destination,
                };
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
                ship_entity,
                ship,
                destination,
                clock.elapsed,
                &mut events,
                &vantages,
                &systems,
                &mut fact_sys,
            );
            if let Some(system) = target_system {
                *state = ShipState::InSystem { system };
                // #291: fire fleet_system_entered for sublight arrival.
                if let Some(fleet) = ship.fleet {
                    super::transit_events::fire_fleet_transit(
                        &mut event_system,
                        super::transit_events::TransitDirection::Entered,
                        clock.elapsed,
                        super::transit_events::TransitMode::Sublight,
                        system,
                        fleet,
                    );
                }
                if let Some(ref mut lds) = last_docked {
                    lds.0 = Some(system);
                }
            } else {
                *state = ShipState::Loitering {
                    position: destination,
                };
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
    start_ftl_travel_with_bonus(
        ship_state,
        ship,
        origin_system,
        destination_system,
        origin_pos,
        dest_pos,
        current_time,
        0.0,
        1.0,
        PortParams::NONE,
    )
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
        ship_state,
        ship,
        origin_system,
        destination_system,
        origin_pos,
        dest_pos,
        current_time,
        ftl_range_bonus,
        ftl_speed_multiplier,
        port_params,
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

    /// Create PortParams from the pre-computed `SystemModifiers` on a star system.
    pub fn from_system_modifiers(sys_mods: &crate::galaxy::SystemModifiers) -> Self {
        use crate::amount::Amt;
        if sys_mods.port_repair.value().final_value() > Amt::ZERO {
            PortParams {
                has_port: true,
                ftl_range_bonus: sys_mods.port_ftl_range_bonus.value().final_value().to_f64(),
                travel_time_factor: sys_mods
                    .port_travel_time_factor
                    .value()
                    .final_value()
                    .to_f64(),
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
    mut ships: Query<(
        Entity,
        &Ship,
        &mut ShipState,
        &mut Position,
        Option<&mut super::transit_events::LastDockedSystem>,
    )>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
    mut events: MessageWriter<GameEvent>,
    mut fact_sys: FactSysParam,
    mut event_system: ResMut<crate::event_system::EventSystem>,
    // Round 9 PR #1 Step 3: per-faction routing.
    vantage_q: FactionVantageQueries,
) {
    let vantages = vantage_q.collect();

    for (ship_entity, ship, mut state, mut ship_pos, last_docked) in ships.iter_mut() {
        let (destination_system, arrival_at) = match *state {
            ShipState::InFTL {
                destination_system,
                arrival_at,
                ..
            } => (destination_system, arrival_at),
            _ => continue,
        };

        if clock.elapsed >= arrival_at {
            if let Ok((star, dest_pos)) = systems.get(destination_system) {
                *ship_pos = *dest_pos;
                *state = ShipState::InSystem {
                    system: destination_system,
                };
                // #291: fire fleet_system_entered for FTL arrival.
                if let Some(fleet) = ship.fleet {
                    super::transit_events::fire_fleet_transit(
                        &mut event_system,
                        super::transit_events::TransitDirection::Entered,
                        clock.elapsed,
                        super::transit_events::TransitMode::Ftl,
                        destination_system,
                        fleet,
                    );
                }
                if let Some(mut lds) = last_docked {
                    lds.0 = Some(destination_system);
                }
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
                let fact = KnowledgeFact::ShipArrived {
                    event_id: Some(event_id),
                    system: Some(destination_system),
                    name: ship.name.clone(),
                    detail: desc,
                    ship: ship_entity,
                };
                fact_sys.record_for(fact, &vantages, dest_pos.as_array(), clock.elapsed);
                info!("Ship {} arrived at {} via FTL", ship.name, star.name);
            } else {
                warn!("Ship {} FTL destination entity no longer exists", ship.name);
            }
        }
    }
}

/// #249 helper — shared dual-write for sublight `ShipArrived` events.
///
/// Round 9 PR #1 Step 3: `vantages` replaces the legacy
/// `Option<&PlayerVantage>` — `record_for` is a no-op on an empty
/// slice, matching the previous "no player vantage" branch.
#[allow(clippy::too_many_arguments)]
fn write_ship_arrived_dual(
    target_system: Option<Entity>,
    ship_entity: Entity,
    ship: &Ship,
    destination: [f64; 3],
    now: i64,
    events: &mut MessageWriter<GameEvent>,
    vantages: &[FactionVantage],
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

    let fact = KnowledgeFact::ShipArrived {
        event_id: Some(event_id),
        system: related,
        name: ship.name.clone(),
        detail: desc,
        ship: ship_entity,
    };
    fact_sys.record_for(fact, vantages, origin_pos, now);
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
