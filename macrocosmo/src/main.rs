use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::game_state::{LoadSaveRequest, NewGameParams};
use macrocosmo::interactions::InteractionsPlugin;
use macrocosmo::observer::{CliArgs, ObserverMode, ObserverModeKind, RngSeed};
use macrocosmo::simulation::SimulationPlugin;

fn main() {
    let cli = CliArgs::parse();

    let observer_mode = ObserverMode {
        kind: if cli.no_player || cli.observer {
            ObserverModeKind::EmpireView
        } else {
            ObserverModeKind::Disabled
        },
        seed: cli.seed,
        time_horizon: cli.time_horizon,
        initial_speed: cli.speed,
        read_only: cli.observer,
        previous_kind: None,
    };
    let rng_seed = RngSeed(cli.seed);

    if observer_mode.is_empire_view() {
        let source = if cli.observer {
            "--observer"
        } else {
            "--no-player"
        };
        info!(
            "Starting in observer mode ({source}): seed={:?}, time_horizon={:?}, speed={:?}, read_only={}",
            observer_mode.seed,
            observer_mode.time_horizon,
            observer_mode.initial_speed,
            observer_mode.read_only
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
        observer_mode: observer_mode.is_empire_view(),
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

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Macrocosmo".into(),
            resolution: (1280, 720).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins((SimulationPlugin, InteractionsPlugin));

    app.run();
}
