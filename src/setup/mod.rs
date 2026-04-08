use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship::{spawn_ship, ShipType};

pub struct GameSetupPlugin;

impl Plugin for GameSetupPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            spawn_initial_ships.after(crate::galaxy::generate_galaxy),
        );
    }
}

fn spawn_initial_ships(
    mut commands: Commands,
    capitals: Query<(Entity, &StarSystem, &Position)>,
) {
    let Some((capital_entity, capital_system, capital_pos)) =
        capitals.iter().find(|(_, sys, _)| sys.is_capital)
    else {
        warn!("No capital star system found; initial fleet not spawned");
        return;
    };

    let pos = *capital_pos;

    spawn_ship(&mut commands, ShipType::Explorer, "Explorer-1".to_string(), capital_entity, pos);
    spawn_ship(&mut commands, ShipType::Explorer, "Explorer-2".to_string(), capital_entity, pos);
    spawn_ship(&mut commands, ShipType::Courier, "Courier-1".to_string(), capital_entity, pos);
    spawn_ship(&mut commands, ShipType::ColonyShip, "Colony Ship-1".to_string(), capital_entity, pos);

    info!(
        "Initial fleet spawned at capital {}: 2 explorers, 1 courier, 1 colony ship",
        capital_system.name
    );
}
