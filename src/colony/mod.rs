use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship::{spawn_ship, ShipType};
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
                    advance_production_tick,
                )
                    .chain(),
            );
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
}

impl BuildOrder {
    pub fn is_complete(&self) -> bool {
        self.minerals_invested >= self.minerals_cost && self.energy_invested >= self.energy_cost
    }
}

pub fn spawn_capital_colony(mut commands: Commands, query: Query<(Entity, &StarSystem)>) {
    for (entity, system) in query.iter() {
        if system.is_capital {
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
            ));
            info!("Capital colony spawned on {}", system.name);
            return;
        }
    }
    warn!("No capital star system found; capital colony not created");
}

fn tick_production(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(&Production, &mut ResourceStockpile)>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as f64;
    for (prod, mut stockpile) in &mut query {
        stockpile.minerals += prod.minerals_per_sexadie * d;
        stockpile.energy += prod.energy_per_sexadie * d;
        stockpile.research += prod.research_per_sexadie * d;
    }
}

fn tick_population_growth(
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

fn tick_build_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(&Colony, &mut BuildQueue, &mut ResourceStockpile)>,
    positions: Query<&Position>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    for (colony, mut build_queue, mut stockpile) in &mut query {
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
                    info!("Ship built and launched: {}", completed.ship_type_name);
                }
            }
        }
    }
}

fn advance_production_tick(clock: Res<GameClock>, mut last_tick: ResMut<LastProductionTick>) {
    last_tick.0 = clock.elapsed;
}
