use bevy::prelude::*;

use std::path::Path;

use crate::amount::{Amt, SignedAmt};
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{StarSystem, SystemAttributes, Sovereignty};
use crate::modifier::{ModifiedValue, Modifier};
use crate::scripting::building_api::{parse_building_definitions, BuildingRegistry};
use crate::ship::{spawn_ship, Owner, Ship, ShipState, ShipType};
use crate::time_system::GameClock;

pub struct ColonyPlugin;

#[derive(Resource, Default)]
pub struct LastProductionTick(pub i64);

impl Plugin for ColonyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastProductionTick>()
            .init_resource::<BuildingRegistry>()
            .insert_resource(AuthorityParams::default())
            .insert_resource(ConstructionParams::default())
            .add_systems(
                Startup,
                (
                    load_building_registry.after(crate::scripting::init_scripting),
                    spawn_capital_colony.after(crate::galaxy::generate_galaxy),
                ),
            )
            .add_systems(
                Update,
                (
                    tick_timed_effects,
                    tick_authority,
                    sync_building_modifiers,
                    sync_maintenance_modifiers,
                    sync_food_consumption,
                    tick_production,
                    tick_maintenance,
                    tick_population_growth,
                    tick_build_queue,
                    tick_building_queue,
                    advance_production_tick,
                )
                    .chain()
                    .after(crate::time_system::advance_game_time),
            )
            .add_systems(Update, update_sovereignty);
    }
}

#[derive(Component)]
pub struct Colony {
    pub system: Entity,
    pub population: f64,
    pub growth_rate: f64,
}

#[derive(Component)]
pub struct ResourceStockpile {
    pub minerals: Amt,
    pub energy: Amt,
    pub research: Amt,
    pub food: Amt,
    pub authority: Amt,
}

#[derive(Component)]
pub struct ResourceCapacity {
    pub minerals: Amt,
    pub energy: Amt,
    pub food: Amt,
    pub authority: Amt,
}

impl Default for ResourceCapacity {
    fn default() -> Self {
        Self {
            minerals: Amt::units(1000),
            energy: Amt::units(1000),
            food: Amt::units(500),
            authority: Amt::units(10000),
        }
    }
}

#[derive(Component)]
pub struct Production {
    pub minerals_per_hexadies: ModifiedValue,
    pub energy_per_hexadies: ModifiedValue,
    pub research_per_hexadies: ModifiedValue,
    pub food_per_hexadies: ModifiedValue,
}

/// Base authority produced per hexady by the capital colony.
pub const BASE_AUTHORITY_PER_HEXADIES: Amt = Amt::units(1);

/// Authority cost per hexady for each non-capital colony (empire scale cost).
pub const AUTHORITY_COST_PER_COLONY: Amt = Amt::new(0, 500);

/// Production efficiency multiplier applied to non-capital colonies when
/// the capital's authority stockpile is depleted.
/// 0.5 as fixed-point: Amt(500) means ×0.500
pub const AUTHORITY_DEFICIT_PENALTY: Amt = Amt::new(0, 500);

/// Configurable authority parameters. Tech effects can push modifiers to
/// adjust authority production or cost scaling.
#[derive(Resource)]
pub struct AuthorityParams {
    /// Authority produced per hexady by the capital colony. Base = 1.0
    pub production: ModifiedValue,
    /// Authority cost per hexady per non-capital colony. Base = 0.5
    pub cost_per_colony: ModifiedValue,
}

impl Default for AuthorityParams {
    fn default() -> Self {
        Self {
            production: ModifiedValue::new(BASE_AUTHORITY_PER_HEXADIES),
            cost_per_colony: ModifiedValue::new(AUTHORITY_COST_PER_COLONY),
        }
    }
}

/// Colony-level maintenance cost as a ModifiedValue (energy/hexady).
/// The sync_maintenance_modifiers system pushes building and ship maintenance
/// as base_add modifiers; tick_maintenance reads final_value().
#[derive(Component)]
pub struct MaintenanceCost {
    pub energy_per_hexadies: ModifiedValue,
}

impl Default for MaintenanceCost {
    fn default() -> Self {
        Self {
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
        }
    }
}

/// Colony-level food consumption as a ModifiedValue (food/hexady).
/// The sync_food_consumption system sets the base each tick based on population;
/// tech modifiers (e.g. Hydroponics -20%) stay attached as multiplier modifiers.
#[derive(Component)]
pub struct FoodConsumption {
    pub food_per_hexadies: ModifiedValue,
}

impl Default for FoodConsumption {
    fn default() -> Self {
        Self {
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        }
    }
}

/// Global construction cost/time modifiers. Base = 1.0 for all fields.
/// Techs push multiplier modifiers (e.g. -0.15 for "15% cheaper ships").
/// Effective cost = base_cost * modifier.final_value().
#[derive(Resource)]
pub struct ConstructionParams {
    pub ship_cost_modifier: ModifiedValue,
    pub building_cost_modifier: ModifiedValue,
    pub ship_build_time_modifier: ModifiedValue,
    pub building_build_time_modifier: ModifiedValue,
}

impl Default for ConstructionParams {
    fn default() -> Self {
        Self {
            ship_cost_modifier: ModifiedValue::new(Amt::units(1)),
            building_cost_modifier: ModifiedValue::new(Amt::units(1)),
            ship_build_time_modifier: ModifiedValue::new(Amt::units(1)),
            building_build_time_modifier: ModifiedValue::new(Amt::units(1)),
        }
    }
}

/// #29: Production focus weights for colony output
#[derive(Component)]
pub struct ProductionFocus {
    pub minerals_weight: Amt,
    pub energy_weight: Amt,
    pub research_weight: Amt,
}

impl Default for ProductionFocus {
    fn default() -> Self {
        Self {
            minerals_weight: Amt::units(1),
            energy_weight: Amt::units(1),
            research_weight: Amt::units(1),
        }
    }
}

impl ProductionFocus {
    pub fn balanced() -> Self {
        Self::default()
    }
    pub fn minerals() -> Self {
        Self {
            minerals_weight: Amt::units(2),
            energy_weight: Amt::new(0, 500),
            research_weight: Amt::new(0, 500),
        }
    }
    pub fn energy() -> Self {
        Self {
            minerals_weight: Amt::new(0, 500),
            energy_weight: Amt::units(2),
            research_weight: Amt::new(0, 500),
        }
    }
    pub fn research() -> Self {
        Self {
            minerals_weight: Amt::new(0, 500),
            energy_weight: Amt::new(0, 500),
            research_weight: Amt::units(2),
        }
    }

