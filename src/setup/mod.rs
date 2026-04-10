use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::player::PlayerEmpire;
use crate::ship::{spawn_ship, Owner, ShipType};

pub struct GameSetupPlugin;

impl Plugin for GameSetupPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            spawn_initial_ships
                .after(crate::galaxy::generate_galaxy)
                .after(crate::player::spawn_player_empire),
        );
    }
}

fn spawn_initial_ships(
    mut commands: Commands,
    capitals: Query<(Entity, &StarSystem, &Position)>,
    empire_q: Query<Entity, With<PlayerEmpire>>,
) {
    let Some((capital_entity, capital_system, capital_pos)) =
        capitals.iter().find(|(_, sys, _)| sys.is_capital)
    else {
        warn!("No capital star system found; initial fleet not spawned");
        return;
    };

    let owner = match empire_q.single() {
        Ok(empire_entity) => Owner::Empire(empire_entity),
        Err(_) => {
            warn!("No player empire found; ships will have neutral owner");
            Owner::Neutral
        }
    };

    let pos = *capital_pos;

    spawn_ship(&mut commands, ShipType::Explorer, "Explorer-1".to_string(), capital_entity, pos, owner);
    spawn_ship(&mut commands, ShipType::Explorer, "Explorer-2".to_string(), capital_entity, pos, owner);
    spawn_ship(&mut commands, ShipType::Courier, "Courier-1".to_string(), capital_entity, pos, owner);
    spawn_ship(&mut commands, ShipType::ColonyShip, "Colony Ship-1".to_string(), capital_entity, pos, owner);

    info!(
        "Initial fleet spawned at capital {}: 2 explorers, 1 courier, 1 colony ship",
        capital_system.name
    );
}
