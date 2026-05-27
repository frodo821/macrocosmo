use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::game_state::{GameState, NewGameParams};
use macrocosmo::observer::{ObserverMode, ObserverModeKind, RngSeed};
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::simulation::SimulationPlugin;
use macrocosmo::time_system::GameClock;

#[test]
fn simulation_plugin_runs_headless_without_interactions() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(NewGameParams {
        seed: Some(0xC0FFEE),
        observer_mode: false,
        ..Default::default()
    });
    app.insert_resource(ObserverMode {
        kind: ObserverModeKind::Disabled,
        ..Default::default()
    });
    app.insert_resource(RngSeed(Some(0xC0FFEE)));
    app.insert_resource(AiPlayerMode(false));
    app.add_plugins(SimulationPlugin);

    app.update();
    app.update();

    assert!(
        app.world().contains_resource::<GameClock>(),
        "SimulationPlugin should initialize the game clock"
    );
    assert!(
        !app.world().contains_resource::<ButtonInput<KeyCode>>(),
        "SimulationPlugin should run without installing input resources"
    );
    assert_eq!(
        app.world().resource::<State<GameState>>().get(),
        &GameState::InGame,
        "headless simulation setup should reach InGame"
    );

    let mut player_empires = app
        .world_mut()
        .query_filtered::<Entity, (With<Empire>, With<PlayerEmpire>)>();
    let count = player_empires.iter(app.world()).count();
    assert_eq!(
        count, 1,
        "headless player-mode simulation should spawn one player empire"
    );
}
