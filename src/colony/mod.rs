use bevy::prelude::*;

use crate::galaxy::StarSystem;

pub struct ColonyPlugin;

impl Plugin for ColonyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_capital_colony.after(crate::galaxy::generate_galaxy));
    }
}

/// A colony on a star system
#[derive(Component)]
pub struct Colony {
    /// The star system entity this colony is in
    pub system: Entity,
    /// Population (abstract units)
    pub population: f64,
    /// Base growth rate per sexadie
    pub growth_rate: f64,
}

/// Resource stockpile for a colony
#[derive(Component)]
pub struct ResourceStockpile {
    pub minerals: f64,
    pub energy: f64,
    pub research: f64,
}

/// Production rates per sexadie
#[derive(Component)]
pub struct Production {
    pub minerals_per_sexadie: f64,
    pub energy_per_sexadie: f64,
    pub research_per_sexadie: f64,
}

/// Ship construction queue
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
        self.minerals_invested >= self.minerals_cost
            && self.energy_invested >= self.energy_cost
    }
}

fn spawn_capital_colony(
    mut commands: Commands,
    query: Query<(Entity, &StarSystem)>,
) {
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
