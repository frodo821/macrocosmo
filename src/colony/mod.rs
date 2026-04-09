use bevy::prelude::*;

use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{StarSystem, SystemAttributes, Sovereignty};
use crate::ship::{spawn_ship, Owner, Ship, ShipState, ShipType};
use crate::time_system::GameClock;

pub struct ColonyPlugin;

#[derive(Resource, Default)]
pub struct LastProductionTick(pub i64);

impl Plugin for ColonyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastProductionTick>()
            .add_systems(
                Startup,
                spawn_capital_colony.after(crate::galaxy::generate_galaxy),
            )
            .add_systems(
                Update,
                (
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
    pub minerals: f64,
    pub energy: f64,
    pub research: f64,
    pub food: f64,
    pub authority: f64,
}

#[derive(Component)]
pub struct Production {
    pub minerals_per_hexadies: f64,
    pub energy_per_hexadies: f64,
    pub research_per_hexadies: f64,
    pub food_per_hexadies: f64,
}

/// #29: Production focus weights for colony output
#[derive(Component)]
pub struct ProductionFocus {
    pub minerals_weight: f64,
    pub energy_weight: f64,
    pub research_weight: f64,
}

impl Default for ProductionFocus {
    fn default() -> Self {
        Self {
            minerals_weight: 1.0,
            energy_weight: 1.0,
            research_weight: 1.0,
        }
    }
}

impl ProductionFocus {
    pub fn balanced() -> Self {
        Self::default()
    }
    pub fn minerals() -> Self {
        Self {
            minerals_weight: 2.0,
            energy_weight: 0.5,
            research_weight: 0.5,
        }
    }
    pub fn energy() -> Self {
        Self {
            minerals_weight: 0.5,
            energy_weight: 2.0,
            research_weight: 0.5,
        }
    }
    pub fn research() -> Self {
        Self {
            minerals_weight: 0.5,
            energy_weight: 0.5,
            research_weight: 2.0,
        }
    }

    pub fn label(&self) -> &'static str {
        if (self.minerals_weight - 1.0).abs() < 0.01
            && (self.energy_weight - 1.0).abs() < 0.01
            && (self.research_weight - 1.0).abs() < 0.01
        {
            "Balanced"
        } else if self.minerals_weight > 1.5 {
            "Minerals"
        } else if self.energy_weight > 1.5 {
            "Energy"
        } else if self.research_weight > 1.5 {
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
    pub minerals_cost: f64,
    pub minerals_invested: f64,
    pub energy_cost: f64,
    pub energy_invested: f64,
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
}

impl BuildingType {
    pub fn production_bonus(&self) -> (f64, f64, f64) {
        // (minerals, energy, research) per hexadies
        match self {
            BuildingType::Mine => (3.0, 0.0, 0.0),
            BuildingType::PowerPlant => (0.0, 3.0, 0.0),
            BuildingType::ResearchLab => (0.0, 0.0, 2.0),
            BuildingType::Shipyard => (0.0, 0.0, 0.0), // special effect
            BuildingType::Port => (0.0, 0.0, 0.0),     // special effect
        }
    }

    pub fn build_cost(&self) -> (f64, f64) {
        // (minerals, energy)
        match self {
            BuildingType::Mine => (150.0, 50.0),
            BuildingType::PowerPlant => (50.0, 150.0),
            BuildingType::ResearchLab => (100.0, 100.0),
            BuildingType::Shipyard => (300.0, 200.0),
            BuildingType::Port => (400.0, 300.0),
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
        }
    }

    /// Energy maintenance cost per hexadies (#51)
    pub fn maintenance_cost(&self) -> f64 {
        match self {
            BuildingType::Mine => 0.2,
            BuildingType::PowerPlant => 0.0, // self-powered
            BuildingType::ResearchLab => 0.5,
            BuildingType::Shipyard => 1.0,
            BuildingType::Port => 0.5,
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
    pub minerals_remaining: f64,
    pub energy_remaining: f64,
    pub build_time_remaining: i64,
}

pub fn spawn_capital_colony(
    mut commands: Commands,
    query: Query<(Entity, &StarSystem, &SystemAttributes)>,
) {
    for (entity, system, attributes) in query.iter() {
        if system.is_capital {
            let num_slots = attributes.max_building_slots as usize;
            let mut slots = vec![None; num_slots];
            // Capital starts with 1 Mine, 1 PowerPlant, and 1 Shipyard (#35)
            if num_slots > 0 {
                slots[0] = Some(BuildingType::Mine);
            }
            if num_slots > 1 {
                slots[1] = Some(BuildingType::PowerPlant);
            }
            if num_slots > 2 {
                slots[2] = Some(BuildingType::Shipyard);
            }
            commands.spawn((
                Colony {
                    system: entity,
                    population: 100.0,
                    growth_rate: 0.01,
                },
                ResourceStockpile {
                    minerals: 500.0,
                    energy: 500.0,
                    research: 0.0,
                    food: 100.0,
                    authority: 0.0,
                },
                Production {
                    minerals_per_hexadies: 5.0,
                    energy_per_hexadies: 5.0,
                    research_per_hexadies: 1.0,
                    food_per_hexadies: 0.0,
                },
                BuildQueue {
                    queue: Vec::new(),
                },
                Buildings { slots },
                BuildingQueue::default(),
                ProductionFocus::default(),
            ));
            info!("Capital colony spawned on {}", system.name);
            return;
        }
    }
    warn!("No capital star system found; capital colony not created");
}

/// #29: tick_production uses ProductionFocus weights and building bonuses
/// #45: Multiplies output by GlobalParams production multipliers
/// #44: Research is no longer accumulated in the stockpile; emitted via emit_research
pub fn tick_production(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    global_params: Res<crate::technology::GlobalParams>,
    mut query: Query<(&Production, &mut ResourceStockpile, Option<&Buildings>, Option<&ProductionFocus>)>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as f64;
    for (prod, mut stockpile, buildings, focus) in &mut query {
        let (mut bonus_m, mut bonus_e) = (0.0, 0.0);
        if let Some(buildings) = buildings {
            for slot in &buildings.slots {
                if let Some(bt) = slot {
                    let (m, e, _r) = bt.production_bonus();
                    bonus_m += m;
                    bonus_e += e;
                }
            }
        }
        let (mw, ew) = match focus {
            Some(f) => (f.minerals_weight, f.energy_weight),
            None => (1.0, 1.0),
        };
        stockpile.minerals += (prod.minerals_per_hexadies + bonus_m) * mw * d * global_params.production_multiplier_minerals;
        stockpile.energy += (prod.energy_per_hexadies + bonus_e) * ew * d * global_params.production_multiplier_energy;
        // Research is no longer accumulated in the stockpile; it is emitted
        // directly via emit_research in the technology module.
    }
}

/// #45: Population growth uses GlobalParams bonus
pub fn tick_population_growth(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    global_params: Res<crate::technology::GlobalParams>,
    mut query: Query<&mut Colony>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    for mut colony in &mut query {
        let effective_growth = colony.growth_rate + global_params.population_growth_bonus;
        let growth_factor = (1.0 + effective_growth).powi(delta as i32);
        colony.population *= growth_factor;
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

            let minerals_needed = order.minerals_cost - order.minerals_invested;
            let minerals_transfer = minerals_needed.min(stockpile.minerals).max(0.0);
            order.minerals_invested += minerals_transfer;
            stockpile.minerals -= minerals_transfer;

            let energy_needed = order.energy_cost - order.energy_invested;
            let energy_transfer = energy_needed.min(stockpile.energy).max(0.0);
            order.energy_invested += energy_transfer;
            stockpile.energy -= energy_transfer;

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

            let minerals_transfer = order.minerals_remaining.min(stockpile.minerals).max(0.0);
            order.minerals_remaining -= minerals_transfer;
            stockpile.minerals -= minerals_transfer;

            let energy_transfer = order.energy_remaining.min(stockpile.energy).max(0.0);
            order.energy_remaining -= energy_transfer;
            stockpile.energy -= energy_transfer;

            order.build_time_remaining -= 1;

            if bq.queue[0].minerals_remaining <= 0.0
                && bq.queue[0].energy_remaining <= 0.0
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
/// Ship maintenance is charged to the colony at the ship's home_port, not the docked location.
/// If the home_port colony no longer exists, falls back to the capital system.
/// Runs after production so that newly generated energy is available.
pub fn tick_maintenance(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut colonies: Query<(&Colony, &mut ResourceStockpile, Option<&Buildings>)>,
    mut ships: Query<(&mut Ship, &ShipState)>,
    stars: Query<&StarSystem>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as f64;

    // #64: Collect ship maintenance costs grouped by home_port system entity.
    // Also handle fallback: if home_port colony doesn't exist, reassign to capital.
    let capital_entity: Option<Entity> = {
        let colony_systems: Vec<Entity> = colonies.iter().map(|(c, _, _)| c.system).collect();
        let mut found = None;
        for sys in &colony_systems {
            if let Ok(star) = stars.get(*sys) {
                if star.is_capital {
                    found = Some(*sys);
                    break;
                }
            }
        }
        found
    };

    // Collect per-system ship maintenance costs
    let mut ship_maintenance_by_system: std::collections::HashMap<Entity, f64> =
        std::collections::HashMap::new();

    // Check which systems have colonies
    let colony_systems: std::collections::HashSet<Entity> = colonies
        .iter()
        .map(|(c, _, _)| c.system)
        .collect();

    for (mut ship, _state) in &mut ships {
        // If home_port colony no longer exists, fall back to capital
        if !colony_systems.contains(&ship.home_port) {
            if let Some(cap) = capital_entity {
                ship.home_port = cap;
            }
            // If no capital either, maintenance just won't be charged
        }
        *ship_maintenance_by_system
            .entry(ship.home_port)
            .or_insert(0.0) += ship.ship_type.maintenance_cost();
    }

    for (colony, mut stockpile, buildings) in &mut colonies {
        let mut total_maintenance = 0.0;

        // Building maintenance
        if let Some(buildings) = buildings {
            for slot in &buildings.slots {
                if let Some(building) = slot {
                    total_maintenance += building.maintenance_cost();
                }
            }
        }

        // #64: Ship maintenance charged to this colony via home_port
        if let Some(&ship_cost) = ship_maintenance_by_system.get(&colony.system) {
            total_maintenance += ship_cost;
        }

        // Deduct energy
        stockpile.energy -= total_maintenance * d;

        // If energy goes negative, cap at 0 (penalty system later)
        if stockpile.energy < 0.0 {
            stockpile.energy = 0.0;
            // TODO: apply penalties (production halt, etc.)
        }
    }
}

pub fn advance_production_tick(clock: Res<GameClock>, mut last_tick: ResMut<LastProductionTick>) {
    last_tick.0 = clock.elapsed;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_order(minerals_cost: f64, minerals_invested: f64, energy_cost: f64, energy_invested: f64) -> BuildOrder {
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
        let order = make_order(100.0, 100.0, 50.0, 50.0);
        assert!(order.is_complete());
    }

    #[test]
    fn build_order_incomplete_minerals_short() {
        let order = make_order(100.0, 80.0, 50.0, 50.0);
        assert!(!order.is_complete());
    }

    #[test]
    fn build_order_incomplete_energy_short() {
        let order = make_order(100.0, 100.0, 50.0, 30.0);
        assert!(!order.is_complete());
    }

    #[test]
    fn build_order_incomplete_time_remaining() {
        let mut order = make_order(100.0, 100.0, 50.0, 50.0);
        order.build_time_remaining = 5;
        assert!(!order.is_complete());
    }

    #[test]
    fn mine_production_bonus() {
        let (m, e, r) = BuildingType::Mine.production_bonus();
        assert_eq!(m, 3.0);
        assert_eq!(e, 0.0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn power_plant_production_bonus() {
        let (m, e, r) = BuildingType::PowerPlant.production_bonus();
        assert_eq!(m, 0.0);
        assert_eq!(e, 3.0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn research_lab_production_bonus() {
        let (m, e, r) = BuildingType::ResearchLab.production_bonus();
        assert_eq!(m, 0.0);
        assert_eq!(e, 0.0);
        assert_eq!(r, 2.0);
    }

    #[test]
    fn shipyard_production_bonus() {
        let (m, e, r) = BuildingType::Shipyard.production_bonus();
        assert_eq!(m, 0.0);
        assert_eq!(e, 0.0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn mine_build_cost() {
        assert_eq!(BuildingType::Mine.build_cost(), (150.0, 50.0));
    }

    #[test]
    fn power_plant_build_cost() {
        assert_eq!(BuildingType::PowerPlant.build_cost(), (50.0, 150.0));
    }

    #[test]
    fn research_lab_build_cost() {
        assert_eq!(BuildingType::ResearchLab.build_cost(), (100.0, 100.0));
    }

    #[test]
    fn shipyard_build_cost() {
        assert_eq!(BuildingType::Shipyard.build_cost(), (300.0, 200.0));
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
        let (mut m, mut e, mut r) = (0.0, 0.0, 0.0);
        for slot in &buildings.slots {
            if let Some(bt) = slot {
                let (bm, be, br) = bt.production_bonus();
                m += bm;
                e += be;
                r += br;
            }
        }
        assert_eq!(m, 6.0);
        assert_eq!(e, 3.0);
        assert_eq!(r, 2.0);
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
        assert_eq!(BuildingType::Port.build_cost(), (400.0, 300.0));
    }

    #[test]
    fn port_build_time() {
        assert_eq!(BuildingType::Port.build_time(), 40);
    }

    #[test]
    fn port_production_bonus() {
        let (m, e, r) = BuildingType::Port.production_bonus();
        assert_eq!(m, 0.0);
        assert_eq!(e, 0.0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn port_name() {
        assert_eq!(BuildingType::Port.name(), "Port");
    }

    // --- #51: Maintenance cost tests ---

    #[test]
    fn building_maintenance_costs() {
        assert_eq!(BuildingType::Mine.maintenance_cost(), 0.2);
        assert_eq!(BuildingType::PowerPlant.maintenance_cost(), 0.0);
        assert_eq!(BuildingType::ResearchLab.maintenance_cost(), 0.5);
        assert_eq!(BuildingType::Shipyard.maintenance_cost(), 1.0);
        assert_eq!(BuildingType::Port.maintenance_cost(), 0.5);
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
        let mut energy = 100.0;
        let delta = 5.0;

        let mut total_maintenance = 0.0;
        for slot in &buildings.slots {
            if let Some(bt) = slot {
                total_maintenance += bt.maintenance_cost();
            }
        }
        assert!((total_maintenance - 1.2).abs() < 1e-10);

        energy -= total_maintenance * delta;
        assert!((energy - 94.0).abs() < 1e-10);
    }

    #[test]
    fn maintenance_negative_energy_capped_at_zero() {
        let mut energy = 2.0;
        let total_maintenance = 1.0;
        let delta = 5.0;

        energy -= total_maintenance * delta;
        if energy < 0.0 {
            energy = 0.0;
        }
        assert_eq!(energy, 0.0);
    }
}
