mod ai;
mod amount;
mod choice;
mod colony;
mod communication;
mod components;
mod condition;
mod deep_space;
mod effect;
mod empire;
mod event_system;
mod events;
mod faction;
mod galaxy;
mod knowledge;
mod modifier;
mod notifications;
mod observer;
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

use observer::{CliArgs, ObserverMode, ObserverPlugin, RngSeed};

fn main() {
    let cli = CliArgs::parse();

    let observer_mode = ObserverMode {
        enabled: cli.no_player,
        seed: cli.seed,
        time_horizon: cli.time_horizon,
        initial_speed: cli.speed,
    };
    let rng_seed = RngSeed(cli.seed);

    if observer_mode.enabled {
        info!(
            "Starting in observer mode (--no-player): seed={:?}, time_horizon={:?}, speed={:?}",
            observer_mode.seed, observer_mode.time_horizon, observer_mode.initial_speed
        );
    }

    App::new()
        .insert_resource(observer_mode)
        .insert_resource(rng_seed)
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
            ai::AiPlugin,
            ObserverPlugin,
        ))
        .add_plugins(ui::UiPlugin)
        .run();
}
