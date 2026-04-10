use bevy::prelude::*;
use rand::Rng;

use crate::amount::Amt;
use crate::colony::{
    BuildQueue, Buildings, BuildingQueue, Colony, FoodConsumption, MaintenanceCost,
    Production, ProductionFocus, ResourceCapacity, ResourceStockpile,
};
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Habitability, HostilePresence, ResourceLevel, StarSystem, SystemAttributes};
use crate::modifier::{CachedValue, Modifier, ScopedModifiers};
use crate::physics::{distance_ly, distance_ly_arr, sublight_travel_hexadies};
use crate::knowledge::{KnowledgeStore, SystemKnowledge, SystemSnapshot};
use crate::player::{Player, PlayerEmpire, StationedAt};
use crate::ship_design::{HullRegistry, ModuleRegistry};
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};

// --- #34: Command queue ---

#[derive(Component, Default, Clone)]
pub struct CommandQueue {
    pub commands: Vec<QueuedCommand>,
    /// Predicted position after all queued commands execute
    pub predicted_position: [f64; 3],
    /// Predicted system after all queued commands execute
    pub predicted_system: Option<Entity>,
}

impl CommandQueue {
    /// Push a command and update predicted position
    pub fn push(&mut self, cmd: QueuedCommand, system_positions: &impl Fn(Entity) -> Option<[f64; 3]>) {
        match &cmd {
            QueuedCommand::MoveTo { system } | QueuedCommand::Survey { system } | QueuedCommand::Colonize { system } => {
                if let Some(pos) = system_positions(*system) {
                    self.predicted_position = pos;
                    self.predicted_system = Some(*system);
                }
            }
        }
        self.commands.push(cmd);
    }

    /// Reset prediction to current ship state (call when queue becomes empty or command consumed)
    pub fn sync_prediction(&mut self, current_pos: [f64; 3], current_system: Option<Entity>) {
        if self.commands.is_empty() {
            self.predicted_position = current_pos;
            self.predicted_system = current_system;
        }
    }
}

#[derive(Clone, Debug)]
pub enum QueuedCommand {
    MoveTo { system: Entity },
    Survey { system: Entity },
    Colonize { system: Entity },
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
            sync_ship_module_modifiers,
            sync_ship_hitpoints.after(sync_ship_module_modifiers),
            tick_shield_regen,
            sublight_movement_system,
            process_ftl_travel,
            deliver_survey_results.after(process_ftl_travel),
            process_surveys,
            process_settling,
            process_pending_ship_commands,
            process_command_queue
                .after(sublight_movement_system)
                .after(process_ftl_travel)
                .after(process_surveys),
            resolve_combat,
            tick_ship_repair,
        ).after(crate::time_system::advance_game_time)
         .before(crate::colony::advance_production_tick));
    }
}

/// Syncs module modifiers from equipped modules to ShipModifiers.
/// Clears and rebuilds module modifiers each time a ship's modules change.
pub fn sync_ship_module_modifiers(
    ships: Query<(Entity, &Ship), Changed<Ship>>,
    mut ship_mods: Query<&mut ShipModifiers>,
    module_registry: Res<ModuleRegistry>,
    hull_registry: Res<HullRegistry>,
) {
    use crate::amount::SignedAmt;
    for (entity, ship) in &ships {
        let Ok(mut mods) = ship_mods.get_mut(entity) else { continue };
        // Reset all module modifiers by creating fresh scoped modifiers
        // (preserving base values but clearing modifiers)
        mods.speed = ScopedModifiers::default();
        mods.ftl_range = ScopedModifiers::default();
        mods.survey_speed = ScopedModifiers::default();
        mods.colonize_speed = ScopedModifiers::default();
        mods.evasion = ScopedModifiers::default();
        mods.cargo_capacity = ScopedModifiers::default();
        mods.attack = ScopedModifiers::default();
        mods.defense = ScopedModifiers::default();
        mods.armor_max = ScopedModifiers::default();
        mods.shield_max = ScopedModifiers::default();
        mods.shield_regen = ScopedModifiers::default();

        // Apply hull modifiers first
        if let Some(hull_def) = hull_registry.get(&ship.hull_id) {
            for mod_def in &hull_def.modifiers {
                let modifier = Modifier {
                    id: format!("hull_{}_{}", ship.hull_id, mod_def.target),
                    label: hull_def.name.clone(),
                    base_add: SignedAmt::from_f64(mod_def.base_add),
                    multiplier: SignedAmt::from_f64(mod_def.multiplier),
                    add: SignedAmt::from_f64(mod_def.add),
                    expires_at: None,
                    on_expire_event: None,
                };
                push_ship_modifier(&mut mods, &mod_def.target, modifier);
            }
        }

        // Apply module modifiers
        for (i, equipped) in ship.modules.iter().enumerate() {
            if let Some(module_def) = module_registry.modules.get(&equipped.module_id) {
                for mod_def in &module_def.modifiers {
                    let modifier = Modifier {
                        id: format!("module_{}_{}", equipped.module_id, i),
                        label: module_def.name.clone(),
                        base_add: SignedAmt::from_f64(mod_def.base_add),
                        multiplier: SignedAmt::from_f64(mod_def.multiplier),
                        add: SignedAmt::from_f64(mod_def.add),
                        expires_at: None,
                        on_expire_event: None,
                    };
                    push_ship_modifier(&mut mods, &mod_def.target, modifier);
                }
            }
        }
    }
}

