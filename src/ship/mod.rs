use bevy::prelude::*;
use rand::Rng;

use crate::colony::{
    BuildQueue, Buildings, BuildingQueue, Colony, Production, ProductionFocus,
    ResourceStockpile,
};
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Habitability, ResourceLevel, StarSystem, SystemAttributes};
use crate::physics::{distance_ly, distance_ly_arr, sublight_travel_hexadies};
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};

// --- #34: Command queue ---

#[derive(Component, Default)]
pub struct CommandQueue {
    pub commands: Vec<QueuedCommand>,
}

#[derive(Clone, Debug)]
pub enum QueuedCommand {
    MoveTo(Entity),
    FTLTo(Entity),
    Survey(Entity),
    Colonize,
}

/// Result of an exploration event rolled when a survey completes.
#[derive(Clone, Debug)]
pub enum ExplorationEvent {
    ResourceBonus { resource: String, old_level: String, new_level: String },
    AncientRuins { research_bonus: f64 },
    Danger { description: String },
    Special { description: String },
    Nothing,
}

/// Roll a random exploration event.
///
/// Probabilities: 60% Nothing, 15% ResourceBonus, 10% AncientRuins, 10% Danger, 5% Special.
pub fn roll_exploration_event(rng: &mut impl Rng) -> ExplorationEvent {
    let roll: f64 = rng.random_range(0.0..1.0);
    if roll < 0.60 {
        ExplorationEvent::Nothing
    } else if roll < 0.75 {
        ExplorationEvent::ResourceBonus {
            resource: String::new(),
            old_level: String::new(),
            new_level: String::new(),
        }
    } else if roll < 0.85 {
        ExplorationEvent::AncientRuins { research_bonus: 0.0 }
    } else if roll < 0.95 {
        ExplorationEvent::Danger { description: String::new() }
    } else {
        ExplorationEvent::Special { description: String::new() }
    }
}

/// Attempt to upgrade a ResourceLevel one tier.
/// Returns the new level, or None if already Rich.
pub fn upgrade_resource_level(level: ResourceLevel) -> Option<ResourceLevel> {
    match level {
        ResourceLevel::None => Some(ResourceLevel::Poor),
        ResourceLevel::Poor => Some(ResourceLevel::Moderate),
        ResourceLevel::Moderate => Some(ResourceLevel::Rich),
        ResourceLevel::Rich => None,
    }
}

fn resource_level_name(level: ResourceLevel) -> &'static str {
    match level {
        ResourceLevel::Rich => "Rich",
        ResourceLevel::Moderate => "Moderate",
        ResourceLevel::Poor => "Poor",
        ResourceLevel::None => "None",
    }
}

/// Initial FTL speed as a multiple of light speed
pub const INITIAL_FTL_SPEED_C: f64 = 10.0;

/// Duration of a survey operation in hexadies (30 hexadies = half a year) (#32)
pub const SURVEY_DURATION_HEXADIES: i64 = 30;

/// Duration of a colonization/settling operation in hexadies (60 hexadies = 1 year) (#32)
pub const SETTLING_DURATION_HEXADIES: i64 = 60;

/// Maximum distance in light-years from which a survey can be initiated
pub const SURVEY_RANGE_LY: f64 = 5.0;

pub struct ShipPlugin;

impl Plugin for ShipPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (
            sublight_movement_system,
            process_ftl_travel,
            process_surveys,
            process_settling,
            process_pending_ship_commands,
            process_command_queue
                .after(sublight_movement_system)
                .after(process_ftl_travel)
                .after(process_surveys),
        ));
    }
}

// --- #33: Pending ship command system ---

/// A command queued for a remote ship, waiting for light-speed communication delay.
#[derive(Component)]
pub struct PendingShipCommand {
    pub ship: Entity,
    pub command: ShipCommand,
    pub arrives_at: i64,
}

/// The kinds of commands that can be issued to a ship.
#[derive(Clone, Debug)]
pub enum ShipCommand {
    FTLTo { destination: Entity },
    SubLightTo { destination: Entity },
    Survey { target: Entity },
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
    /// #32: Colony ship settling state
    Settling {
        system: Entity,
        started_at: i64,
        completes_at: i64,
    },
}

