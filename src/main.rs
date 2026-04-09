mod amount;
mod colony;
mod communication;
mod components;
mod events;
mod galaxy;
mod knowledge;
mod physics;
mod player;
mod scripting;
mod setup;
mod ship;
mod technology;
mod time_system;
mod ui;
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
            knowledge::KnowledgePlugin,
            ship::ShipPlugin,
            colony::ColonyPlugin,
            scripting::ScriptingPlugin,
            technology::TechnologyPlugin,
            events::EventsPlugin,
            setup::GameSetupPlugin,
            ui::UiPlugin,
        ))
        .run();
}
