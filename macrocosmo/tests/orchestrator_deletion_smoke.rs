//! #449 PR2c: smoke test asserting that the game-side
//! `OrchestratorRegistry` / `Orchestrator` integration has been fully
//! retired. Catches regressions where a future PR re-adds the
//! game-side type bindings — the engine-agnostic
//! `macrocosmo_ai::orchestrator::Orchestrator` type still exists for
//! the abstract scenario harness, but the **game** must not depend on
//! it.
//!
//! The assertion shape is "does the test compile + the resource is
//! absent at startup?". A direct grep for the symbols in src/ is done
//! in CI; this companion smoke catches any future re-introduction
//! through the real plugin bootstrap path.

mod common;

use bevy::prelude::*;

use common::test_app;

/// `AiPlugin::build` no longer initialises `OrchestratorRegistry` —
/// asserting via the world (rather than `cfg`) ensures the resource
/// type is genuinely gone, not just renamed.
#[test]
fn ai_plugin_does_not_install_orchestrator_registry_resource() {
    let mut app = test_app();
    app.update();

    // We cannot reference the deleted `OrchestratorRegistry` type
    // directly (that's the point — it doesn't exist anymore). Instead
    // we assert by name through the type registry, which Bevy
    // populates from `register_type` calls. The deleted call was
    // `app.register_type::<OrchestratorRegistry>()` in
    // `reflect_registration::register_all_reflect_types`.
    let type_registry = app.world().resource::<AppTypeRegistry>().read();
    let still_registered = type_registry
        .iter()
        .any(|info: &bevy::reflect::TypeRegistration| {
            info.type_info()
                .type_path()
                .ends_with("OrchestratorRegistry")
        });
    assert!(
        !still_registered,
        "OrchestratorRegistry must not appear in the type registry — \
         the game-side orchestrator integration is removed in PR2c"
    );
}

/// `AiPlugin::build` registers the new ShortAgent driver
/// (`run_short_agents`) instead of `run_orchestrators`. We can't
/// reference the deleted system fn directly; this test asserts the
/// new system is in place by spawning the smoke world and observing
/// that no panic / unknown-system error fires through one Update.
#[test]
fn ai_plugin_smoke_runs_clean_after_orchestrator_removal() {
    let mut app = test_app();
    // Drive the schedule a few times — `run_short_agents` is wired
    // under `AiTickSet::Reason` and would panic at schedule build
    // time if `dispatch_ai_pending_commands.after(run_short_agents)`
    // referenced a non-registered system.
    for _ in 0..3 {
        app.update();
    }
}