/// Cargo hold for Courier ships (and potentially others).
#[derive(Component, Default, Debug, Clone)]
pub struct Cargo {
    pub minerals: f64,
    pub energy: f64,
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
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id()
}

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
    start_ftl_travel_with_bonus(ship_state, ship, origin_system, destination_system, origin_pos, dest_pos, current_time, 0.0, 1.0)
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
) -> Result<(), &'static str> {
    if ship.ftl_range <= 0.0 {
        return Err("Ship has no FTL capability");
    }

    let effective_range = ship.ftl_range + ftl_range_bonus;
    let dist = distance_ly(origin_pos, dest_pos);
    if dist > effective_range {
        return Err("Destination is beyond FTL range");
    }

    let effective_ftl_speed = INITIAL_FTL_SPEED_C * ftl_speed_multiplier;
    let travel_hexadies = (dist * HEXADIES_PER_YEAR as f64 / effective_ftl_speed).ceil() as i64;

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

// --- Survey system (#9) ---

/// Attempt to start a survey operation on a target star system.
/// #45: Accepts optional survey_range_bonus from GlobalParams
pub fn start_survey(
    ship_state: &mut ShipState,
    ship: &Ship,
    target_system: Entity,
    ship_pos: &Position,
    system_pos: &Position,
    current_time: i64,
) -> Result<(), &'static str> {
    start_survey_with_bonus(ship_state, ship, target_system, ship_pos, system_pos, current_time, 0.0)
}

pub fn start_survey_with_bonus(
    ship_state: &mut ShipState,
    ship: &Ship,
    target_system: Entity,
    ship_pos: &Position,
    system_pos: &Position,
    current_time: i64,
    survey_range_bonus: f64,
) -> Result<(), &'static str> {
    if ship.ship_type != ShipType::Explorer {
        return Err("Only Explorer ships can perform surveys");
    }

    match ship_state {
        ShipState::Docked { .. } => {}
        _ => return Err("Ship must be docked to begin a survey"),
    }

    let effective_range = SURVEY_RANGE_LY + survey_range_bonus;
    let distance = ship_pos.distance_to(system_pos);
    if distance > effective_range {
        return Err("Target system is beyond survey range");
    }

    *ship_state = ShipState::Surveying {
        target_system,
        started_at: current_time,
        completes_at: current_time + SURVEY_DURATION_HEXADIES,
    };

    Ok(())
}

/// System that processes ongoing surveys and marks star systems as surveyed
/// when the survey duration has elapsed. Rolls an exploration event on completion.
pub fn process_surveys(
    clock: Res<GameClock>,
    mut ships: Query<(&mut Ship, &mut ShipState)>,
    mut systems: Query<(&mut StarSystem, Option<&mut SystemAttributes>)>,
    mut events: MessageWriter<GameEvent>,
) {
    let mut rng = rand::rng();

    for (mut ship, mut state) in ships.iter_mut() {
        let (target_system, completes_at) = match *state {
            ShipState::Surveying {
                target_system,
                completes_at,
                ..
            } => (target_system, completes_at),
            _ => continue,
        };

        if clock.elapsed >= completes_at {
            if let Ok((mut star_system, attrs)) = systems.get_mut(target_system) {
                star_system.surveyed = true;
                let system_name = star_system.name.clone();
                info!(
                    "Survey complete: {} has been surveyed",
                    system_name
                );

                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::SurveyComplete,
                    description: format!("{} completed survey of {}", ship.name, system_name),
                    related_system: Some(target_system),
                });

                // Roll an exploration event
                let event = roll_exploration_event(&mut rng);
                apply_exploration_event(
                    &event,
                    &system_name,
                    &mut ship,
                    attrs,
                    &mut rng,
                    clock.elapsed,
                    target_system,
                    &mut events,
                );
            }

            *state = ShipState::Docked {
                system: target_system,
            };
        }
    }
}

