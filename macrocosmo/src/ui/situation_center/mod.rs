//! Empire Situation Center (ESC) — #326 epic, ESC-1 (#344).
//!
//! This module provides the framework that ESC-2 (Notifications tab +
//! bridge, #345) and ESC-3 (four ongoing tabs, #346) build on. The
//! public surface is intentionally narrow:
//!
//! * Type-level: [`Event`], [`Notification`], [`EventSource`],
//!   [`NotificationSource`], [`Severity`], [`EventKind`].
//! * Trait-level: [`SituationTab`], [`OngoingTab`], plus the
//!   [`AppSituationExt`] extension trait for registration.
//! * Registry / state: [`SituationTabRegistry`],
//!   [`SituationCenterState`].
//! * Plugin: [`SituationCenterPlugin`], wired into `UiPlugin`.
//!
//! Consumer docs live in `docs/plan-326-esc.md`. See that file for the
//! Lua API future-proof argument (why the traits here are stable under
//! the upcoming `define_situation_tab` API).

pub mod lua_adapter;
pub mod notifications_tab;
pub mod panel;
pub mod registry;
pub mod state;
pub mod tab;
pub mod types;

use bevy::prelude::*;

pub use lua_adapter::{LuaOngoingTabAdapter, LuaTabRegistration};
pub use notifications_tab::{
    EscNotificationQueue, NotificationsTab, PendingAck, PushOutcome, apply_pending_acks_system,
    drain_pending_acks_for_tests, enqueue_pending_ack,
};
pub use panel::{TOGGLE_KEY, draw_situation_center_system, toggle_situation_center};
pub use registry::{AppSituationExt, SituationTabRegistry};
pub use state::{SituationCenterState, TabState};
pub use tab::{
    OngoingTab, OngoingTabAdapter, SituationTab, TabBadge, TabId, TabMeta, render_event_tree,
};
pub use types::{
    BuildOrderId, Event, EventId, EventKind, EventSource, GameTime, Notification, NotificationId,
    NotificationSource, Severity, severity_max,
};

/// Plugin that installs the ESC framework.
///
/// Registers:
/// * [`SituationCenterState`] + [`SituationTabRegistry`] + [`EscNotificationQueue`] resources.
/// * The `NotificationsTab` (framework-bundled).
/// * The F3 toggle system in `Update`.
/// * The panel draw system in `EguiPrimaryContextPass`, chained after
///   overlays so it sits alongside Research panel / Ship Designer in
///   the floating-window slot.
///
/// ESC-2 (#345) adds the push / bridge / ack wiring for the
/// notifications queue. ESC-3 (#346) registers the four bundled
/// ongoing tabs. Neither requires framework refactor — see
/// `docs/plan-326-esc.md`.
pub struct SituationCenterPlugin;

impl Plugin for SituationCenterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SituationCenterState>()
            .init_resource::<SituationTabRegistry>()
            .init_resource::<EscNotificationQueue>()
            .add_systems(Update, toggle_situation_center)
            // #345 ESC-2: drain ack intents emitted by the tab renderer
            // each frame and apply them to the queue. Registered in
            // `Update` rather than `EguiPrimaryContextPass` so the
            // render path can fire ack buttons in frame N and the
            // queue reflects them at the start of frame N+1's game
            // systems (ordering mirrors `toggle_situation_center`).
            .add_systems(Update, apply_pending_acks_system);

        // Register the framework-bundled Notifications tab. ESC-2
        // swaps the stub queue for the real pipeline but keeps this
        // registration intact.
        app.register_situation_tab(NotificationsTab);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn plugin_installs_core_resources_and_notifications_tab() {
        let mut app = App::new();
        // `ButtonInput` is needed by `toggle_situation_center`'s
        // system parameter validation — Bevy will not let the system
        // register without the resource present.
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.add_plugins(SituationCenterPlugin);

        // Run one frame so the Update schedule validates; no panic ⇒
        // the toggle system has all its resources.
        app.update();

        assert!(app.world().contains_resource::<SituationCenterState>());
        assert!(app.world().contains_resource::<SituationTabRegistry>());
        assert!(app.world().contains_resource::<EscNotificationQueue>());

        let registry = app.world().resource::<SituationTabRegistry>();
        assert!(
            registry.get(NotificationsTab::ID).is_some(),
            "NotificationsTab must be bundled by the plugin",
        );
    }

    /// End-to-end registry chain: plugin install → register a dummy
    /// ongoing tab → retrieve it via `AppSituationExt`.
    #[test]
    fn register_ongoing_tab_after_plugin() {
        // Alias `Event` (our struct) to avoid ambiguity with
        // `bevy::ecs::event::Event` imported via `bevy::prelude::*` below.
        use crate::ui::situation_center::types::Event as EscEvent;

        struct DummyTab;
        impl OngoingTab for DummyTab {
            fn meta(&self) -> TabMeta {
                TabMeta {
                    id: "dummy",
                    display_name: "Dummy",
                    order: 42,
                }
            }
            fn collect(&self, _world: &World) -> Vec<EscEvent> {
                Vec::new()
            }
        }

        let mut app = App::new();
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.add_plugins(SituationCenterPlugin);
        app.register_ongoing_situation_tab(DummyTab);

        let registry = app.world().resource::<SituationTabRegistry>();
        assert!(registry.get("dummy").is_some());
    }
}
