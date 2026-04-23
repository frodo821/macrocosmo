mod ai;
mod amount;
mod casus_belli;
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
mod game_state;
mod knowledge;
mod modifier;
mod negotiation;
mod notifications;
mod observer;
mod persistence;
mod physics;
mod player;
mod profiling;
#[cfg(feature = "remote")]
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

use ai::AiPlayerMode;
use game_state::{LoadSaveRequest, NewGameParams};
use observer::{CliArgs, ObserverMode, ObserverPlugin, RngSeed};

fn main() {
    let cli = CliArgs::parse();

    let observer_mode = ObserverMode {
        enabled: cli.no_player || cli.observer,
        seed: cli.seed,
        time_horizon: cli.time_horizon,
        initial_speed: cli.speed,
        read_only: cli.observer,
    };
    let rng_seed = RngSeed(cli.seed);

    if observer_mode.enabled {
        let source = if cli.observer {
            "--observer"
        } else {
            "--no-player"
        };
        info!(
            "Starting in observer mode ({source}): seed={:?}, time_horizon={:?}, speed={:?}, read_only={}",
            observer_mode.seed, observer_mode.time_horizon, observer_mode.initial_speed, observer_mode.read_only
        );
    }

    // --no-player implies --ai-player: the player empire is AI-driven.
    let ai_player_mode = AiPlayerMode(cli.ai_player || cli.no_player);

    // #439 Phase 1: mirror the CLI flags into `NewGameParams` so the
    // `OnEnter(NewGame)` world-spawn systems (Phase 3) have a single
    // source of truth. Until then, `RngSeed` / `ObserverMode` stay as
    // the canonical resources — this is read-only context.
    let new_game_params = NewGameParams {
        seed: cli.seed,
        scenario_id: None,
        observer_mode: observer_mode.enabled,
        faction_override: None,
    };

    let mut app = App::new();
    // `GameState` is registered by `GameSetupPlugin` (via
    // `GameStatePlugin`) later in the add_plugins chain. `StatesPlugin`
    // comes in via `DefaultPlugins`, which is installed immediately
    // below — keeping `init_state` off of `main()` means there's only
    // one place that owns state registration, and the order is
    // implicit in the plugin graph.
    app.insert_resource(new_game_params)
        .insert_resource(observer_mode)
        .insert_resource(rng_seed)
        .insert_resource(ai_player_mode);

    // #439 Phase 3: `--load <path>` inserts a `LoadSaveRequest`.
    // `dispatch_initial_state` routes to `GameState::LoadingSave` when
    // this resource is present, and `perform_load` applies the save.
    if let Some(path) = cli.load.clone() {
        info!("Loading save from {:?}", path);
        app.insert_resource(LoadSaveRequest { path });
    }

    app
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
            casus_belli::CasusBelliPlugin,
            ObserverPlugin,
        ))
        .add_plugins(ui::UiPlugin);

    #[cfg(feature = "remote")]
    {
        app.add_plugins(remote::remote_plugin());
        app.add_plugins(bevy::remote::http::RemoteHttpPlugin::default());
        app.init_resource::<remote::PendingInputReleases>();
        app.init_resource::<remote::ScreenshotBuffer>();
        app.init_resource::<ui::UiElementRegistry>();
        app.add_systems(PreUpdate, remote::release_pending_inputs);
        app.add_systems(PreUpdate, remote::clear_ui_element_registry);
        info!("BRP remote server enabled on localhost:15702");
    }

    app.run();
}
