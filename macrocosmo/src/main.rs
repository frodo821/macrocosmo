mod amount;
mod choice;
mod colony;
mod communication;
mod components;
mod condition;
mod deep_space;
mod effect;
mod event_system;
mod events;
mod faction;
mod galaxy;
mod knowledge;
mod modifier;
mod notifications;
mod physics;
mod player;
mod scripting;
mod setup;
mod species;
mod ship;
mod ship_design;
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
            event_system::EventSystemPlugin,
            events::EventsPlugin,
            species::SpeciesPlugin,
            ship_design::ShipDesignPlugin,
        ))
        .add_plugins((
            deep_space::DeepSpacePlugin,
            setup::GameSetupPlugin,
            notifications::NotificationsPlugin,
            faction::FactionRelationsPlugin,
            choice::ChoicesPlugin,
        ))
        .add_plugins(ui::UiPlugin)
        .run();
}