/// Apply an exploration event's effects and log it.
fn apply_exploration_event(
    event: &ExplorationEvent,
    system_name: &str,
    ship: &mut Ship,
    attrs: Option<Mut<SystemAttributes>>,
    rng: &mut impl Rng,
    timestamp: i64,
    target_system: Entity,
    events: &mut MessageWriter<GameEvent>,
) {
    match event {
        ExplorationEvent::Nothing => {}
        ExplorationEvent::ResourceBonus { .. } => {
            if let Some(mut attrs) = attrs {
                let field = rng.random_range(0u8..3);
                let (name, old_level) = match field {
                    0 => ("minerals", attrs.mineral_richness),
                    1 => ("energy", attrs.energy_potential),
                    _ => ("research", attrs.research_potential),
                };

                if let Some(new_level) = upgrade_resource_level(old_level) {
                    match field {
                        0 => attrs.mineral_richness = new_level,
                        1 => attrs.energy_potential = new_level,
                        _ => attrs.research_potential = new_level,
                    }
                    events.write(GameEvent {
                        timestamp,
                        kind: GameEventKind::SurveyDiscovery,
                        description: format!(
                            "Survey of {} discovered rich {} deposits! {} -> {}",
                            system_name,
                            name,
                            resource_level_name(old_level),
                            resource_level_name(new_level),
                        ),
                        related_system: Some(target_system),
                    });
                } else {
                    events.write(GameEvent {
                        timestamp,
                        kind: GameEventKind::SurveyDiscovery,
                        description: format!(
                            "Survey of {} found {} deposits already at maximum level",
                            system_name, name,
                        ),
                        related_system: Some(target_system),
                    });
                }
            }
        }
        ExplorationEvent::AncientRuins { .. } => {
            let bonus = rng.random_range(50.0..200.0);
            events.write(GameEvent {
                timestamp,
                kind: GameEventKind::SurveyDiscovery,
                description: format!(
                    "Ancient ruins discovered at {}! Research bonus: {:.0} RP",
                    system_name, bonus,
                ),
                related_system: Some(target_system),
            });
        }
        ExplorationEvent::Danger { .. } => {
            let damage_pct = rng.random_range(0.20..0.50);
            let damage = ship.max_hp * damage_pct as f32;
            ship.hp = (ship.hp - damage).max(1.0);
            events.write(GameEvent {
                timestamp,
                kind: GameEventKind::SurveyDiscovery,
                description: format!(
                    "Danger at {}! Ship {} took {:.0} damage ({:.0}% HP) from hazardous anomaly",
                    system_name, ship.name, damage, damage_pct * 100.0,
                ),
                related_system: Some(target_system),
            });
        }
        ExplorationEvent::Special { .. } => {
            if let Some(mut attrs) = attrs {
                let extra_slots = rng.random_range(1u8..=2);
                attrs.max_building_slots += extra_slots;
                events.write(GameEvent {
                    timestamp,
                    kind: GameEventKind::SurveyDiscovery,
                    description: format!(
                        "Special discovery at {}! Found {} additional building site(s)",
                        system_name, extra_slots,
                    ),
                    related_system: Some(target_system),
                });
            }
        }
    }
}

// --- Settling system (#32) ---

/// System that processes ongoing settling operations. When the timer completes,
/// establishes a colony and despawns the colony ship.
pub fn process_settling(
    mut commands: Commands,
    clock: Res<GameClock>,
    ships: Query<(Entity, &Ship, &ShipState)>,
    mut systems: Query<(&mut StarSystem, Option<&SystemAttributes>)>,
    mut events: MessageWriter<GameEvent>,
) {
    for (ship_entity, ship, state) in &ships {
        let (system_entity, completes_at) = match *state {
            ShipState::Settling {
                system,
                completes_at,
                ..
            } => (system, completes_at),
            _ => continue,
        };

        if clock.elapsed >= completes_at {
            let Ok((mut star_system, attrs)) = systems.get_mut(system_entity) else {
                continue;
            };

            if star_system.colonized {
                info!("System {} is already colonized, settling aborted", star_system.name);
                commands.entity(ship_entity).despawn();
                continue;
            }

            if let Some(attrs) = attrs {
                if attrs.habitability == Habitability::GasGiant {
                    info!("Colony Ship {} cannot colonize gas giant {}", ship.name, star_system.name);
                    continue;
                }

                star_system.colonized = true;
                let system_name = star_system.name.clone();
                let minerals_rate = resource_production_rate(attrs.mineral_richness);
                let energy_rate = resource_production_rate(attrs.energy_potential);
                let research_rate = resource_production_rate(attrs.research_potential);
                let num_slots = attrs.max_building_slots as usize;

                commands.spawn((
                    Colony {
                        system: system_entity,
                        population: 10.0,
                        growth_rate: 0.005,
                    },
                    ResourceStockpile {
                        minerals: 100.0,
                        energy: 100.0,
                        research: 0.0,
                    },
                    Production {
                        minerals_per_hexadies: minerals_rate,
                        energy_per_hexadies: energy_rate,
                        research_per_hexadies: research_rate,
                    },
                    BuildQueue {
                        queue: Vec::new(),
                    },
                    Buildings {
                        slots: vec![None; num_slots],
                    },
                    BuildingQueue::default(),
                    ProductionFocus::default(),
                ));

                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::ColonyEstablished,
                    description: format!("Colony established at {}", system_name),
                    related_system: Some(system_entity),
                });

                info!("Colony established at {} (M:{}/E:{}/R:{} per sd)", system_name, minerals_rate, energy_rate, research_rate);
            }

            // Consume the colony ship
            commands.entity(ship_entity).despawn();
        }
    }
}

