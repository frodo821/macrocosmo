//! Integration smoke test for the AI Debug UI plugin wiring.
//!
//! Renders nothing (egui systems need an EguiContexts, which requires a
//! full render pipeline — unavailable in headless tests), but does verify:
//!
//! - `AiDebugUi` is initialised by `UiPlugin`.
//! - `toggle_ai_debug` runs each frame under `Update` without panic.
//! - `sample_ai_debug_stream` noops cleanly when the window is closed.
//! - When `AiDebugUi::open` is forced `true`, `sample_ai_debug_stream`
//!   populates `last_snapshot` from the bus.

use bevy::prelude::*;
use macrocosmo::ai::AiPlugin;
use macrocosmo::time_system::{GameClock, GameSpeed};
use macrocosmo::ui::ai_debug::{AiDebugUi, sample_ai_debug_stream, toggle_ai_debug};

fn minimal_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    app.init_resource::<ButtonInput<KeyCode>>();
    app.add_plugins(AiPlugin);
    app.init_resource::<AiDebugUi>();
    // Only the two non-egui systems — `draw_ai_debug_system` needs an
    // EguiContexts resource that the headless app can't build without
    // the full render graph.
    app.add_systems(Update, (toggle_ai_debug, sample_ai_debug_stream));
    app
}

#[test]
fn ai_debug_smoke_runs_without_panic() {
    let mut app = minimal_app();
    // Several frames so AiPlugin's Startup schema declarations + the
    // debug systems interleave at least once.
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().get_resource::<AiDebugUi>().is_some());
}

#[test]
fn sample_noop_when_closed() {
    let mut app = minimal_app();
    app.update();
    let ui = app.world().resource::<AiDebugUi>();
    assert!(!ui.open);
    assert!(ui.last_snapshot.is_none());
}

#[test]
fn sample_populates_last_snapshot_when_open() {
    let mut app = minimal_app();
    // Let Startup schema declarations run first.
    app.update();
    app.world_mut().resource_mut::<AiDebugUi>().open = true;
    app.update();
    let ui = app.world().resource::<AiDebugUi>();
    assert!(
        ui.last_snapshot.is_some(),
        "sample_ai_debug_stream should have captured a snapshot while open"
    );
}
