mod galaxy;
mod player;
mod communication;
mod time_system;
mod physics;
mod visualization;

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Macrocosmo".into(),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            time_system::GameTimePlugin,
            galaxy::GalaxyPlugin,
            player::PlayerPlugin,
            communication::CommunicationPlugin,
            visualization::VisualizationPlugin,
        ))
        .run();
}