// --- Colony ship arrival (#20) ---

pub fn resource_production_rate(level: ResourceLevel) -> f64 {
    match level {
        ResourceLevel::Rich => 8.0,
        ResourceLevel::Moderate => 5.0,
        ResourceLevel::Poor => 2.0,
        ResourceLevel::None => 0.0,
    }
}

// --- Pending ship command processing (#33) ---

/// Processes pending ship commands that have arrived after communication delay.
/// #45: Uses GlobalParams for tech bonuses
pub fn process_pending_ship_commands(
    mut commands: Commands,
    clock: Res<GameClock>,
    global_params: Res<crate::technology::GlobalParams>,
    pending: Query<(Entity, &PendingShipCommand)>,
    mut ships: Query<(&mut Ship, &mut ShipState, &Position)>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
) {
    for (cmd_entity, pending_cmd) in &pending {
        if clock.elapsed < pending_cmd.arrives_at {
            continue;
        }

        let Ok((ship, mut state, ship_pos)) = ships.get_mut(pending_cmd.ship) else {
            commands.entity(cmd_entity).despawn();
            continue;
        };

        let docked_system = match *state {
            ShipState::Docked { system } => system,
            _ => {
                info!(
                    "Remote command for {} discarded: ship is no longer docked",
                    ship.name,
                );
                commands.entity(cmd_entity).despawn();
                continue;
            }
        };

        match &pending_cmd.command {
            ShipCommand::FTLTo { destination } => {
                let dest = *destination;
                let Ok((dest_star, dest_pos)) = systems.get(dest) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                let Ok((_, origin_pos)) = systems.get(docked_system) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                match start_ftl_travel_with_bonus(
                    &mut state,
                    &ship,
                    docked_system,
                    dest,
                    origin_pos,
                    dest_pos,
                    clock.elapsed,
                    global_params.ftl_range_bonus,
                    global_params.ftl_speed_multiplier,
                ) {
                    Ok(()) => {
                        info!(
                            "Remote FTL command executed: {} jumping to {}",
                            ship.name, dest_star.name,
                        );
                    }
                    Err(e) => {
                        info!(
                            "Remote FTL command for {} failed: {}",
                            ship.name, e,
                        );
                    }
                }
            }
            ShipCommand::SubLightTo { destination } => {
                let dest = *destination;
                let Ok((dest_star, dest_pos)) = systems.get(dest) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                start_sublight_travel_with_bonus(
                    &mut state,
                    ship_pos,
                    &ship,
                    *dest_pos,
                    Some(dest),
                    clock.elapsed,
                    global_params.sublight_speed_bonus,
                );
                info!(
                    "Remote sub-light command executed: {} heading to {}",
                    ship.name, dest_star.name,
                );
            }
            ShipCommand::Survey { target } => {
                let tgt = *target;
                let Ok((tgt_star, tgt_pos)) = systems.get(tgt) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                match start_survey_with_bonus(&mut state, &ship, tgt, ship_pos, tgt_pos, clock.elapsed, global_params.survey_range_bonus) {
                    Ok(()) => {
                        info!(
                            "Remote survey command executed: {} surveying {}",
                            ship.name, tgt_star.name,
                        );
                    }
                    Err(e) => {
                        info!(
                            "Remote survey command for {} failed: {}",
                            ship.name, e,
                        );
                    }
                }
            }
        }

        commands.entity(cmd_entity).despawn();
    }
}

// --- Command queue processing (#34) ---

