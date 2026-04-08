use bevy::prelude::*;

use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{StarSystem, SystemAttributes, Sovereignty};
use crate::ship::{spawn_ship, Owner, ShipType};
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
                    tick_population_growth,
                    tick_build_queue,
                    tick_building_queue,
                    advance_production_tick,
                )
                    .chain(),
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
}

#[derive(Component)]
pub struct Production {
    pub minerals_per_sexadie: f64,
    pub energy_per_sexadie: f64,
    pub research_per_sexadie: f64,
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
    /// #32: Total build time in sexadies
    pub build_time_total: i64,
    /// #32: Remaining build time in sexadies
    pub build_time_remaining: i64,
}

impl BuildOrder {
    pub fn is_complete(&self) -> bool {
        self.minerals_invested >= self.minerals_cost
            && self.energy_invested >= self.energy_cost
            && self.build_time_remaining <= 0
    }

    /// Returns the build time in sexadies for a given ship type name.
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
}

impl BuildingType {
    pub fn production_bonus(&self) -> (f64, f64, f64) {
        // (minerals, energy, research) per sexadie
        match self {
            BuildingType::Mine => (3.0, 0.0, 0.0),
            BuildingType::PowerPlant => (0.0, 3.0, 0.0),
            BuildingType::ResearchLab => (0.0, 0.0, 2.0),
            BuildingType::Shipyard => (0.0, 0.0, 0.0), // special effect
        }
    }

    pub fn build_cost(&self) -> (f64, f64) {
        // (minerals, energy)
        match self {
            BuildingType::Mine => (150.0, 50.0),
            BuildingType::PowerPlant => (50.0, 150.0),
            BuildingType::ResearchLab => (100.0, 100.0),
            BuildingType::Shipyard => (300.0, 200.0),
        }
    }

    pub fn build_time(&self) -> i64 {
        // sexadies to build
        match self {
            BuildingType::Mine => 10,
            BuildingType::PowerPlant => 10,
            BuildingType::ResearchLab => 15,
            BuildingType::Shipyard => 30,
        }
    }

    /// Display name for the building type.
    pub fn name(&self) -> &'static str {
        match self {
            BuildingType::Mine => "Mine",
            BuildingType::PowerPlant => "PowerPlant",
            BuildingType::ResearchLab => "ResearchLab",
            BuildingType::Shipyard => "Shipyard",
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
                },
                Production {
                    minerals_per_sexadie: 5.0,
                    energy_per_sexadie: 5.0,
                    research_per_sexadie: 1.0,
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
pub fn tick_production(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(&Production, &mut ResourceStockpile, Option<&Buildings>, Option<&ProductionFocus>)>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as f64;
    for (prod, mut stockpile, buildings, focus) in &mut query {
        let (mut bonus_m, mut bonus_e, mut bonus_r) = (0.0, 0.0, 0.0);
        if let Some(buildings) = buildings {
            for slot in &buildings.slots {
                if let Some(bt) = slot {
                    let (m, e, r) = bt.production_bonus();
                    bonus_m += m;
                    bonus_e += e;
                    bonus_r += r;
                }
            }
        }
        let (mw, ew, rw) = match focus {
            Some(f) => (f.minerals_weight, f.energy_weight, f.research_weight),
            None => (1.0, 1.0, 1.0),
        };
        stockpile.minerals += (prod.minerals_per_sexadie + bonus_m) * mw * d;
        stockpile.energy += (prod.energy_per_sexadie + bonus_e) * ew * d;
        stockpile.research += (prod.research_per_sexadie + bonus_r) * rw * d;
    }
}

pub fn tick_population_growth(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<&mut Colony>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    for mut colony in &mut query {
        let growth_factor = (1.0 + colony.growth_rate).powi(delta as i32);
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
}