/// Push a modifier to the appropriate ShipModifiers field based on target string.
fn push_ship_modifier(mods: &mut Mut<ShipModifiers>, target: &str, modifier: Modifier) {
    match target {
        "ship.speed" => mods.speed.push_modifier(modifier),
        "ship.ftl_range" => mods.ftl_range.push_modifier(modifier),
        "ship.survey_speed" => mods.survey_speed.push_modifier(modifier),
        "ship.colonize_speed" => mods.colonize_speed.push_modifier(modifier),
        "ship.evasion" => mods.evasion.push_modifier(modifier),
        "ship.cargo_capacity" => mods.cargo_capacity.push_modifier(modifier),
        "ship.attack" => mods.attack.push_modifier(modifier),
        "ship.defense" => mods.defense.push_modifier(modifier),
        "ship.armor_max" => mods.armor_max.push_modifier(modifier),
        "ship.shield_max" => mods.shield_max.push_modifier(modifier),
        "ship.shield_regen" => mods.shield_regen.push_modifier(modifier),
        _ => {}
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
    MoveTo { destination: Entity },
    Survey { target: Entity },
    Colonize,
}

/// A module equipped in a specific slot on a ship.
#[derive(Clone, Debug)]
pub struct EquippedModule {
    pub slot_type: String,
    pub module_id: String,
}

/// Per-ship modifier scopes, driven by equipped modules and tech effects.
#[derive(Component, Default)]
pub struct ShipModifiers {
    pub speed: ScopedModifiers,
    pub ftl_range: ScopedModifiers,
    pub survey_speed: ScopedModifiers,
    pub colonize_speed: ScopedModifiers,
    pub evasion: ScopedModifiers,
    pub cargo_capacity: ScopedModifiers,
    pub attack: ScopedModifiers,
    pub defense: ScopedModifiers,
    pub armor_max: ScopedModifiers,
    pub shield_max: ScopedModifiers,
    pub shield_regen: ScopedModifiers,
}

/// Cached computed stats for a ship, derived from ShipModifiers.
#[derive(Component, Default)]
pub struct ShipStats {
    pub speed: CachedValue,
    pub ftl_range: CachedValue,
    pub survey_speed: CachedValue,
    pub colonize_speed: CachedValue,
    pub evasion: CachedValue,
    pub cargo_capacity: CachedValue,
    pub maintenance: Amt,
}

/// Ship design presets for the three legacy ship types.
/// These provide default stats when registries are not available (e.g. in tests).
pub struct ShipDesignPreset {
    pub design_id: &'static str,
    pub design_name: &'static str,
    pub hull_id: &'static str,
    pub sublight_speed: f64,
    pub ftl_range: f64,
    pub hp: f64,
    pub maintenance: Amt,
    pub build_cost_minerals: Amt,
    pub build_cost_energy: Amt,
    pub build_time: i64,
    pub can_survey: bool,
    pub can_colonize: bool,
}

pub const EXPLORER_PRESET: ShipDesignPreset = ShipDesignPreset {
    design_id: "explorer_mk1",
    design_name: "Explorer",
    hull_id: "corvette",
    sublight_speed: 0.75,
    ftl_range: 10.0,
    hp: 50.0,
    maintenance: Amt::new(0, 500),
    build_cost_minerals: Amt::units(200),
    build_cost_energy: Amt::units(100),
    build_time: 60,
    can_survey: true,
    can_colonize: false,
};

pub const COLONY_SHIP_PRESET: ShipDesignPreset = ShipDesignPreset {
    design_id: "colony_ship_mk1",
    design_name: "Colony Ship",
    hull_id: "freighter",
    sublight_speed: 0.5,
    ftl_range: 15.0,
    hp: 100.0,
    maintenance: Amt::units(1),
    build_cost_minerals: Amt::units(500),
    build_cost_energy: Amt::units(300),
    build_time: 120,
    can_survey: false,
    can_colonize: true,
};

pub const COURIER_PRESET: ShipDesignPreset = ShipDesignPreset {
    design_id: "courier_mk1",
    design_name: "Courier",
    hull_id: "courier_hull",
    sublight_speed: 0.80,
    ftl_range: 0.0,
    hp: 35.0,
    maintenance: Amt::new(0, 300),
    build_cost_minerals: Amt::units(100),
    build_cost_energy: Amt::units(50),
    build_time: 30,
    can_survey: false,
    can_colonize: false,
};

pub const SCOUT_PRESET: ShipDesignPreset = ShipDesignPreset {
    design_id: "scout_mk1",
    design_name: "Scout",
    hull_id: "scout_hull",
    sublight_speed: 0.85,
    ftl_range: 10.0,
    hp: 40.0,
    maintenance: Amt::new(0, 400),
    build_cost_minerals: Amt::units(150),
    build_cost_energy: Amt::units(80),
    build_time: 45,
    can_survey: true,
    can_colonize: false,
};

/// Look up a design preset by design_id.
pub fn design_preset(design_id: &str) -> Option<&'static ShipDesignPreset> {
    match design_id {
        "explorer_mk1" => Some(&EXPLORER_PRESET),
        "colony_ship_mk1" => Some(&COLONY_SHIP_PRESET),
        "courier_mk1" => Some(&COURIER_PRESET),
        "scout_mk1" => Some(&SCOUT_PRESET),
        _ => None,
    }
}

/// All available design presets.
pub fn all_design_presets() -> &'static [&'static ShipDesignPreset] {
    &[&EXPLORER_PRESET, &COLONY_SHIP_PRESET, &COURIER_PRESET, &SCOUT_PRESET]
}

/// Compute maintenance cost for a ship given its design_id.
/// Falls back to the design preset if registries are not available.
pub fn ship_maintenance_cost(design_id: &str) -> Amt {
    design_preset(design_id).map(|p| p.maintenance).unwrap_or(Amt::new(0, 500))
}

/// Compute build cost (minerals, energy) for a ship given its design_id.
pub fn ship_build_cost(design_id: &str) -> (Amt, Amt) {
    design_preset(design_id)
        .map(|p| (p.build_cost_minerals, p.build_cost_energy))
        .unwrap_or((Amt::units(200), Amt::units(100)))
}

/// Compute build time for a ship given its design_id.
pub fn ship_build_time(design_id: &str) -> i64 {
    design_preset(design_id).map(|p| p.build_time).unwrap_or(60)
}

/// Scrap refund: 50% of build cost in both minerals and energy.
pub fn ship_scrap_refund(design_id: &str) -> (Amt, Amt) {
    let (m, e) = ship_build_cost(design_id);
    (Amt::milli(m.raw() / 2), Amt::milli(e.raw() / 2))
}

/// Check if a design can perform surveys.
pub fn design_can_survey(design_id: &str) -> bool {
    design_preset(design_id).map(|p| p.can_survey).unwrap_or(false)
}

/// Check if a design can colonize.
pub fn design_can_colonize(design_id: &str) -> bool {
    design_preset(design_id).map(|p| p.can_colonize).unwrap_or(false)
}

/// 3-layer hit point model: shield → armor → hull.
/// Shield regenerates over time; armor/hull require docking at a Port.
#[derive(Component, Clone, Debug)]
pub struct ShipHitpoints {
    pub hull: f64,
    pub hull_max: f64,
    pub armor: f64,
    pub armor_max: f64,
    pub shield: f64,
    pub shield_max: f64,
    pub shield_regen: f64, // per hexadies
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Empire(Entity),
    Neutral,
}

impl Owner {
    /// Check if this owner is any empire (not neutral).
    pub fn is_empire(&self) -> bool {
        matches!(self, Owner::Empire(_))
    }
}

#[derive(Component)]
pub struct Ship {
    pub name: String,
    pub design_id: String,
    pub hull_id: String,
    pub modules: Vec<EquippedModule>,
    pub owner: Owner,
    pub sublight_speed: f64,
    pub ftl_range: f64,
    pub player_aboard: bool,
    /// #64: System entity where maintenance is charged
    pub home_port: Entity,
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
    pub minerals: Amt,
    pub energy: Amt,
}

/// #103: Survey data carried by an FTL-capable ship back to the player's system.
/// Stored on the ship when survey completes until the ship docks at the player's
/// StationedAt system, at which point the results are published.
#[derive(Component, Clone, Debug)]
pub struct SurveyData {
    /// The system that was surveyed.
    pub target_system: Entity,
    /// The game time when the survey completed.
    pub surveyed_at: i64,
    /// Name of the surveyed system (cached for event descriptions).
    pub system_name: String,
}

// --- #54: Fleet formation system ---

#[derive(Component)]
pub struct Fleet {
    pub name: String,
    pub members: Vec<Entity>,
    pub flagship: Entity,
}

impl Fleet {
    /// Fleet movement speed = slowest member
    pub fn speed(&self, ships: &Query<&Ship>) -> f64 {
        self.members
            .iter()
            .filter_map(|e| ships.get(*e).ok())
            .map(|s| s.sublight_speed)
            .fold(f64::MAX, f64::min)
    }

    /// Fleet FTL range = shortest range member
    pub fn ftl_range(&self, ships: &Query<&Ship>) -> f64 {
        self.members
            .iter()
            .filter_map(|e| ships.get(*e).ok())
            .map(|s| s.ftl_range)
            .fold(f64::MAX, f64::min)
    }
}