/// #45: Uses GlobalParams for tech bonuses
pub fn process_command_queue(
    clock: Res<GameClock>,
    global_params: Res<crate::technology::GlobalParams>,
    mut ships: Query<(Entity, &Ship, &mut ShipState, &mut CommandQueue, &Position)>,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
) {
    for (_entity, ship, mut state, mut queue, ship_pos) in ships.iter_mut() {
        // Only process queue when ship is Docked (current command finished)
        let ShipState::Docked { system: docked_system } = *state else {
            continue;
        };

        if queue.commands.is_empty() {
            continue;
        }

        let next = queue.commands.remove(0);

        match next {
            QueuedCommand::MoveTo(target) => {
                let Ok((_target_entity, target_star, target_pos)) = systems.get(target) else {
                    warn!("Queued MoveTo target no longer exists");
                    continue;
                };
                start_sublight_travel_with_bonus(
                    &mut state,
                    ship_pos,
                    ship,
                    *target_pos,
                    Some(target),
                    clock.elapsed,
                    global_params.sublight_speed_bonus,
                );
                info!("Queue: Ship {} moving to {}", ship.name, target_star.name);
            }
            QueuedCommand::FTLTo(target) => {
                let Ok((_target_entity, target_star, target_pos)) = systems.get(target) else {
                    warn!("Queued FTLTo target no longer exists");
                    continue;
                };
                let Ok((_dock_entity, _dock_star, dock_pos)) = systems.get(docked_system) else {
                    continue;
                };
                match start_ftl_travel_with_bonus(
                    &mut state,
                    ship,
                    docked_system,
                    target,
                    dock_pos,
                    target_pos,
                    clock.elapsed,
                    global_params.ftl_range_bonus,
                    global_params.ftl_speed_multiplier,
                ) {
                    Ok(()) => {
                        info!(
                            "Queue: Ship {} FTL jumping to {}",
                            ship.name, target_star.name
                        );
                    }
                    Err(e) => {
                        warn!("Queue: FTL failed for {}: {}", ship.name, e);
                    }
                }
            }
            QueuedCommand::Survey(target) => {
                let Ok((_target_entity, target_star, target_pos)) = systems.get(target) else {
                    warn!("Queued Survey target no longer exists");
                    continue;
                };
                match start_survey_with_bonus(
                    &mut state,
                    ship,
                    target,
                    ship_pos,
                    target_pos,
                    clock.elapsed,
                    global_params.survey_range_bonus,
                ) {
                    Ok(()) => {
                        info!(
                            "Queue: Ship {} surveying {}",
                            ship.name, target_star.name
                        );
                    }
                    Err(e) => {
                        warn!("Queue: Survey failed for {}: {}", ship.name, e);
                    }
                }
            }
            QueuedCommand::Colonize => {
                // Colonization is handled automatically by process_settling
                // when a colony ship docks. No explicit action needed here.
                info!(
                    "Queue: Ship {} colonize command (handled on dock)",
                    ship.name
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;

    fn make_ship(ship_type: ShipType) -> Ship {
        Ship {
            name: "Test Ship".to_string(),
            ship_type,
            owner: Owner::Player,
            sublight_speed: ship_type.default_sublight_speed(),
            ftl_range: ship_type.default_ftl_range(),
            hp: ship_type.default_hp(),
            max_hp: ship_type.default_hp(),
            player_aboard: false,
        }
    }

    #[test]
    fn start_sublight_sets_correct_arrival_time() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship(ShipType::ColonyShip); // 0.5c
        let origin = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest = Position { x: 1.0, y: 0.0, z: 0.0 }; // 1 LY away
        let mut state = ShipState::Docked { system };
        start_sublight_travel(&mut state, &origin, &ship, dest, Some(system), 100);
        match state {
            ShipState::SubLight { arrival_at, departed_at, .. } => {
                assert_eq!(departed_at, 100);
                assert_eq!(arrival_at, 220);
            }
            _ => panic!("Expected SubLight state"),
        }
    }

    #[test]
    fn start_ftl_rejects_no_ftl_ship() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship(ShipType::Explorer);
        let mut state = ShipState::Docked { system: origin };
        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 1.0, y: 0.0, z: 0.0 };
        let result = start_ftl_travel(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0);
        assert_eq!(result, Err("Ship has no FTL capability"));
    }

    #[test]
    fn start_ftl_rejects_out_of_range() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship(ShipType::ColonyShip);
        let mut state = ShipState::Docked { system: origin };
        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 50.0, y: 0.0, z: 0.0 };
        let result = start_ftl_travel(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0);
        assert_eq!(result, Err("Destination is beyond FTL range"));
    }

    #[test]
    fn start_ftl_correct_travel_time() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship(ShipType::ColonyShip);
        let mut state = ShipState::Docked { system: origin };
        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 10.0, y: 0.0, z: 0.0 };
        let result = start_ftl_travel(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0);
        assert!(result.is_ok());
        match state {
            ShipState::InFTL { arrival_at, .. } => assert_eq!(arrival_at, 60),
            _ => panic!("Expected InFTL state"),
        }
    }

    #[test]
    fn start_survey_rejects_non_explorer() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship(ShipType::ColonyShip);
        let mut state = ShipState::Docked { system };
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 0);
        assert_eq!(result, Err("Only Explorer ships can perform surveys"));
    }

    #[test]
    fn start_survey_rejects_non_docked() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship(ShipType::Explorer);
        let mut state = ShipState::SubLight {
            origin: [0.0; 3],
            destination: [1.0, 0.0, 0.0],
            target_system: Some(system),
            departed_at: 0,
            arrival_at: 100,
        };
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 0);
        assert_eq!(result, Err("Ship must be docked to begin a survey"));
    }

    #[test]
    fn start_survey_rejects_out_of_range() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship(ShipType::Explorer);
        let mut state = ShipState::Docked { system };
        let ship_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let target_pos = Position { x: 10.0, y: 0.0, z: 0.0 };
        let result = start_survey(&mut state, &ship, system, &ship_pos, &target_pos, 0);
        assert_eq!(result, Err("Target system is beyond survey range"));
    }

    #[test]
    fn start_survey_sets_correct_completion_time() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship(ShipType::Explorer);
        let mut state = ShipState::Docked { system };
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 50);
        assert!(result.is_ok());
        match state {
            ShipState::Surveying { completes_at, started_at, .. } => {
                assert_eq!(started_at, 50);
                assert_eq!(completes_at, 80); // 50 + SURVEY_DURATION_HEXADIES (30)
            }
            _ => panic!("Expected Surveying state"),
        }
    }

    #[test]
    fn test_roll_exploration_event_does_not_panic() {
        let mut rng = rand::rng();
        for _ in 0..1000 {
            let event = roll_exploration_event(&mut rng);
            match event {
                ExplorationEvent::Nothing
                | ExplorationEvent::ResourceBonus { .. }
                | ExplorationEvent::AncientRuins { .. }
                | ExplorationEvent::Danger { .. }
                | ExplorationEvent::Special { .. } => {}
            }
        }
    }

    #[test]
    fn test_upgrade_resource_level() {
        use crate::galaxy::ResourceLevel;
        assert_eq!(upgrade_resource_level(ResourceLevel::None), Some(ResourceLevel::Poor));
        assert_eq!(upgrade_resource_level(ResourceLevel::Poor), Some(ResourceLevel::Moderate));
        assert_eq!(upgrade_resource_level(ResourceLevel::Moderate), Some(ResourceLevel::Rich));
        assert_eq!(upgrade_resource_level(ResourceLevel::Rich), None);
    }

    #[test]
    fn test_resource_level_name() {
        use crate::galaxy::ResourceLevel;
        assert_eq!(resource_level_name(ResourceLevel::Rich), "Rich");
        assert_eq!(resource_level_name(ResourceLevel::Moderate), "Moderate");
        assert_eq!(resource_level_name(ResourceLevel::Poor), "Poor");
        assert_eq!(resource_level_name(ResourceLevel::None), "None");
    }

    #[test]
    fn test_roll_distribution_roughly_correct() {
        let mut rng = rand::rng();
        let mut nothing = 0u32;
        let mut resource = 0u32;
        let mut ruins = 0u32;
        let mut danger = 0u32;
        let mut special = 0u32;

        let n = 10_000;
        for _ in 0..n {
            match roll_exploration_event(&mut rng) {
                ExplorationEvent::Nothing => nothing += 1,
                ExplorationEvent::ResourceBonus { .. } => resource += 1,
                ExplorationEvent::AncientRuins { .. } => ruins += 1,
                ExplorationEvent::Danger { .. } => danger += 1,
                ExplorationEvent::Special { .. } => special += 1,
            }
        }

        assert!(nothing > 0, "Nothing should appear");
        assert!(resource > 0, "ResourceBonus should appear");
        assert!(ruins > 0, "AncientRuins should appear");
        assert!(danger > 0, "Danger should appear");
        assert!(special > 0, "Special should appear");

        assert!(nothing > resource, "Nothing should be more common than ResourceBonus");
        assert!(nothing > ruins, "Nothing should be more common than AncientRuins");
    }
}
