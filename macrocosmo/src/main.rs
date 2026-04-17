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
mod profiling;
mod remote;
mod scripting;
mod setup;
mod ship;
mod ship_design;
mod species;
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

    let mut app = App::new();
    app.insert_resource(observer_mode)
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
        .add_plugins(ui::UiPlugin);

    #[cfg(feature = "remote")]
    {
        use remote::remote_commands::*;
        app.add_plugins(
            bevy::remote::RemotePlugin::default()
                .with_method(ENTITY_SCREEN_POS_METHOD, process_entity_screen_pos)
                .with_method(ADVANCE_TIME_METHOD, process_advance_time)
                .with_method(EVAL_LUA_METHOD, process_eval_lua),
        );
        app.add_plugins(bevy::remote::http::RemoteHttpPlugin::default());
        info!("BRP remote server enabled on localhost:15702");
    }

    app.run();
}