/// Marks a ship as belonging to a fleet.
#[derive(Component)]
pub struct FleetMembership {
    pub fleet: Entity,
}

/// Create a fleet from the given ships, returning the fleet entity.
pub fn create_fleet(
    commands: &mut Commands,
    name: String,
    members: Vec<Entity>,
    flagship: Entity,
) -> Entity {
    let fleet_entity = commands
        .spawn(Fleet {
            name,
            members: members.clone(),
            flagship,
        })
        .id();
    for member in &members {
        commands
            .entity(*member)
            .insert(FleetMembership { fleet: fleet_entity });
    }
    fleet_entity
}

/// Dissolve a fleet, removing FleetMembership from all members and despawning the fleet entity.
pub fn dissolve_fleet(commands: &mut Commands, fleet_entity: Entity, fleet: &Fleet) {
    for member in &fleet.members {
        commands.entity(*member).remove::<FleetMembership>();
    }
    commands.entity(fleet_entity).despawn();
}

pub fn spawn_ship(
    commands: &mut Commands,
    design_id: &str,
    name: String,
    system: Entity,
    initial_position: Position,
    owner: Owner,
) -> Entity {
    let preset = design_preset(design_id).unwrap_or(&EXPLORER_PRESET);
    let hull_hp = preset.hp;
    commands
        .spawn((
            Ship {
                name,
                design_id: preset.design_id.to_string(),
                hull_id: preset.hull_id.to_string(),
                modules: Vec::new(),
                owner,
                sublight_speed: preset.sublight_speed,
                ftl_range: preset.ftl_range,
                player_aboard: false,
                home_port: system,
            },
            ShipState::Docked { system },
            initial_position,
            CommandQueue::default(),
            Cargo::default(),
            ShipHitpoints {
                hull: hull_hp,
                hull_max: hull_hp,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            ShipStats::default(),
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
    start_ftl_travel_with_bonus(ship_state, ship, origin_system, destination_system, origin_pos, dest_pos, current_time, 0.0, 1.0, false)
}

/// Port FTL range bonus in light-years (#46)
pub const PORT_FTL_RANGE_BONUS_LY: f64 = 10.0;

/// Port FTL travel time reduction factor (#46): 20% reduction
pub const PORT_TRAVEL_TIME_FACTOR: f64 = 0.8;

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
    if !design_can_survey(&ship.design_id) {
        return Err("Only Explorer ships can perform surveys");
    }

    let docked_system = match ship_state {
        ShipState::Docked { system } => *system,
        _ => return Err("Ship must be docked to begin a survey"),
    };

    // #102: Ship must be docked at the target system to survey it
    if docked_system != target_system {
        return Err("Ship must be docked at the target system to survey it");
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
///
/// #103: FTL-capable ships store survey data internally instead of publishing
/// immediately. They auto-queue an FTL return to the player's StationedAt system
/// if no commands are pending. Non-FTL ships publish results immediately via
/// light-speed propagation (existing behavior).
pub fn process_surveys(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut ships: Query<(Entity, &Ship, &mut ShipState, &mut ShipHitpoints, &Position, Option<&mut CommandQueue>)>,
    mut systems: Query<(&mut StarSystem, Option<&mut SystemAttributes>, &Position), Without<Ship>>,
    hostiles: Query<&HostilePresence>,
    player_q: Query<&StationedAt, With<Player>>,
    mut events: MessageWriter<GameEvent>,
) {
    let mut rng = rand::rng();

    // Collect player's stationed-at system for auto-return
    let player_system = player_q.iter().next().map(|s| s.system);

    for (ship_entity, ship, mut state, mut ship_hp, ship_pos, mut cmd_queue) in ships.iter_mut() {
        let (target_system, completes_at) = match *state {
            ShipState::Surveying {
                target_system,
                completes_at,
                ..
            } => (target_system, completes_at),
            _ => continue,
        };

        if clock.elapsed >= completes_at {
            let has_ftl = ship.ftl_range > 0.0;

            if has_ftl {
                // #103: FTL ship — store data internally, do NOT mark system as surveyed yet
                if let Ok((star_system, attrs, _sys_pos)) = systems.get_mut(target_system) {
                    let system_name = star_system.name.clone();
                    info!(
                        "Survey complete (FTL ship): {} surveyed {} — data stored on ship",
                        ship.name, system_name
                    );

                    // Check for hostile presence (this is immediately visible to the ship)
                    let has_hostile = hostiles.iter().any(|h| h.system == target_system);
                    if has_hostile {
                        events.write(GameEvent {
                            timestamp: clock.elapsed,
                            kind: GameEventKind::HostileDetected,
                            description: format!(
                                "Warning: Hostile presence detected at {}!",
                                system_name,
                            ),
                            related_system: Some(target_system),
                        });
                    }

                    // Roll exploration event (applied immediately — ship observes it)
                    let event = roll_exploration_event(&mut rng);
                    apply_exploration_event(
                        &event,
                        &system_name,
                        &ship,
                        &mut ship_hp,
                        attrs,
                        &mut rng,
                        clock.elapsed,
                        target_system,
                        &mut events,
                    );

                    // Store survey data on the ship entity
                    commands.entity(ship_entity).insert(SurveyData {
                        target_system,
                        surveyed_at: clock.elapsed,
                        system_name: system_name.clone(),
                    });

                    // Auto-queue FTL return to player's system if command queue is empty
                    let queue_empty = cmd_queue.as_ref().map(|q| q.commands.is_empty()).unwrap_or(true);
                    if queue_empty {
                        if let Some(player_sys) = player_system {
                            if player_sys != target_system {
                                if let Some(ref mut queue) = cmd_queue {
                                    queue.commands.push(QueuedCommand::MoveTo {
                                        system: player_sys,
                                    });
                                    info!(
                                        "Auto-queued FTL return to player system for {}",
                                        ship.name
                                    );
                                }
                            }
                        }
                    }
                }
            } else {
                // Non-FTL ship — existing behavior: mark surveyed immediately
                if let Ok((mut star_system, attrs, _sys_pos)) = systems.get_mut(target_system) {
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

                    // Check for hostile presence at this system
                    let has_hostile = hostiles.iter().any(|h| h.system == target_system);
                    if has_hostile {
                        events.write(GameEvent {
                            timestamp: clock.elapsed,
                            kind: GameEventKind::HostileDetected,
                            description: format!(
                                "Warning: Hostile presence detected at {}!",
                                system_name,
                            ),
                            related_system: Some(target_system),
                        });
                    }

                    // Roll an exploration event
                    let event = roll_exploration_event(&mut rng);
                    apply_exploration_event(
                        &event,
                        &system_name,
                        &ship,
                        &mut ship_hp,
                        attrs,
                        &mut rng,
                        clock.elapsed,
                        target_system,
                        &mut events,
                    );
                }
            }

            *state = ShipState::Docked {
                system: target_system,
            };
        }
    }
}

/// #103: Deliver survey results when an FTL ship carrying survey data docks
/// at the player's StationedAt system.
pub fn deliver_survey_results(
    mut commands: Commands,
    clock: Res<GameClock>,
    ships: Query<(Entity, &Ship, &ShipState, &SurveyData)>,
    mut systems: Query<(&mut StarSystem, &Position), Without<Ship>>,
    player_q: Query<&StationedAt, With<Player>>,
    mut empire_q: Query<&mut KnowledgeStore, With<PlayerEmpire>>,
    mut events: MessageWriter<GameEvent>,
) {
    let player_system = match player_q.iter().next() {
        Some(s) => s.system,
        None => return,
    };

    for (ship_entity, ship, state, survey_data) in &ships {
        let ShipState::Docked { system: docked_at } = state else {
            continue;
        };

        if *docked_at != player_system {
            continue;
        }

        // Ship is docked at the player's system — deliver results
        let target = survey_data.target_system;

        // Mark the target system as surveyed and update KnowledgeStore
        if let Ok((mut star_system, pos)) = systems.get_mut(target) {
            star_system.surveyed = true;
            info!(
                "Survey data delivered: {} marked as surveyed (delivered by {})",
                survey_data.system_name, ship.name
            );

            // Update KnowledgeStore
            if let Ok(mut store) = empire_q.single_mut() {
                store.update(SystemKnowledge {
                    system: target,
                    observed_at: survey_data.surveyed_at,
                    received_at: clock.elapsed,
                    data: SystemSnapshot {
                        name: star_system.name.clone(),
                        position: pos.as_array(),
                        surveyed: true,
                        ..default()
                    },
                });
            }
        }

        // Publish GameEvent
        events.write(GameEvent {
            timestamp: clock.elapsed,
            kind: GameEventKind::SurveyComplete,
            description: format!(
                "{} delivered survey data for {} (surveyed at t={})",
                ship.name, survey_data.system_name, survey_data.surveyed_at
            ),
            related_system: Some(target),
        });

        // Clear survey data from the ship
        commands.entity(ship_entity).remove::<SurveyData>();
    }
}

/// Apply an exploration event's effects and log it.
fn apply_exploration_event(
    event: &ExplorationEvent,
    system_name: &str,
    ship: &Ship,
    ship_hp: &mut ShipHitpoints,
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
            let damage = ship_hp.hull_max * damage_pct;
            ship_hp.hull = (ship_hp.hull - damage).max(1.0);
            events.write(GameEvent {
                timestamp,
                kind: GameEventKind::SurveyDiscovery,
                description: format!(
                    "Danger at {}! Ship {} took {:.0} damage ({:.0}% hull) from hazardous anomaly",
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
/// establishes a colony on the first habitable planet and despawns the colony ship.
pub fn process_settling(
    mut commands: Commands,
    clock: Res<GameClock>,
    ships: Query<(Entity, &Ship, &ShipState)>,
    systems: Query<&StarSystem>,
    planet_query: Query<(Entity, &crate::galaxy::Planet, &SystemAttributes)>,
    existing_colonies: Query<&Colony>,
    existing_stockpiles: Query<&ResourceStockpile, With<StarSystem>>,
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
            let Ok(star_system) = systems.get(system_entity) else {
                continue;
            };

            // Check if any planet in this system already has a colony
            let already_colonized = existing_colonies.iter().any(|c| {
                planet_query.get(c.planet).ok().map(|(_, p, _)| p.system) == Some(system_entity)
            });

            if already_colonized {
                info!("System {} is already colonized, settling aborted", star_system.name);
                commands.entity(ship_entity).despawn();
                continue;
            }

            // Find the first habitable planet in this system
            let target_planet = planet_query.iter().find(|(_, p, attrs)| {
                p.system == system_entity && attrs.habitability != Habitability::GasGiant
            });

            let Some((planet_entity, _, attrs)) = target_planet else {
                info!("Colony Ship {} found no habitable planet at {}", ship.name, star_system.name);
                commands.entity(ship_entity).despawn();
                continue;
            };

            let system_name = star_system.name.clone();
            let minerals_rate = resource_production_rate(attrs.mineral_richness);
            let energy_rate = resource_production_rate(attrs.energy_potential);
            let research_rate = resource_production_rate(attrs.research_potential);
            let num_slots = attrs.max_building_slots as usize;

            commands.spawn((
                Colony {
                    planet: planet_entity,
                    population: 10.0,
                    growth_rate: 0.005,
                },
                Production {
                    minerals_per_hexadies: crate::modifier::ModifiedValue::new(minerals_rate),
                    energy_per_hexadies: crate::modifier::ModifiedValue::new(energy_rate),
                    research_per_hexadies: crate::modifier::ModifiedValue::new(research_rate),
                    food_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                },
                BuildQueue {
                    queue: Vec::new(),
                },
                Buildings {
                    slots: vec![None; num_slots],
                },
                BuildingQueue::default(),
                ProductionFocus::default(),
                MaintenanceCost::default(),
                FoodConsumption::default(),
            ));

            // Add ResourceStockpile and ResourceCapacity to the StarSystem if not already present
            if existing_stockpiles.get(system_entity).is_err() {
                commands.entity(system_entity).insert((
                    ResourceStockpile {
                        minerals: Amt::units(100),
                        energy: Amt::units(100),
                        research: Amt::ZERO,
                        food: Amt::units(50),
                        authority: Amt::ZERO,
                    },
                    ResourceCapacity::default(),
                ));
            }

            events.write(GameEvent {
                timestamp: clock.elapsed,
                kind: GameEventKind::ColonyEstablished,
                description: format!("Colony established at {}", system_name),
                related_system: Some(system_entity),
            });

            info!("Colony established at {} (M:{}/E:{}/R:{} per sd)", system_name, minerals_rate, energy_rate, research_rate);

            // Consume the colony ship
            commands.entity(ship_entity).despawn();
        }
    }
}

// --- Colony ship arrival (#20) ---

pub fn resource_production_rate(level: ResourceLevel) -> crate::amount::Amt {
    use crate::amount::Amt;
    match level {
        ResourceLevel::Rich => Amt::units(8),
        ResourceLevel::Moderate => Amt::units(5),
        ResourceLevel::Poor => Amt::units(2),
        ResourceLevel::None => Amt::ZERO,
    }
}

// --- Pending ship command processing (#33) ---

/// Processes pending ship commands that have arrived after communication delay.
/// #45: Uses GlobalParams for tech bonuses
/// #46: Checks for port facility at origin system
pub fn process_pending_ship_commands(
    mut commands: Commands,
    clock: Res<GameClock>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    pending: Query<(Entity, &PendingShipCommand)>,
    mut ships: Query<(&mut Ship, &mut ShipState, &Position)>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
    colonies: Query<(&crate::colony::Colony, &crate::colony::Buildings)>,
    planets: Query<&crate::galaxy::Planet>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };
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
            ShipCommand::MoveTo { destination } => {
                let dest = *destination;
                let Ok((dest_star, dest_pos)) = systems.get(dest) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                let Ok((_, origin_pos)) = systems.get(docked_system) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                // Try FTL first, fall back to sublight
                let origin_has_port = colonies.iter().any(|(col, bldgs)| col.system(&planets) == Some(docked_system) && bldgs.has_port());
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
                    origin_has_port,
                ) {
                    Ok(()) => {
                        info!(
                            "Remote move command executed: {} FTL jumping to {}",
                            ship.name, dest_star.name,
                        );
                    }
                    Err(_) => {
                        // Fall back to sublight
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
                            "Remote move command executed: {} sub-light to {}",
                            ship.name, dest_star.name,
                        );
                    }
                }
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
            ShipCommand::Colonize => {
                if !design_can_colonize(&ship.design_id) {
                    info!(
                        "Remote colonize command for {} failed: not a colony ship",
                        ship.name,
                    );
                } else {
                    *state = ShipState::Settling {
                        system: docked_system,
                        started_at: clock.elapsed,
                        completes_at: clock.elapsed + SETTLING_DURATION_HEXADIES,
                    };
                    info!(
                        "Remote colonize command executed: {} settling at docked system",
                        ship.name,
                    );
                }
            }
        }

        commands.entity(cmd_entity).despawn();
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

// --- Command queue processing (#34) ---

/// #45: Uses GlobalParams for tech bonuses
/// #46: Checks for port facility at origin system
/// #108: Unified MoveTo with auto-route planning (FTL chain > FTL direct > SubLight)
pub fn process_command_queue(
    clock: Res<GameClock>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    mut ships: Query<(Entity, &Ship, &mut ShipState, &mut CommandQueue, &Position)>,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    colonies: Query<(&crate::colony::Colony, &crate::colony::Buildings)>,
    planets: Query<&crate::galaxy::Planet>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };
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
            QueuedCommand::MoveTo { system: target } => {
                let Ok((_target_entity, target_star, target_pos)) = systems.get(target) else {
                    warn!("Queued MoveTo target no longer exists");
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };

                // Already at target?
                if docked_system == target {
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                }

                let Ok((_, _, origin_pos)) = systems.get(docked_system) else {
                    warn!("Queue: Origin system no longer exists");
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };

                let origin_has_port = colonies.iter().any(|(col, bldgs)| col.system(&planets) == Some(docked_system) && bldgs.has_port());
                let port_range_bonus = if origin_has_port { PORT_FTL_RANGE_BONUS_LY } else { 0.0 };
                let effective_ftl_range = ship.ftl_range + global_params.ftl_range_bonus + port_range_bonus;

                // Try FTL route planning first
                if effective_ftl_range > 0.0 {
                    if let Some(route) = plan_ftl_route(origin_pos.as_array(), target, effective_ftl_range, &systems) {
                        // Execute first hop
                        let first_hop = route[0];
                        let Ok((_, first_star, first_pos)) = systems.get(first_hop) else {
                            warn!("Queue: FTL route hop no longer exists");
                            queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                            continue;
                        };
                        match start_ftl_travel_with_bonus(
                            &mut state,
                            ship,
                            docked_system,
                            first_hop,
                            origin_pos,
                            first_pos,
                            clock.elapsed,
                            global_params.ftl_range_bonus,
                            global_params.ftl_speed_multiplier,
                            origin_has_port,
                        ) {
                            Ok(()) => {
                                // Prepend remaining hops as MoveTo commands
                                for (i, &hop) in route[1..].iter().enumerate().rev() {
                                    queue.commands.insert(0, QueuedCommand::MoveTo { system: hop });
                                }
                                info!(
                                    "Queue: Ship {} FTL jumping to {} (route: {} hops)",
                                    ship.name, first_star.name, route.len()
                                );
                                continue;
                            }
                            Err(e) => {
                                info!("Queue: FTL route first hop failed for {}: {}, falling back to sublight", ship.name, e);
                            }
                        }
                    }
                }

                // Try hybrid route: FTL to nearest reachable surveyed system to target, then sublight
                if effective_ftl_range > 0.0 {
                    let origin_arr = origin_pos.as_array();
                    let target_arr = target_pos.as_array();
                    let mut best_waypoint: Option<(Entity, [f64; 3], f64)> = None;

                    for (wp_entity, wp_star, wp_pos) in systems.iter() {
                        if !wp_star.surveyed { continue; }
                        let wp_arr = wp_pos.as_array();
                        // Must be reachable via FTL from current position (direct or chain)
                        if plan_ftl_route(origin_arr, wp_entity, effective_ftl_range, &systems).is_none() {
                            continue;
                        }
                        // Calculate total travel time: FTL to waypoint + sublight to target
                        let ftl_dist = distance_ly_arr(origin_arr, wp_arr);
                        let ftl_time = (ftl_dist * HEXADIES_PER_YEAR as f64 / (INITIAL_FTL_SPEED_C * global_params.ftl_speed_multiplier)).ceil();
                        let sl_dist = distance_ly_arr(wp_arr, target_arr);
                        let sl_speed = ship.sublight_speed + global_params.sublight_speed_bonus;
                        let sl_time = if sl_speed > 0.0 { (sl_dist * HEXADIES_PER_YEAR as f64 / sl_speed).ceil() } else { f64::MAX };
                        let total = ftl_time + sl_time;

                        // Must be faster than direct sublight
                        let direct_sl_time = {
                            let d = distance_ly_arr(origin_arr, target_arr);
                            if sl_speed > 0.0 { (d * HEXADIES_PER_YEAR as f64 / sl_speed).ceil() } else { f64::MAX }
                        };
                        if total >= direct_sl_time { continue; }

                        match &best_waypoint {
                            Some((_, _, best_total)) if total >= *best_total => {}
                            _ => { best_waypoint = Some((wp_entity, wp_arr, total)); }
                        }
                    }

                    if let Some((wp_entity, _, _)) = best_waypoint {
                        // FTL to waypoint, then sublight to final target
                        queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
                        queue.commands.insert(0, QueuedCommand::MoveTo { system: wp_entity });
                        info!("Queue: Ship {} hybrid route — FTL to waypoint, then sublight to {}", ship.name, target_star.name);
                        continue;
                    }
                }

                // Fall back to sublight direct
                start_sublight_travel_with_bonus(
                    &mut state,
                    origin_pos,
                    ship,
                    *target_pos,
                    Some(target),
                    clock.elapsed,
                    global_params.sublight_speed_bonus,
                );
                info!("Queue: Ship {} sub-light moving to {}", ship.name, target_star.name);
            }
            QueuedCommand::Survey { system: target } => {
                let Ok((_target_entity, target_star, target_pos)) = systems.get(target) else {
                    warn!("Queued Survey target no longer exists");
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };
                // #101: If not docked at the target system, auto-insert a move command
                if docked_system != target {
                    // Re-insert the survey command after the move
                    queue.commands.insert(0, QueuedCommand::Survey { system: target });
                    queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
                    info!("Queue: Ship {} not at target, auto-inserting move before survey of {}", ship.name, target_star.name);
                    continue;
                }
                let origin = Position::from(ship_pos.as_array());
                match start_survey_with_bonus(
                    &mut state,
                    ship,
                    target,
                    &origin,
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
                queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
            }
            QueuedCommand::Colonize { system: target } => {
                let Ok((_target_entity, target_star, _target_pos)) = systems.get(target) else {
                    warn!("Queued Colonize target no longer exists");
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };
                // #101: If not docked at the target system, auto-insert a move command
                if docked_system != target {
                    // Re-insert the colonize command after the move
                    queue.commands.insert(0, QueuedCommand::Colonize { system: target });
                    queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
                    info!("Queue: Ship {} not at target, auto-inserting move before colonize of {}", ship.name, target_star.name);
                    continue;
                }
                // #102: Start settling at the docked system
                if !design_can_colonize(&ship.design_id) {
                    warn!("Queue: Ship {} cannot colonize (not a colony ship)", ship.name);
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                }
                *state = ShipState::Settling {
                    system: docked_system,
                    started_at: clock.elapsed,
                    completes_at: clock.elapsed + SETTLING_DURATION_HEXADIES,
                };
                info!(
                    "Queue: Ship {} colonizing {}",
                    ship.name, target_star.name
                );
                queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
            }
        }
    }
}

// --- Combat resolution (#55, #97) ---

/// Hit chance: precision * track / (track + evasion)
fn hit_chance(weapon: &crate::ship_design::WeaponStats, target_evasion: f64) -> f64 {
    weapon.precision * (weapon.track / (weapon.track + target_evasion))
}

/// Apply weapon damage to a hostile (single HP pool).
fn apply_damage_to_hostile(hostile_hp: &mut f64, weapon: &crate::ship_design::WeaponStats, rng: &mut impl Rng) {
    let dmg = (weapon.hull_damage + weapon.hull_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
    *hostile_hp -= dmg;
}

/// Apply damage through 3-layer HP: shield → armor → hull.
fn apply_damage_to_ship(hp: &mut ShipHitpoints, weapon: &crate::ship_design::WeaponStats, rng: &mut impl Rng) {
    // Shield phase
    if hp.shield > 0.0 && rng.random::<f64>() >= weapon.shield_piercing {
        let dmg = (weapon.shield_damage + weapon.shield_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
        hp.shield = (hp.shield - dmg).max(0.0);
        return; // damage absorbed by shield
    }

    // Armor phase
    if hp.armor > 0.0 && rng.random::<f64>() >= weapon.armor_piercing {
        let dmg = (weapon.armor_damage + weapon.armor_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
        hp.armor = (hp.armor - dmg).max(0.0);
        return; // damage absorbed by armor
    }

    // Hull phase
    let dmg = (weapon.hull_damage + weapon.hull_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
    hp.hull = (hp.hull - dmg).max(0.0);
}

/// Apply flat hostile damage through 3-layer HP (simplified for hostile attacks).
fn apply_flat_damage_to_ship(hp: &mut ShipHitpoints, damage: f64) {
    let mut remaining = damage;

    // Shield absorbs first
    if hp.shield > 0.0 {
        let absorbed = remaining.min(hp.shield);
        hp.shield -= absorbed;
        remaining -= absorbed;
    }

    // Armor absorbs next
    if remaining > 0.0 && hp.armor > 0.0 {
        let absorbed = remaining.min(hp.armor);
        hp.armor -= absorbed;
        remaining -= absorbed;
    }

    // Hull takes the rest
    if remaining > 0.0 {
        hp.hull = (hp.hull - remaining).max(0.0);
    }
}

/// Resolves combat between player ships and hostile presences at star systems.
/// Combat turns per hexadies: 12. Uses WeaponStats from equipped modules.
pub fn resolve_combat(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<crate::colony::LastProductionTick>,
    mut ships: Query<(Entity, &Ship, &mut ShipHitpoints, &ShipModifiers, &ShipState)>,
    mut hostiles: Query<(Entity, &mut HostilePresence)>,
    module_registry: Res<ModuleRegistry>,
    systems: Query<&StarSystem>,
    mut events: MessageWriter<GameEvent>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let combat_turns = (delta * 12) as u32;
    let mut rng = rand::rng();

    // Collect hostile systems first to avoid borrow issues
    let hostile_data: Vec<(Entity, Entity, f64, f64, f64, crate::galaxy::HostileType, f64)> = hostiles
        .iter()
        .map(|(e, h)| (e, h.system, h.strength, h.hp, h.max_hp, h.hostile_type, h.evasion))
        .collect();

    for (hostile_entity, system_entity, _hostile_strength, _hostile_hp, _hostile_max_hp, _hostile_type, hostile_evasion) in &hostile_data {
        let system_name = systems
            .get(*system_entity)
            .map(|s| s.name.clone())
            .unwrap_or_default();

        // Find all player ships docked at this system
        let docked_ships: Vec<Entity> = ships
            .iter()
            .filter_map(|(entity, _ship, _hp, _mods, state)| {
                if let ShipState::Docked { system } = state {
                    if *system == *system_entity {
                        return Some(entity);
                    }
                }
                None
            })
            .collect();

        if docked_ships.is_empty() {
            continue;
        }

        // --- Player ships attack hostile ---
        // Collect weapon data for each ship
        struct ShipWeaponData {
            entity: Entity,
            weapons: Vec<crate::ship_design::WeaponStats>,
        }
        let mut ship_weapons: Vec<ShipWeaponData> = Vec::new();
        for &ship_entity in &docked_ships {
            if let Ok((_e, ship, _hp, _mods, _state)) = ships.get(ship_entity) {
                let mut weapons = Vec::new();
                for equipped in &ship.modules {
                    if let Some(module_def) = module_registry.modules.get(&equipped.module_id) {
                        if let Some(weapon) = &module_def.weapon {
                            weapons.push(weapon.clone());
                        }
                    }
                }
                ship_weapons.push(ShipWeaponData { entity: ship_entity, weapons });
            }
        }

        // Apply weapon damage to hostile
        let Ok((_he, mut hostile)) = hostiles.get_mut(*hostile_entity) else {
            continue;
        };

        for sw in &ship_weapons {
            for weapon in &sw.weapons {
                let shots = if weapon.cooldown > 0 { combat_turns / weapon.cooldown as u32 } else { combat_turns };
                for _ in 0..shots {
                    let chance = hit_chance(weapon, *hostile_evasion);
                    if rng.random::<f64>() < chance {
                        apply_damage_to_hostile(&mut hostile.hp, weapon, &mut rng);
                    }
                }
            }
        }

        // Check if hostile is destroyed
        if hostile.hp <= 0.0 {
            commands.entity(*hostile_entity).despawn();
            events.write(GameEvent {
                timestamp: clock.elapsed,
                kind: GameEventKind::CombatVictory,
                description: format!(
                    "Victory! Hostile {:?} at {} has been defeated",
                    hostile.hostile_type, system_name
                ),
                related_system: Some(*system_entity),
            });
            continue;
        }

        let hostile_str = hostile.strength;
        let hostile_tp = hostile.hostile_type;
        // Drop the mutable borrow on hostile before accessing ships mutably
        drop(hostile);

        // --- Hostile attacks player ships ---
        // Hostile deals strength damage per combat turn, distributed evenly
        if hostile_str > 0.0 && !docked_ships.is_empty() {
            let total_damage = hostile_str * combat_turns as f64;
            let damage_per_ship = total_damage / docked_ships.len() as f64;
            let mut destroyed_ships: Vec<(Entity, String)> = Vec::new();

            for &ship_entity in &docked_ships {
                if let Ok((_e, ship, mut hp, _mods, _state)) = ships.get_mut(ship_entity) {
                    apply_flat_damage_to_ship(&mut hp, damage_per_ship);
                    if hp.hull <= 0.0 {
                        destroyed_ships.push((ship_entity, ship.name.clone()));
                    }
                }
            }

            for (entity, name) in &destroyed_ships {
                commands.entity(*entity).despawn();
                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::CombatDefeat,
                    description: format!("{} destroyed in combat at {}", name, system_name),
                    related_system: Some(*system_entity),
                });
            }

            // Check if all player ships at this system are destroyed
            let surviving = docked_ships.len() - destroyed_ships.len();
            if surviving == 0 {
                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::CombatDefeat,
                    description: format!(
                        "All ships destroyed by hostile {:?} at {}",
                        hostile_tp,
                        system_name
                    ),
                    related_system: Some(*system_entity),
                });
            }
        }
    }
}

/// Sync ShipHitpoints max values from ShipModifiers.
/// Only updates when the modifier-computed values differ from current values.
pub fn sync_ship_hitpoints(
    mut ships: Query<(&ShipModifiers, &mut ShipHitpoints)>,
) {
    for (mods, mut hp) in &mut ships {
        let new_armor_max = mods.armor_max.final_value().to_f64();
        let new_shield_max = mods.shield_max.final_value().to_f64();
        let new_shield_regen = mods.shield_regen.final_value().to_f64();
        // Only update if values actually changed from modifiers
        if (hp.armor_max - new_armor_max).abs() > f64::EPSILON
            || (hp.shield_max - new_shield_max).abs() > f64::EPSILON
            || (hp.shield_regen - new_shield_regen).abs() > f64::EPSILON
        {
            hp.armor_max = new_armor_max;
            hp.shield_max = new_shield_max;
            hp.shield_regen = new_shield_regen;
            // Clamp current values to new max
            hp.armor = hp.armor.min(hp.armor_max);
            hp.shield = hp.shield.min(hp.shield_max);
        }
    }
}

/// Regenerate shields over time.
pub fn tick_shield_regen(
    clock: Res<GameClock>,
    last_tick: Res<crate::colony::LastProductionTick>,
    mut ships: Query<&mut ShipHitpoints>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as f64;
    for mut hp in &mut ships {
        if hp.shield < hp.shield_max && hp.shield_regen > 0.0 {
            hp.shield = (hp.shield + hp.shield_regen * d).min(hp.shield_max);
        }
    }
}

/// Repair armor and hull for ships docked at a system with a Port building.
/// Repair rate: 5.0 HP per hexadies.
const REPAIR_RATE_PER_HEXADIES: f64 = 5.0;

pub fn tick_ship_repair(
    clock: Res<GameClock>,
    last_tick: Res<crate::colony::LastProductionTick>,
    mut ships: Query<(&ShipState, &mut ShipHitpoints)>,
    colonies: Query<(&Colony, Option<&Buildings>)>,
    planets: Query<&crate::galaxy::Planet>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let repair_amount = REPAIR_RATE_PER_HEXADIES * delta as f64;

    for (state, mut hp) in &mut ships {
        let ShipState::Docked { system } = state else {
            continue;
        };

        // Check if the system has a colony with a Port
        let has_port = colonies.iter().any(|(col, bldgs)| {
            col.system(&planets) == Some(*system)
                && bldgs.map_or(false, |b| b.has_port())
        });

        if has_port {
            // Repair armor first, then hull
            if hp.armor < hp.armor_max {
                hp.armor = (hp.armor + repair_amount).min(hp.armor_max);
            }
            if hp.hull < hp.hull_max {
                hp.hull = (hp.hull + repair_amount).min(hp.hull_max);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;

    fn make_ship(design_id: &str) -> Ship {
        let preset = design_preset(design_id).unwrap_or(&EXPLORER_PRESET);
        Ship {
            name: "Test Ship".to_string(),
            design_id: preset.design_id.to_string(),
            hull_id: preset.hull_id.to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: preset.sublight_speed,
            ftl_range: preset.ftl_range,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }
    }

    #[test]
    fn start_sublight_sets_correct_arrival_time() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1"); // 0.5c
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
        let ship = make_ship("courier_mk1");
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
        let ship = make_ship("colony_ship_mk1");
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
        let ship = make_ship("colony_ship_mk1");
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
        let ship = make_ship("colony_ship_mk1");
        let mut state = ShipState::Docked { system };
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 0);
        assert_eq!(result, Err("Only Explorer ships can perform surveys"));
    }

    #[test]
    fn start_survey_rejects_non_docked() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("explorer_mk1");
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
        let ship = make_ship("explorer_mk1");
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
        let ship = make_ship("explorer_mk1");
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

    // --- #46: Port FTL tests ---

    #[test]
    fn start_ftl_with_port_reduces_travel_time() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1");
        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 10.0, y: 0.0, z: 0.0 };

        // Without port
        let mut state_no_port = ShipState::Docked { system: origin };
        let _ = start_ftl_travel_with_bonus(&mut state_no_port, &ship, origin, dest, &origin_pos, &dest_pos, 0, 0.0, 1.0, false);
        let time_no_port = match state_no_port {
            ShipState::InFTL { arrival_at, .. } => arrival_at,
            _ => panic!("Expected InFTL state"),
        };

        // With port
        let mut state_port = ShipState::Docked { system: origin };
        let _ = start_ftl_travel_with_bonus(&mut state_port, &ship, origin, dest, &origin_pos, &dest_pos, 0, 0.0, 1.0, true);
        let time_port = match state_port {
            ShipState::InFTL { arrival_at, .. } => arrival_at,
            _ => panic!("Expected InFTL state"),
        };

        // Port should reduce travel time by 20%
        assert!(time_port < time_no_port, "Port should reduce FTL travel time");
        let expected = (time_no_port as f64 * PORT_TRAVEL_TIME_FACTOR).ceil() as i64;
        assert_eq!(time_port, expected);
    }

    #[test]
    fn start_ftl_with_port_extends_range() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1"); // ftl_range = 15.0

        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 20.0, y: 0.0, z: 0.0 }; // 20 ly, beyond base 15 ly range

        // Without port: should fail
        let mut state = ShipState::Docked { system: origin };
        let result = start_ftl_travel_with_bonus(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0, 0.0, 1.0, false);
        assert_eq!(result, Err("Destination is beyond FTL range"));

        // With port: +10 ly range, so 25 ly total, should succeed
        let mut state = ShipState::Docked { system: origin };
        let result = start_ftl_travel_with_bonus(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0, 0.0, 1.0, true);
        assert!(result.is_ok(), "Port should extend FTL range by {} ly", PORT_FTL_RANGE_BONUS_LY);
    }

    // --- #51: Ship maintenance cost tests ---

    #[test]
    fn ship_maintenance_costs() {
        assert_eq!(ship_maintenance_cost("explorer_mk1"), Amt::new(0, 500));
        assert_eq!(ship_maintenance_cost("colony_ship_mk1"), Amt::units(1));
        assert_eq!(ship_maintenance_cost("courier_mk1"), Amt::new(0, 300));
    }

    // --- #54: Fleet tests ---

    #[test]
    fn fleet_speed_is_min_of_members() {
        let mut world = World::new();
        let ship_a = world.spawn(Ship {
            name: "Fast".to_string(),
            design_id: "courier_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.85,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }).id();
        let ship_b = world.spawn(Ship {
            name: "Slow".to_string(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "freighter".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 30.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }).id();

        let fleet = Fleet {
            name: "Test Fleet".to_string(),
            members: vec![ship_a, ship_b],
            flagship: ship_a,
        };

        let mut system_state = bevy::ecs::system::SystemState::<Query<&Ship>>::new(&mut world);
        let ships = system_state.get(&world);
        let speed = fleet.speed(&ships);
        assert!((speed - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn fleet_ftl_range_is_min_of_members() {
        let mut world = World::new();
        let ship_a = world.spawn(Ship {
            name: "Short Range".to_string(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "freighter".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }).id();
        let ship_b = world.spawn(Ship {
            name: "Long Range".to_string(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "freighter".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 30.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }).id();

        let fleet = Fleet {
            name: "Test Fleet".to_string(),
            members: vec![ship_a, ship_b],
            flagship: ship_a,
        };

        let mut system_state = bevy::ecs::system::SystemState::<Query<&Ship>>::new(&mut world);
        let ships = system_state.get(&world);
        let range = fleet.ftl_range(&ships);
        assert!((range - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fleet_creation_adds_membership() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let ship_a = world.spawn((
            make_ship("explorer_mk1"),
            ShipState::Docked { system },
            pos,
            CommandQueue::default(),
            Cargo::default(),
        )).id();
        let ship_b = world.spawn((
            make_ship("colony_ship_mk1"),
            ShipState::Docked { system },
            pos,
            CommandQueue::default(),
            Cargo::default(),
        )).id();

        let members = vec![ship_a, ship_b];
        let fleet_entity = {
            let mut commands = world.commands();
            let e = create_fleet(&mut commands, "Alpha Fleet".to_string(), members, ship_a);
            e
        };
        world.flush();

        let fleet = world.get::<Fleet>(fleet_entity).expect("Fleet should exist");
        assert_eq!(fleet.name, "Alpha Fleet");
        assert_eq!(fleet.members.len(), 2);
        assert_eq!(fleet.flagship, ship_a);

        let membership_a = world.get::<FleetMembership>(ship_a).expect("Ship A should have FleetMembership");
        assert_eq!(membership_a.fleet, fleet_entity);

        let membership_b = world.get::<FleetMembership>(ship_b).expect("Ship B should have FleetMembership");
        assert_eq!(membership_b.fleet, fleet_entity);
    }

    #[test]
    fn fleet_dissolution_removes_membership() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let ship_a = world.spawn((
            make_ship("explorer_mk1"),
            ShipState::Docked { system },
            pos,
            CommandQueue::default(),
            Cargo::default(),
        )).id();
        let ship_b = world.spawn((
            make_ship("colony_ship_mk1"),
            ShipState::Docked { system },
            pos,
            CommandQueue::default(),
            Cargo::default(),
        )).id();

        // Create fleet
        let members = vec![ship_a, ship_b];
        let fleet_entity = {
            let mut commands = world.commands();
            create_fleet(&mut commands, "Alpha Fleet".to_string(), members, ship_a)
        };
        world.flush();

        // Verify membership exists
        assert!(world.get::<FleetMembership>(ship_a).is_some());
        assert!(world.get::<FleetMembership>(ship_b).is_some());

        // Dissolve fleet
        let fleet_members = world.get::<Fleet>(fleet_entity).unwrap().members.clone();
        let fleet_flagship = world.get::<Fleet>(fleet_entity).unwrap().flagship;
        let fleet_data = Fleet {
            name: "Alpha Fleet".to_string(),
            members: fleet_members,
            flagship: fleet_flagship,
        };
        {
            let mut commands = world.commands();
            dissolve_fleet(&mut commands, fleet_entity, &fleet_data);
        }
        world.flush();

        // Verify membership removed
        assert!(world.get::<FleetMembership>(ship_a).is_none());
        assert!(world.get::<FleetMembership>(ship_b).is_none());

        // Fleet entity should be despawned
        assert!(world.get_entity(fleet_entity).is_err());
    }

    #[test]
    fn build_cost_returns_expected_values() {
        assert_eq!(ship_build_cost("explorer_mk1"), (Amt::units(200), Amt::units(100)));
        assert_eq!(ship_build_cost("colony_ship_mk1"), (Amt::units(500), Amt::units(300)));
        assert_eq!(ship_build_cost("courier_mk1"), (Amt::units(100), Amt::units(50)));
    }

    #[test]
    fn scrap_refund_is_half_build_cost() {
        for design_id in ["explorer_mk1", "colony_ship_mk1", "courier_mk1"] {
            let (bm, be) = ship_build_cost(design_id);
            let (rm, re) = ship_scrap_refund(design_id);
            assert_eq!(rm, Amt::milli(bm.raw() / 2));
            assert_eq!(re, Amt::milli(be.raw() / 2));
        }
    }

    // --- #102: Survey requires docked at target system ---

    #[test]
    fn start_survey_rejects_wrong_system() {
        let mut world = World::new();
        let system_a = world.spawn_empty().id();
        let system_b = world.spawn_empty().id();
        let ship = make_ship("explorer_mk1");
        let mut state = ShipState::Docked { system: system_a };
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let result = start_survey(&mut state, &ship, system_b, &pos, &pos, 0);
        assert_eq!(result, Err("Ship must be docked at the target system to survey it"));
    }

    #[test]
    fn start_survey_same_system_succeeds() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("explorer_mk1");
        let mut state = ShipState::Docked { system };
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 0);
        assert!(result.is_ok());
        assert!(matches!(state, ShipState::Surveying { .. }));
    }

    // --- #101: Auto-insert movement for remote Survey/Colonize ---

    #[test]
    fn command_queue_survey_auto_inserts_move_when_not_at_target() {
        let mut world = World::new();
        let system_a = world.spawn_empty().id();
        let system_b = world.spawn_empty().id();
        // Ship is docked at system_a, survey targets system_b
        let mut queue = CommandQueue {
            commands: vec![QueuedCommand::Survey {
                system: system_b,
            }],
            ..Default::default()
        };
        let state = ShipState::Docked { system: system_a };

        // Simulate what process_command_queue does:
        // It checks if docked_system != target, and if so, inserts move + re-queues survey
        let docked_system = match &state {
            ShipState::Docked { system } => *system,
            _ => panic!("Expected Docked"),
        };
        let next = queue.commands.remove(0);
        match next {
            QueuedCommand::Survey { system: target } => {
                assert_ne!(docked_system, target);
                // Auto-insert: move to target, then re-queue survey
                queue.commands.insert(0, QueuedCommand::Survey { system: target });
                queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
            }
            _ => panic!("Expected Survey command"),
        }

        // Verify: queue should now be [MoveTo, Survey]
        assert_eq!(queue.commands.len(), 2);
        assert!(matches!(queue.commands[0], QueuedCommand::MoveTo { .. }));
        assert!(matches!(queue.commands[1], QueuedCommand::Survey { .. }));
    }

    #[test]
    fn command_queue_colonize_auto_inserts_move_when_not_at_target() {
        let mut world = World::new();
        let system_a = world.spawn_empty().id();
        let system_b = world.spawn_empty().id();
        let mut queue = CommandQueue {
            commands: vec![QueuedCommand::Colonize {
                system: system_b,
            }],
            ..Default::default()
        };
        let state = ShipState::Docked { system: system_a };

        let docked_system = match &state {
            ShipState::Docked { system } => *system,
            _ => panic!("Expected Docked"),
        };
        let next = queue.commands.remove(0);
        match next {
            QueuedCommand::Colonize { system: target } => {
                assert_ne!(docked_system, target);
                // Auto-insert: move to target, then re-queue colonize
                queue.commands.insert(0, QueuedCommand::Colonize { system: target });
                queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
            }
            _ => panic!("Expected Colonize command"),
        }

        // Should be [MoveTo, Colonize] — route planning (FTL vs sublight) is handled by process_command_queue
        assert_eq!(queue.commands.len(), 2);
        assert!(matches!(queue.commands[0], QueuedCommand::MoveTo { .. }));
        assert!(matches!(queue.commands[1], QueuedCommand::Colonize { .. }));
    }
}