    pub fn label(&self) -> &'static str {
        if self.minerals_weight == Amt::units(1)
            && self.energy_weight == Amt::units(1)
            && self.research_weight == Amt::units(1)
        {
            "Balanced"
        } else if self.minerals_weight > Amt::new(1, 500) {
            "Minerals"
        } else if self.energy_weight > Amt::new(1, 500) {
            "Energy"
        } else if self.research_weight > Amt::new(1, 500) {
            "Research"
        } else {
            "Custom"
        }
    }
}

#[derive(Component)]
pub struct BuildQueue {
    pub queue: Vec<BuildOrder>,
}

pub struct BuildOrder {
    pub ship_type_name: String,
    pub minerals_cost: Amt,
    pub minerals_invested: Amt,
    pub energy_cost: Amt,
    pub energy_invested: Amt,
    /// #32: Total build time in hexadies
    pub build_time_total: i64,
    /// #32: Remaining build time in hexadies
    pub build_time_remaining: i64,
}

impl BuildOrder {
    pub fn is_complete(&self) -> bool {
        self.minerals_invested >= self.minerals_cost
            && self.energy_invested >= self.energy_cost
            && self.build_time_remaining <= 0
    }

    /// Returns the build time in hexadies for a given ship type name.
    pub fn build_time_for(ship_type_name: &str) -> i64 {
        match ship_type_name {
            "Explorer" => 60,
            "Colony Ship" => 120,
            "Courier" => 30,
            _ => 60,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingType {
    Mine,         // +3 minerals/sd
    PowerPlant,   // +3 energy/sd
    ResearchLab,  // +2 research/sd
    Shipyard,     // 2x build speed
    Port,         // Reduces FTL travel time from this system
    Farm,         // +5 food/hd
}

impl BuildingType {
    pub fn production_bonus(&self) -> (Amt, Amt, Amt, Amt) {
        // (minerals, energy, research, food) per hexadies
        match self {
            BuildingType::Mine => (Amt::units(3), Amt::ZERO, Amt::ZERO, Amt::ZERO),
            BuildingType::PowerPlant => (Amt::ZERO, Amt::units(3), Amt::ZERO, Amt::ZERO),
            BuildingType::ResearchLab => (Amt::ZERO, Amt::ZERO, Amt::units(2), Amt::ZERO),
            BuildingType::Shipyard => (Amt::ZERO, Amt::ZERO, Amt::ZERO, Amt::ZERO),
            BuildingType::Port => (Amt::ZERO, Amt::ZERO, Amt::ZERO, Amt::ZERO),
            BuildingType::Farm => (Amt::ZERO, Amt::ZERO, Amt::ZERO, Amt::units(5)),
        }
    }

    pub fn build_cost(&self) -> (Amt, Amt) {
        // (minerals, energy)
        match self {
            BuildingType::Mine => (Amt::units(150), Amt::units(50)),
            BuildingType::PowerPlant => (Amt::units(50), Amt::units(150)),
            BuildingType::ResearchLab => (Amt::units(100), Amt::units(100)),
            BuildingType::Shipyard => (Amt::units(300), Amt::units(200)),
            BuildingType::Port => (Amt::units(400), Amt::units(300)),
            BuildingType::Farm => (Amt::units(100), Amt::units(50)),
        }
    }

    pub fn build_time(&self) -> i64 {
        // hexadies to build
        match self {
            BuildingType::Mine => 10,
            BuildingType::PowerPlant => 10,
            BuildingType::ResearchLab => 15,
            BuildingType::Shipyard => 30,
            BuildingType::Port => 40,
            BuildingType::Farm => 20,
        }
    }

    /// Energy maintenance cost per hexadies (#51)
    pub fn maintenance_cost(&self) -> Amt {
        match self {
            BuildingType::Mine => Amt::new(0, 200),          // 0.2
            BuildingType::PowerPlant => Amt::ZERO,            // self-powered
            BuildingType::ResearchLab => Amt::new(0, 500),    // 0.5
            BuildingType::Shipyard => Amt::units(1),          // 1.0
            BuildingType::Port => Amt::new(0, 500),           // 0.5
            BuildingType::Farm => Amt::new(0, 300),           // 0.3
        }
    }

    /// Display name for the building type.
    pub fn name(&self) -> &'static str {
        match self {
            BuildingType::Mine => "Mine",
            BuildingType::PowerPlant => "PowerPlant",
            BuildingType::ResearchLab => "ResearchLab",
            BuildingType::Shipyard => "Shipyard",
            BuildingType::Port => "Port",
            BuildingType::Farm => "Farm",
        }
    }
}

#[derive(Component)]
pub struct Buildings {
    pub slots: Vec<Option<BuildingType>>, // None = empty slot
}

impl Buildings {
    /// #35: Check if any slot contains a Shipyard
    pub fn has_shipyard(&self) -> bool {
        self.slots.iter().any(|s| *s == Some(BuildingType::Shipyard))
    }

    /// #46: Check if any slot contains a Port
    pub fn has_port(&self) -> bool {
        self.slots.iter().any(|s| *s == Some(BuildingType::Port))
    }
}

#[derive(Component, Default)]
pub struct BuildingQueue {
    pub queue: Vec<BuildingOrder>,
}

pub struct BuildingOrder {
    pub building_type: BuildingType,
    pub target_slot: usize,
    pub minerals_remaining: Amt,
    pub energy_remaining: Amt,
    pub build_time_remaining: i64,
}

/// Load building definitions from Lua scripts into the BuildingRegistry.
/// Falls back to an empty registry if scripts are missing or fail to parse.
fn load_building_registry(
    engine: Res<crate::scripting::ScriptEngine>,
    mut registry: ResMut<BuildingRegistry>,
) {
    let building_dir = Path::new("scripts/buildings");
    if building_dir.exists() {
        match engine.load_directory(building_dir) {
            Err(e) => {
                warn!("Failed to load building scripts: {e}; building registry will be empty");
            }
            Ok(()) => match parse_building_definitions(engine.lua()) {
                Ok(defs) => {
                    let count = defs.len();
                    for def in defs {
                        registry.insert(def);
                    }
                    info!("Building registry loaded with {} definitions", count);
                }
                Err(e) => {
                    warn!("Failed to parse building definitions: {e}; building registry will be empty");
                }
            },
        }
    } else {
        info!("scripts/buildings directory not found; building registry will be empty");
    }
}

pub fn spawn_capital_colony(
    mut commands: Commands,
    query: Query<(Entity, &StarSystem, &SystemAttributes)>,
) {
    for (entity, system, attributes) in query.iter() {
        if system.is_capital {
            let num_slots = attributes.max_building_slots as usize;
            let mut slots = vec![None; num_slots];
            // Capital starts with 1 Mine, 1 PowerPlant, 1 Shipyard (#35), and 1 Farm (#72)
            if num_slots > 0 {
                slots[0] = Some(BuildingType::Mine);
            }
            if num_slots > 1 {
                slots[1] = Some(BuildingType::PowerPlant);
            }
            if num_slots > 2 {
                slots[2] = Some(BuildingType::Shipyard);
            }
            if num_slots > 3 {
                slots[3] = Some(BuildingType::Farm);
            }
            commands.spawn((
                Colony {
                    system: entity,
                    population: 100.0,
                    growth_rate: 0.01,
                },
                ResourceStockpile {
                    minerals: Amt::units(500),
                    energy: Amt::units(500),
                    research: Amt::ZERO,
                    food: Amt::units(200),
                    authority: Amt::ZERO,
                },
                ResourceCapacity::default(),
                Production {
                    minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
                    energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
                    research_per_hexadies: ModifiedValue::new(Amt::units(1)),
                    food_per_hexadies: ModifiedValue::new(Amt::units(5)),
                },
                BuildQueue {
                    queue: Vec::new(),
                },
                Buildings { slots },
                BuildingQueue::default(),
                ProductionFocus::default(),
                MaintenanceCost::default(),
                FoodConsumption::default(),
            ));
            info!("Capital colony spawned on {}", system.name);
            return;
        }
    }
    warn!("No capital star system found; capital colony not created");
}

/// Remove expired timed modifiers from all ModifiedValue-containing components.
/// Runs BEFORE sync_building_modifiers so that expired timed effects are cleaned
/// up before production values are recalculated.
pub fn tick_timed_effects(
    clock: Res<GameClock>,
    mut productions: Query<(Entity, &mut Production)>,
    mut maintenance_costs: Query<(Entity, &mut MaintenanceCost)>,
    mut food_consumptions: Query<(Entity, &mut FoodConsumption)>,
    mut authority_params: ResMut<AuthorityParams>,
    mut construction_params: ResMut<ConstructionParams>,
    mut event_system: ResMut<crate::event_system::EventSystem>,
) {
    let now = clock.elapsed;

    // Helper: drain expired modifiers and fire any on_expire_event via EventSystem
    fn drain_and_fire(
        mv: &mut ModifiedValue,
        now: i64,
        target: Option<Entity>,
        event_system: &mut crate::event_system::EventSystem,
    ) {
        let expired = mv.drain_expired(now);
        for m in &expired {
            if let Some(ref evt) = m.on_expire_event {
                info!(
                    "Modifier '{}' expired, triggering event: {}",
                    m.id, evt
                );
                event_system.fire_event(evt, target, now);
            }
        }
    }

    for (entity, mut prod) in &mut productions {
        drain_and_fire(&mut prod.minerals_per_hexadies, now, Some(entity), &mut event_system);
        drain_and_fire(&mut prod.energy_per_hexadies, now, Some(entity), &mut event_system);
        drain_and_fire(&mut prod.research_per_hexadies, now, Some(entity), &mut event_system);
        drain_and_fire(&mut prod.food_per_hexadies, now, Some(entity), &mut event_system);
    }
    for (entity, mut mc) in &mut maintenance_costs {
        drain_and_fire(&mut mc.energy_per_hexadies, now, Some(entity), &mut event_system);
    }
    for (entity, mut fc) in &mut food_consumptions {
        drain_and_fire(&mut fc.food_per_hexadies, now, Some(entity), &mut event_system);
    }
    drain_and_fire(&mut authority_params.production, now, None, &mut event_system);
    drain_and_fire(&mut authority_params.cost_per_colony, now, None, &mut event_system);
    drain_and_fire(&mut construction_params.ship_cost_modifier, now, None, &mut event_system);
    drain_and_fire(&mut construction_params.building_cost_modifier, now, None, &mut event_system);
    drain_and_fire(&mut construction_params.ship_build_time_modifier, now, None, &mut event_system);
    drain_and_fire(&mut construction_params.building_build_time_modifier, now, None, &mut event_system);
}

/// Synchronise building-slot bonuses as modifiers on the Production component.
/// For each occupied building slot, a `base_add` modifier is pushed.
/// For empty slots, any previously set modifier is removed.
/// Runs BEFORE tick_production so that `.final_value()` reflects current buildings.
pub fn sync_building_modifiers(
    mut query: Query<(&Buildings, &mut Production)>,
) {
    for (buildings, mut prod) in &mut query {
        for (slot_idx, slot) in buildings.slots.iter().enumerate() {
            let id_m = format!("building_slot_{}_minerals", slot_idx);
            let id_e = format!("building_slot_{}_energy", slot_idx);
            let id_r = format!("building_slot_{}_research", slot_idx);
            let id_f = format!("building_slot_{}_food", slot_idx);
            if let Some(bt) = slot {
                let (m, e, r, f) = bt.production_bonus();
                let label = format!("{} (slot {})", bt.name(), slot_idx);
                if m != Amt::ZERO {
                    prod.minerals_per_hexadies.push_modifier(Modifier {
                        id: id_m,
                        label: label.clone(),
                        base_add: SignedAmt::from_amt(m),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    prod.minerals_per_hexadies.pop_modifier(&id_m);
                }
                if e != Amt::ZERO {
                    prod.energy_per_hexadies.push_modifier(Modifier {
                        id: id_e,
                        label: label.clone(),
                        base_add: SignedAmt::from_amt(e),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    prod.energy_per_hexadies.pop_modifier(&id_e);
                }
                if r != Amt::ZERO {
                    prod.research_per_hexadies.push_modifier(Modifier {
                        id: id_r,
                        label: label.clone(),
                        base_add: SignedAmt::from_amt(r),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    prod.research_per_hexadies.pop_modifier(&id_r);
                }
                if f != Amt::ZERO {
                    prod.food_per_hexadies.push_modifier(Modifier {
                        id: id_f,
                        label,
                        base_add: SignedAmt::from_amt(f),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    prod.food_per_hexadies.pop_modifier(&id_f);
                }
            } else {
                prod.minerals_per_hexadies.pop_modifier(&id_m);
                prod.energy_per_hexadies.pop_modifier(&id_e);
                prod.research_per_hexadies.pop_modifier(&id_r);
                prod.food_per_hexadies.pop_modifier(&id_f);
            }
        }
    }
}

/// Synchronise maintenance cost modifiers on the MaintenanceCost component.
/// Pushes a `base_add` modifier for each occupied building slot and for each
/// ship whose home_port matches the colony's system.
/// Runs BEFORE tick_maintenance so that `.final_value()` is up-to-date.
pub fn sync_maintenance_modifiers(
    mut colonies: Query<(&Colony, &mut MaintenanceCost, Option<&Buildings>)>,
    ships: Query<(Entity, &Ship)>,
    stars: Query<&StarSystem>,
) {
    // Find capital system for fallback
    let capital_entity: Option<Entity> = {
        let mut found = None;
        for (colony, _, _) in colonies.iter() {
            if let Ok(star) = stars.get(colony.system) {
                if star.is_capital {
                    found = Some(colony.system);
                    break;
                }
            }
        }
        found
    };

    // Collect colony system entities for home_port validation
    let colony_systems: std::collections::HashSet<Entity> = colonies
        .iter()
        .map(|(c, _, _)| c.system)
        .collect();

    // Collect ship maintenance costs grouped by effective home_port
    let mut ship_costs_by_system: std::collections::HashMap<Entity, Vec<(String, Amt)>> =
        std::collections::HashMap::new();
    for (entity, ship) in &ships {
        let effective_port = if colony_systems.contains(&ship.home_port) {
            ship.home_port
        } else {
            capital_entity.unwrap_or(ship.home_port)
        };
        ship_costs_by_system
            .entry(effective_port)
            .or_default()
            .push((format!("ship_maint_{:?}", entity), ship.ship_type.maintenance_cost()));
    }

    for (colony, mut maint, buildings) in &mut colonies {
        // Track which modifier IDs we set this frame
        let mut active_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Building maintenance modifiers
        if let Some(buildings) = buildings {
            for (slot_idx, slot) in buildings.slots.iter().enumerate() {
                let id = format!("building_maint_{}", slot_idx);
                if let Some(bt) = slot {
                    let cost = bt.maintenance_cost();
                    if cost != Amt::ZERO {
                        maint.energy_per_hexadies.push_modifier(Modifier {
                            id: id.clone(),
                            label: format!("{} (slot {})", bt.name(), slot_idx),
                            base_add: SignedAmt::from_amt(cost),
                            multiplier: SignedAmt::ZERO,
                            add: SignedAmt::ZERO,
                            expires_at: None,
                            on_expire_event: None,
                        });
                        active_ids.insert(id);
                    } else {
                        maint.energy_per_hexadies.pop_modifier(&id);
                    }
                } else {
                    maint.energy_per_hexadies.pop_modifier(&id);
                }
            }
        }

        // Ship maintenance modifiers
        if let Some(ship_list) = ship_costs_by_system.get(&colony.system) {
            for (ship_id, cost) in ship_list {
                maint.energy_per_hexadies.push_modifier(Modifier {
                    id: ship_id.clone(),
                    label: format!("Ship {}", ship_id),
                    base_add: SignedAmt::from_amt(*cost),
                    multiplier: SignedAmt::ZERO,
                    add: SignedAmt::ZERO,
                    expires_at: None,
                    on_expire_event: None,
                });
                active_ids.insert(ship_id.clone());
            }
        }

        // Remove stale ship modifiers (ships that moved away or were destroyed)
        let stale: Vec<String> = maint
            .energy_per_hexadies
            .modifiers()
            .iter()
            .filter(|m| m.id.starts_with("ship_maint_") && !active_ids.contains(&m.id))
            .map(|m| m.id.clone())
            .collect();
        for id in stale {
            maint.energy_per_hexadies.pop_modifier(&id);
        }
    }
}

/// Synchronise food consumption based on current population.
/// Sets the ModifiedValue base to `population * FOOD_PER_POP_PER_HEXADIES`.
/// Any tech modifiers (e.g. Hydroponics -20%) remain attached as multiplier modifiers.
/// Runs BEFORE tick_population_growth.
pub fn sync_food_consumption(
    mut query: Query<(&Colony, &mut FoodConsumption)>,
) {
    use crate::galaxy::FOOD_PER_POP_PER_HEXADIES;

    for (colony, mut consumption) in &mut query {
        let base = Amt::from_f64(colony.population).mul_amt(FOOD_PER_POP_PER_HEXADIES);
        consumption.food_per_hexadies.set_base(base);
    }
}

/// #29: tick_production uses ProductionFocus weights and building bonuses
/// #45: Multiplies output by GlobalParams production multipliers
/// #44: Research is no longer accumulated in the stockpile; emitted via emit_research
/// #73: Non-capital colonies have production reduced when capital authority is depleted
pub fn tick_production(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    global_params: Res<crate::technology::GlobalParams>,
    mut query: Query<(&Colony, &Production, &mut ResourceStockpile, Option<&ProductionFocus>, Option<&ResourceCapacity>)>,
    stars: Query<&StarSystem>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;
    let d_amt = Amt::units(d);

    // #73: Check if the capital has an authority deficit.
    let capital_authority = query.iter().find_map(|(colony, _, stockpile, _, _)| {
        stars.get(colony.system).ok().and_then(|star| {
            if star.is_capital {
                Some(stockpile.authority)
            } else {
                None
            }
        })
    });
    let authority_deficit = matches!(capital_authority, Some(a) if a == Amt::ZERO);

    for (colony, prod, mut stockpile, focus, capacity) in &mut query {
        let (mw, ew) = match focus {
            Some(f) => (f.minerals_weight, f.energy_weight),
            None => (Amt::units(1), Amt::units(1)),
        };

        // #73: Apply authority deficit penalty to non-capital colonies
        let is_capital = stars.get(colony.system).is_ok_and(|s| s.is_capital);
        let authority_multiplier = if authority_deficit && !is_capital {
            AUTHORITY_DEFICIT_PENALTY
        } else {
            Amt::units(1)
        };

        // Building bonuses are already included via modifiers on Production
        // (sync_building_modifiers runs before this system).
        let m_global = Amt::milli((global_params.production_multiplier_minerals * 1000.0) as u64);
        let e_global = Amt::milli((global_params.production_multiplier_energy * 1000.0) as u64);
        stockpile.minerals = stockpile.minerals.add(
            prod.minerals_per_hexadies.final_value().mul_amt(mw).mul_amt(d_amt).mul_amt(m_global).mul_amt(authority_multiplier)
        );
        stockpile.energy = stockpile.energy.add(
            prod.energy_per_hexadies.final_value().mul_amt(ew).mul_amt(d_amt).mul_amt(e_global).mul_amt(authority_multiplier)
        );
        stockpile.food = stockpile.food.add(
            prod.food_per_hexadies.final_value().mul_amt(d_amt).mul_amt(authority_multiplier)
        );
        // Research is no longer accumulated in the stockpile; it is emitted
        // directly via emit_research in the technology module.

        // Clamp resources to capacity
        if let Some(cap) = capacity {
            stockpile.minerals = stockpile.minerals.min(cap.minerals);
            stockpile.energy = stockpile.energy.min(cap.energy);
            stockpile.food = stockpile.food.min(cap.food);
            stockpile.authority = stockpile.authority.min(cap.authority);
        }
    }
}

/// #69: Logistic population growth with carrying capacity.
/// #72: Food consumption and starvation.
///
/// K (carrying capacity) = min(BASE_CARRYING_CAPACITY * hab_score, food_production / FOOD_PER_POP)
/// Growth rate is scaled by hab_score.
/// dP/dt = r * hab_score * P * (1 - P/K) — when P > K, population declines naturally.
pub fn tick_population_growth(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    global_params: Res<crate::technology::GlobalParams>,
    mut colonies: Query<(
        &mut Colony,
        &mut ResourceStockpile,
        &Production,
        Option<&FoodConsumption>,
    )>,
    stars: Query<(&StarSystem, &crate::galaxy::SystemAttributes)>,
) {
    use crate::galaxy::{BASE_CARRYING_CAPACITY, FOOD_PER_POP_PER_HEXADIES};

    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;

    for (mut colony, mut stockpile, production, food_consumption) in &mut colonies {
        // #72: Food consumption via FoodConsumption ModifiedValue (includes tech modifiers),
        // falling back to manual calculation if the component is absent.
        let food_consumed = if let Some(fc) = food_consumption {
            fc.food_per_hexadies.final_value().mul_u64(d)
        } else {
            Amt::from_f64(colony.population).mul_amt(FOOD_PER_POP_PER_HEXADIES).mul_u64(d)
        };
        stockpile.food = stockpile.food.sub(food_consumed);

        if stockpile.food == Amt::ZERO {
            // Starvation: population decreases (f64 domain)
            let starvation_loss = colony.population * 0.01 * d as f64;
            colony.population = (colony.population - starvation_loss).max(1.0);
        } else {
            // #69: Logistic growth (f64 domain for population math)
            let hab_score = stars
                .get(colony.system)
                .map(|(_, attr)| attr.habitability.base_score())
                .unwrap_or(0.5);

            // Total food production from ModifiedValue (includes building bonuses)
            let food_prod = production.food_per_hexadies.final_value();

            // K = min(habitat limit, food-sustainable population)
            let k_habitat = BASE_CARRYING_CAPACITY * hab_score;
            let k_food = if FOOD_PER_POP_PER_HEXADIES.raw() > 0 {
                food_prod.div_amt(FOOD_PER_POP_PER_HEXADIES).to_f64()
            } else {
                k_habitat
            };
            let k = k_habitat.min(k_food).max(1.0);

            // Logistic: P_new = P + r * hab_score * P * (1 - P/K) * delta
            let effective_growth = colony.growth_rate + global_params.population_growth_bonus;
            let dp = effective_growth * hab_score * colony.population * (1.0 - colony.population / k) * d as f64;
            colony.population = (colony.population + dp).max(1.0);
        }
    }
}

/// #32: build_time_remaining countdown, #35: shipyard check
pub fn tick_build_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(&Colony, &mut BuildQueue, &mut ResourceStockpile, Option<&Buildings>)>,
    positions: Query<&Position>,
    stars: Query<&StarSystem>,
    mut events: MessageWriter<GameEvent>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    for (colony, mut build_queue, mut stockpile, buildings) in &mut query {
        // #35: Skip ship construction if colony has no shipyard
        let has_shipyard = buildings.is_some_and(|b| b.has_shipyard());
        if !build_queue.queue.is_empty() && !has_shipyard {
            warn!("Colony lacks a Shipyard; skipping ship construction");
            continue;
        }

        for _ in 0..delta {
            if build_queue.queue.is_empty() {
                break;
            }
            let order = &mut build_queue.queue[0];

            let minerals_needed = order.minerals_cost.sub(order.minerals_invested);
            let minerals_transfer = minerals_needed.min(stockpile.minerals);
            order.minerals_invested = order.minerals_invested.add(minerals_transfer);
            stockpile.minerals = stockpile.minerals.sub(minerals_transfer);

            let energy_needed = order.energy_cost.sub(order.energy_invested);
            let energy_transfer = energy_needed.min(stockpile.energy);
            order.energy_invested = order.energy_invested.add(energy_transfer);
            stockpile.energy = stockpile.energy.sub(energy_transfer);

            // #32: Decrement build time
            order.build_time_remaining -= 1;

            if build_queue.queue[0].is_complete() {
                let completed = build_queue.queue.remove(0);
                let ship_type = match completed.ship_type_name.as_str() {
                    "Explorer" => ShipType::Explorer,
                    "Colony Ship" => ShipType::ColonyShip,
                    "Courier" => ShipType::Courier,
                    _ => {
                        warn!("Unknown ship type: {}", completed.ship_type_name);
                        continue;
                    }
                };
                if let Ok(pos) = positions.get(colony.system) {
                    spawn_ship(
                        &mut commands,
                        ship_type,
                        completed.ship_type_name.clone(),
                        colony.system,
                        *pos,
                    );
                    let sys_name = stars.get(colony.system).map(|s| s.name.clone()).unwrap_or_default();
                    events.write(GameEvent {
                        timestamp: clock.elapsed,
                        kind: GameEventKind::ShipBuilt,
                        description: format!("{} built at {}", completed.ship_type_name, sys_name),
                        related_system: Some(colony.system),
                    });
                    info!("Ship built and launched: {}", completed.ship_type_name);
                }
            }
        }
    }
}

pub fn tick_building_queue(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(&mut BuildingQueue, &mut Buildings, &mut ResourceStockpile)>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    for (mut bq, mut buildings, mut stockpile) in &mut query {
        for _ in 0..delta {
            if bq.queue.is_empty() {
                break;
            }
            let order = &mut bq.queue[0];

            let minerals_transfer = order.minerals_remaining.min(stockpile.minerals);
            order.minerals_remaining = order.minerals_remaining.sub(minerals_transfer);
            stockpile.minerals = stockpile.minerals.sub(minerals_transfer);

            let energy_transfer = order.energy_remaining.min(stockpile.energy);
            order.energy_remaining = order.energy_remaining.sub(energy_transfer);
            stockpile.energy = stockpile.energy.sub(energy_transfer);

            order.build_time_remaining -= 1;

            if bq.queue[0].minerals_remaining == Amt::ZERO
                && bq.queue[0].energy_remaining == Amt::ZERO
                && bq.queue[0].build_time_remaining <= 0
            {
                let completed = bq.queue.remove(0);
                if completed.target_slot < buildings.slots.len() {
                    buildings.slots[completed.target_slot] = Some(completed.building_type);
                    info!(
                        "Building {:?} completed in slot {}",
                        completed.building_type, completed.target_slot
                    );
                } else {
                    warn!(
                        "Building {:?} completed but target slot {} is out of range (max {})",
                        completed.building_type,
                        completed.target_slot,
                        buildings.slots.len()
                    );
                }
            }
        }
    }
}

/// Updates sovereignty of star systems based on colony presence.
pub fn update_sovereignty(
    colonies: Query<&Colony>,
    mut sovereignties: Query<(Entity, &mut Sovereignty)>,
) {
    let mut colony_pop: std::collections::HashMap<Entity, f64> = std::collections::HashMap::new();
    for colony in &colonies {
        *colony_pop.entry(colony.system).or_insert(0.0) += colony.population;
    }

    for (entity, mut sov) in &mut sovereignties {
        if let Some(&pop) = colony_pop.get(&entity) {
            sov.owner = Some(Owner::Player);
            sov.control_score = pop;
        } else {
            sov.owner = None;
            sov.control_score = 0.0;
        }
    }
}

/// #51/#64: Deduct energy maintenance costs for buildings and ships.
/// Uses MaintenanceCost component (populated by sync_maintenance_modifiers) when present,
/// falling back to manual summing for colonies without the component.
/// Ship home_port reassignment to capital is now handled in sync_maintenance_modifiers.
/// Runs after production so that newly generated energy is available.
pub fn tick_maintenance(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut colonies: Query<(&Colony, &mut ResourceStockpile, Option<&MaintenanceCost>, Option<&Buildings>)>,
    ships: Query<(&Ship, &ShipState)>,
    stars: Query<&StarSystem>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;

    // For colonies WITH MaintenanceCost, just read final_value().
    // For colonies WITHOUT it (backward compat), fall back to manual sum.
    // Collect ship costs for fallback path.
    let capital_entity: Option<Entity> = {
        let mut found = None;
        for (colony, _, _, _) in colonies.iter() {
            if let Ok(star) = stars.get(colony.system) {
                if star.is_capital {
                    found = Some(colony.system);
                    break;
                }
            }
        }
        found
    };

    let colony_systems: std::collections::HashSet<Entity> = colonies
        .iter()
        .map(|(c, _, _, _)| c.system)
        .collect();

    let mut ship_maintenance_by_system: std::collections::HashMap<Entity, Amt> =
        std::collections::HashMap::new();

    for (ship, _state) in &ships {
        let effective_port = if colony_systems.contains(&ship.home_port) {
            ship.home_port
        } else {
            capital_entity.unwrap_or(ship.home_port)
        };
        let entry = ship_maintenance_by_system
            .entry(effective_port)
            .or_insert(Amt::ZERO);
        *entry = entry.add(ship.ship_type.maintenance_cost());
    }

    for (colony, mut stockpile, maint, buildings) in &mut colonies {
        let total_maintenance = if let Some(maint) = maint {
            // ModifiedValue path: sync_maintenance_modifiers has already set modifiers
            maint.energy_per_hexadies.final_value()
        } else {
            // Fallback: manual sum (for colonies spawned without MaintenanceCost)
            let mut total = Amt::ZERO;
            if let Some(buildings) = buildings {
                for slot in &buildings.slots {
                    if let Some(building) = slot {
                        total = total.add(building.maintenance_cost());
                    }
                }
            }
            if let Some(&ship_cost) = ship_maintenance_by_system.get(&colony.system) {
                total = total.add(ship_cost);
            }
            total
        };

        // Deduct energy: maintenance_per_hd * delta
        stockpile.energy = stockpile.energy.sub(total_maintenance.mul_u64(d));
    }
}

/// #73: Authority production and empire-scale consumption.
///
/// - The capital colony produces `BASE_AUTHORITY_PER_HEXADIES` authority per hexady.
/// - Each non-capital colony costs `AUTHORITY_COST_PER_COLONY` authority per hexady,
///   deducted from the capital's stockpile.
/// - When the capital's authority reaches 0, non-capital colonies suffer a production
///   efficiency penalty (applied in `tick_production`).
///
/// NOTE: Remote command costs (one-time authority cost when issuing commands to
/// distant colonies) are not implemented here -- they belong in the communication
/// module and will be handled separately.
pub fn tick_authority(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    authority_params: Res<AuthorityParams>,
    mut colonies: Query<(&Colony, &mut ResourceStockpile, Option<&ResourceCapacity>)>,
    stars: Query<&StarSystem>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;

    // First pass: find capital system and count non-capital colonies
    let mut capital_system: Option<Entity> = None;
    let mut non_capital_count: u64 = 0;
    for (colony, _, _) in colonies.iter() {
        if let Ok(star) = stars.get(colony.system) {
            if star.is_capital {
                capital_system = Some(colony.system);
            } else {
                non_capital_count += 1;
            }
        } else {
            non_capital_count += 1;
        }
    }

    let Some(cap_sys) = capital_system else {
        return; // No capital found
    };

    // Second pass: produce authority at capital and deduct empire scale cost
    let auth_production = authority_params.production.final_value();
    let auth_cost_per_colony = authority_params.cost_per_colony.final_value();
    for (colony, mut stockpile, capacity) in &mut colonies {
        if colony.system == cap_sys {
            // Capital produces authority
            stockpile.authority = stockpile.authority.add(auth_production.mul_u64(d));

            // Deduct empire scale cost for non-capital colonies
            let scale_cost = auth_cost_per_colony.mul_u64(non_capital_count).mul_u64(d);
            stockpile.authority = stockpile.authority.sub(scale_cost);

            // Clamp authority to capacity
            if let Some(cap) = capacity {
                stockpile.authority = stockpile.authority.min(cap.authority);
            }
            break;
        }
    }
}

pub fn advance_production_tick(clock: Res<GameClock>, mut last_tick: ResMut<LastProductionTick>) {
    last_tick.0 = clock.elapsed;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_order(minerals_cost: Amt, minerals_invested: Amt, energy_cost: Amt, energy_invested: Amt) -> BuildOrder {
        let build_time = 60;
        BuildOrder {
            ship_type_name: "Explorer".to_string(),
            minerals_cost,
            minerals_invested,
            energy_cost,
            energy_invested,
            build_time_total: build_time,
            build_time_remaining: 0, // for is_complete tests, set to 0
        }
    }

    #[test]
    fn build_order_complete_when_both_met() {
        let order = make_order(Amt::units(100), Amt::units(100), Amt::units(50), Amt::units(50));
        assert!(order.is_complete());
    }

    #[test]
    fn build_order_incomplete_minerals_short() {
        let order = make_order(Amt::units(100), Amt::units(80), Amt::units(50), Amt::units(50));
        assert!(!order.is_complete());
    }

    #[test]
    fn build_order_incomplete_energy_short() {
        let order = make_order(Amt::units(100), Amt::units(100), Amt::units(50), Amt::units(30));
        assert!(!order.is_complete());
    }

    #[test]
    fn build_order_incomplete_time_remaining() {
        let mut order = make_order(Amt::units(100), Amt::units(100), Amt::units(50), Amt::units(50));
        order.build_time_remaining = 5;
        assert!(!order.is_complete());
    }

    #[test]
    fn mine_production_bonus() {
        let (m, e, r, f) = BuildingType::Mine.production_bonus();
        assert_eq!(m, Amt::units(3));
        assert_eq!(e, Amt::ZERO);
        assert_eq!(r, Amt::ZERO);
        assert_eq!(f, Amt::ZERO);
    }

    #[test]
    fn power_plant_production_bonus() {
        let (m, e, r, f) = BuildingType::PowerPlant.production_bonus();
        assert_eq!(m, Amt::ZERO);
        assert_eq!(e, Amt::units(3));
        assert_eq!(r, Amt::ZERO);
        assert_eq!(f, Amt::ZERO);
    }

    #[test]
    fn research_lab_production_bonus() {
        let (m, e, r, f) = BuildingType::ResearchLab.production_bonus();
        assert_eq!(m, Amt::ZERO);
        assert_eq!(e, Amt::ZERO);
        assert_eq!(r, Amt::units(2));
        assert_eq!(f, Amt::ZERO);
    }

    #[test]
    fn shipyard_production_bonus() {
        let (m, e, r, f) = BuildingType::Shipyard.production_bonus();
        assert_eq!(m, Amt::ZERO);
        assert_eq!(e, Amt::ZERO);
        assert_eq!(r, Amt::ZERO);
        assert_eq!(f, Amt::ZERO);
    }

    #[test]
    fn mine_build_cost() {
        assert_eq!(BuildingType::Mine.build_cost(), (Amt::units(150), Amt::units(50)));
    }

    #[test]
    fn power_plant_build_cost() {
        assert_eq!(BuildingType::PowerPlant.build_cost(), (Amt::units(50), Amt::units(150)));
    }

    #[test]
    fn research_lab_build_cost() {
        assert_eq!(BuildingType::ResearchLab.build_cost(), (Amt::units(100), Amt::units(100)));
    }

    #[test]
    fn shipyard_build_cost() {
        assert_eq!(BuildingType::Shipyard.build_cost(), (Amt::units(300), Amt::units(200)));
    }

    #[test]
    fn build_times() {
        assert_eq!(BuildingType::Mine.build_time(), 10);
        assert_eq!(BuildingType::PowerPlant.build_time(), 10);
        assert_eq!(BuildingType::ResearchLab.build_time(), 15);
        assert_eq!(BuildingType::Shipyard.build_time(), 30);
    }

    #[test]
    fn buildings_slots_empty() {
        let buildings = Buildings {
            slots: vec![None; 5],
        };
        assert_eq!(buildings.slots.len(), 5);
        assert!(buildings.slots.iter().all(|s| s.is_none()));
    }

    #[test]
    fn buildings_slots_with_buildings() {
        let mut buildings = Buildings {
            slots: vec![None; 5],
        };
        buildings.slots[0] = Some(BuildingType::Mine);
        buildings.slots[2] = Some(BuildingType::PowerPlant);

        assert_eq!(buildings.slots[0], Some(BuildingType::Mine));
        assert_eq!(buildings.slots[1], None);
        assert_eq!(buildings.slots[2], Some(BuildingType::PowerPlant));
    }

    #[test]
    fn buildings_total_production_bonus() {
        let buildings = Buildings {
            slots: vec![
                Some(BuildingType::Mine),
                Some(BuildingType::Mine),
                Some(BuildingType::PowerPlant),
                Some(BuildingType::ResearchLab),
                None,
            ],
        };
        let (mut m, mut e, mut r, mut f) = (Amt::ZERO, Amt::ZERO, Amt::ZERO, Amt::ZERO);
        for slot in &buildings.slots {
            if let Some(bt) = slot {
                let (bm, be, br, bf) = bt.production_bonus();
                m = m.add(bm);
                e = e.add(be);
                r = r.add(br);
                f = f.add(bf);
            }
        }
        assert_eq!(m, Amt::units(6));
        assert_eq!(e, Amt::units(3));
        assert_eq!(r, Amt::units(2));
        assert_eq!(f, Amt::ZERO);
    }

    #[test]
    fn has_shipyard_true() {
        let buildings = Buildings {
            slots: vec![Some(BuildingType::Mine), Some(BuildingType::Shipyard), None],
        };
        assert!(buildings.has_shipyard());
    }

    #[test]
    fn has_shipyard_false() {
        let buildings = Buildings {
            slots: vec![Some(BuildingType::Mine), Some(BuildingType::PowerPlant), None],
        };
        assert!(!buildings.has_shipyard());
    }

    #[test]
    fn production_focus_labels() {
        assert_eq!(ProductionFocus::balanced().label(), "Balanced");
        assert_eq!(ProductionFocus::minerals().label(), "Minerals");
        assert_eq!(ProductionFocus::energy().label(), "Energy");
        assert_eq!(ProductionFocus::research().label(), "Research");
    }

    #[test]
    fn build_order_build_time_for() {
        assert_eq!(BuildOrder::build_time_for("Explorer"), 60);
        assert_eq!(BuildOrder::build_time_for("Colony Ship"), 120);
        assert_eq!(BuildOrder::build_time_for("Courier"), 30);
        assert_eq!(BuildOrder::build_time_for("Unknown"), 60);
    }

    // --- #46: Port tests ---

    #[test]
    fn has_port_true() {
        let buildings = Buildings {
            slots: vec![Some(BuildingType::Mine), Some(BuildingType::Port), None],
        };
        assert!(buildings.has_port());
    }

    #[test]
    fn has_port_false() {
        let buildings = Buildings {
            slots: vec![Some(BuildingType::Mine), Some(BuildingType::Shipyard), None],
        };
        assert!(!buildings.has_port());
    }

    #[test]
    fn port_build_cost() {
        assert_eq!(BuildingType::Port.build_cost(), (Amt::units(400), Amt::units(300)));
    }

    #[test]
    fn port_build_time() {
        assert_eq!(BuildingType::Port.build_time(), 40);
    }

    #[test]
    fn port_production_bonus() {
        let (m, e, r, f) = BuildingType::Port.production_bonus();
        assert_eq!(m, Amt::ZERO);
        assert_eq!(e, Amt::ZERO);
        assert_eq!(r, Amt::ZERO);
        assert_eq!(f, Amt::ZERO);
    }

    #[test]
    fn port_name() {
        assert_eq!(BuildingType::Port.name(), "Port");
    }

    // --- #51: Maintenance cost tests ---

    #[test]
    fn building_maintenance_costs() {
        assert_eq!(BuildingType::Mine.maintenance_cost(), Amt::new(0, 200));
        assert_eq!(BuildingType::PowerPlant.maintenance_cost(), Amt::ZERO);
        assert_eq!(BuildingType::ResearchLab.maintenance_cost(), Amt::new(0, 500));
        assert_eq!(BuildingType::Shipyard.maintenance_cost(), Amt::units(1));
        assert_eq!(BuildingType::Port.maintenance_cost(), Amt::new(0, 500));
    }

    #[test]
    fn maintenance_deducts_from_stockpile() {
        let buildings = Buildings {
            slots: vec![
                Some(BuildingType::Mine),       // 0.2
                Some(BuildingType::Shipyard),    // 1.0
                Some(BuildingType::PowerPlant),  // 0.0
                None,
            ],
        };
        let mut energy = Amt::units(100);
        let delta = Amt::units(5);

        let mut total_maintenance = Amt::ZERO;
        for slot in &buildings.slots {
            if let Some(bt) = slot {
                total_maintenance = total_maintenance.add(bt.maintenance_cost());
            }
        }
        assert_eq!(total_maintenance, Amt::new(1, 200));

        energy = energy.sub(total_maintenance.mul_amt(delta));
        assert_eq!(energy, Amt::units(94));
    }

    #[test]
    fn maintenance_negative_energy_capped_at_zero() {
        let mut energy = Amt::units(2);
        let total_maintenance = Amt::units(1);
        let delta = Amt::units(5);

        // total_maintenance * delta = 5, energy = 2, saturating sub => 0
        energy = energy.sub(total_maintenance.mul_amt(delta));
        assert_eq!(energy, Amt::ZERO);
    }

    // --- #72: Farm and food tests ---

    #[test]
    fn farm_production_bonus() {
        let (m, e, r, f) = BuildingType::Farm.production_bonus();
        assert_eq!(m, Amt::ZERO);
        assert_eq!(e, Amt::ZERO);
        assert_eq!(r, Amt::ZERO);
        assert_eq!(f, Amt::units(5));
    }

    #[test]
    fn farm_build_cost() {
        assert_eq!(BuildingType::Farm.build_cost(), (Amt::units(100), Amt::units(50)));
    }

    #[test]
    fn farm_build_time() {
        assert_eq!(BuildingType::Farm.build_time(), 20);
    }

    #[test]
    fn farm_maintenance_cost() {
        assert_eq!(BuildingType::Farm.maintenance_cost(), Amt::new(0, 300));
    }

    #[test]
    fn farm_name() {
        assert_eq!(BuildingType::Farm.name(), "Farm");
    }

    #[test]
    fn buildings_total_production_with_farm() {
        let buildings = Buildings {
            slots: vec![
                Some(BuildingType::Mine),
                Some(BuildingType::Farm),
                Some(BuildingType::Farm),
                None,
            ],
        };
        let (mut m, mut e, mut r, mut f) = (Amt::ZERO, Amt::ZERO, Amt::ZERO, Amt::ZERO);
        for slot in &buildings.slots {
            if let Some(bt) = slot {
                let (bm, be, br, bf) = bt.production_bonus();
                m = m.add(bm);
                e = e.add(be);
                r = r.add(br);
                f = f.add(bf);
            }
        }
        assert_eq!(m, Amt::units(3));
        assert_eq!(e, Amt::ZERO);
        assert_eq!(r, Amt::ZERO);
        assert_eq!(f, Amt::units(10));
    }

    #[test]
    fn food_consumption_by_population() {
        // population=100, food=100, 1 hexadies: consumes 100*0.1*1 = 10 food
        let population: f64 = 100.0;
        let mut food: f64 = 100.0;
        let delta: f64 = 1.0;
        food -= population * 0.1 * delta;
        assert!((food - 90.0).abs() < 1e-10);
    }

    #[test]
    fn starvation_reduces_population() {
        // population=100, food=0, 1 hexadies: loses 100*0.01*1 = 1 pop
        let mut population: f64 = 100.0;
        let food: f64 = 0.0;
        let delta: f64 = 1.0;
        if food <= 0.0 {
            let loss = population * 0.01 * delta;
            population = (population - loss).max(1.0);
        }
        assert!((population - 99.0).abs() < 1e-10);
    }

    #[test]
    fn starvation_population_minimum() {
        // population should not drop below 1.0
        let mut population: f64 = 0.5;
        let food: f64 = 0.0;
        let delta: f64 = 1.0;
        if food <= 0.0 {
            let loss = population * 0.01 * delta;
            population = (population - loss).max(1.0);
        }
        assert_eq!(population, 1.0);
    }
}
