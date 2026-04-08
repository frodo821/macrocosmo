use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::physics::{distance_ly, distance_ly_arr, sublight_travel_sexadies};
use crate::time_system::{GameClock, SEXADIES_PER_YEAR};

/// Initial FTL speed as a multiple of light speed
pub const INITIAL_FTL_SPEED_C: f64 = 10.0;

/// Duration of a survey operation in sexadies (5 sexadies = 1 month)
pub const SURVEY_DURATION_SEXADIES: i64 = 5;

/// Maximum distance in light-years from which a survey can be initiated
pub const SURVEY_RANGE_LY: f64 = 5.0;

pub struct ShipPlugin;

impl Plugin for ShipPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (sublight_movement_system, process_ftl_travel, process_surveys));
    }
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShipType {
    Explorer,
    ColonyShip,
    Courier,
}

impl ShipType {
    pub fn default_sublight_speed(&self) -> f64 {
        match self {
            ShipType::Explorer => 0.75,
            ShipType::ColonyShip => 0.5,
            ShipType::Courier => 0.85,
        }
    }

    pub fn default_ftl_range(&self) -> f64 {
        match self {
            ShipType::Explorer => 0.0,
            ShipType::ColonyShip => 30.0,
            ShipType::Courier => 0.0,
        }
    }

    pub fn default_hp(&self) -> f32 {
        match self {
            ShipType::Explorer => 50.0,
            ShipType::ColonyShip => 100.0,
            ShipType::Courier => 20.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Player,
}

#[derive(Component)]
pub struct Ship {
    pub name: String,
    pub ship_type: ShipType,
    pub owner: Owner,
    pub sublight_speed: f64,
    pub ftl_range: f64,
    pub hp: f32,
    pub max_hp: f32,
    pub player_aboard: bool,
}

#[derive(Component)]
pub enum ShipState {
    Docked { system: Entity },
    SubLight {
        origin: [f64; 3],
        destination: [f64; 3],
        target_system: Option<Entity>,
        departed_at: i64,
        arrival_at: i64,
    },
    InFTL {
        origin_system: Entity,
        destination_system: Entity,
        departed_at: i64,
        arrival_at: i64,
    },
    Surveying {
        target_system: Entity,
        started_at: i64,
        completes_at: i64,
    },
}

pub fn spawn_ship(
    commands: &mut Commands,
    ship_type: ShipType,
    name: String,
    system: Entity,
    initial_position: Position,
) -> Entity {
    let hp = ship_type.default_hp();
    commands
        .spawn((
            Ship {
                name,
                ship_type,
                owner: Owner::Player,
                sublight_speed: ship_type.default_sublight_speed(),
                ftl_range: ship_type.default_ftl_range(),
                hp,
                max_hp: hp,
                player_aboard: false,
            },
            ShipState::Docked { system },
            initial_position,
        ))
        .id()
}

// --- Sub-light travel ---

pub fn start_sublight_travel(
    ship_state: &mut ShipState,
    ship_pos: &Position,
    ship: &Ship,
    destination: Position,
    target_system: Option<Entity>,
    current_time: i64,
) {
    let origin = ship_pos.as_array();
    let dest = destination.as_array();
    let dist = distance_ly_arr(origin, dest);
    let travel_time = sublight_travel_sexadies(dist, ship.sublight_speed);
    *ship_state = ShipState::SubLight {
        origin,
        destination: dest,
        target_system,
        departed_at: current_time,
        arrival_at: current_time + travel_time,
    };
}

fn sublight_movement_system(
    clock: Res<GameClock>,
    mut query: Query<(&mut ShipState, &mut Position, &Ship)>,
) {
    for (mut state, mut pos, _ship) in query.iter_mut() {
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
            }
        } else {
            pos.x = origin[0] + (destination[0] - origin[0]) * progress;
            pos.y = origin[1] + (destination[1] - origin[1]) * progress;
            pos.z = origin[2] + (destination[2] - origin[2]) * progress;
        }
    }
}

// --- FTL travel ---

pub fn start_ftl_travel(
    ship_state: &mut ShipState,
    ship: &Ship,
    origin_system: Entity,
    destination_system: Entity,
    origin_pos: &Position,
    dest_pos: &Position,
    current_time: i64,
) -> Result<(), &'static str> {
    if ship.ftl_range <= 0.0 {
        return Err("Ship has no FTL capability");
    }

    let dist = distance_ly(origin_pos, dest_pos);
    if dist > ship.ftl_range {
        return Err("Destination is beyond FTL range");
    }

    let travel_sexadies = (dist * SEXADIES_PER_YEAR as f64 / INITIAL_FTL_SPEED_C).ceil() as i64;

    *ship_state = ShipState::InFTL {
        origin_system,
        destination_system,
        departed_at: current_time,
        arrival_at: current_time + travel_sexadies,
    };

    Ok(())
}

fn process_ftl_travel(
    clock: Res<GameClock>,
    mut ships: Query<(&Ship, &mut ShipState, &mut Position)>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
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
                info!("Ship {} arrived at {} via FTL", ship.name, star.name);
            } else {
                warn!("Ship {} FTL destination entity no longer exists", ship.name);
            }
        }
    }
}

// --- Survey system (#9) ---

/// Attempt to start a survey operation on a target star system.
///
/// Validates that the ship is an Explorer, is not in transit, and is within
/// `SURVEY_RANGE_LY` of the target system.
pub fn start_survey(
    ship_state: &mut ShipState,
    ship: &Ship,
    target_system: Entity,
    ship_pos: &Position,
    system_pos: &Position,
    current_time: i64,
) -> Result<(), &'static str> {
    // Ship must be an Explorer
    if ship.ship_type != ShipType::Explorer {
        return Err("Only Explorer ships can perform surveys");
    }

    // Ship must be Docked (not in transit or already surveying)
    match ship_state {
        ShipState::Docked { .. } => {}
        _ => return Err("Ship must be docked to begin a survey"),
    }

    // Target system must be within survey range
    let distance = ship_pos.distance_to(system_pos);
    if distance > SURVEY_RANGE_LY {
        return Err("Target system is beyond survey range");
    }

    *ship_state = ShipState::Surveying {
        target_system,
        started_at: current_time,
        completes_at: current_time + SURVEY_DURATION_SEXADIES,
    };

    Ok(())
}

/// System that processes ongoing surveys and marks star systems as surveyed
/// when the survey duration has elapsed.
fn process_surveys(
    clock: Res<GameClock>,
    mut ships: Query<(&Ship, &mut ShipState)>,
    mut systems: Query<&mut StarSystem>,
) {
    for (_ship, mut state) in ships.iter_mut() {
        let (target_system, completes_at) = match *state {
            ShipState::Surveying {
                target_system,
                completes_at,
                ..
            } => (target_system, completes_at),
            _ => continue,
        };

        if clock.elapsed >= completes_at {
            if let Ok(mut star_system) = systems.get_mut(target_system) {
                star_system.surveyed = true;
                info!(
                    "Survey complete: {} has been surveyed",
                    star_system.name
                );
            }

            // Transition ship back to docked at the target system
            *state = ShipState::Docked {
                system: target_system,
            };
        }
    }
}
