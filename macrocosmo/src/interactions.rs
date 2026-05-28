use bevy::prelude::*;

pub mod esc_notifications;
pub mod observer_controls;
pub mod player_controls;
pub mod time_controls;

/// Human/tool interaction layer: input, rendering, UI, observer controls, and remote access.
pub struct InteractionsPlugin;

impl Plugin for InteractionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            crate::input::KeybindingPlugin,
            crate::visualization::VisualizationPlugin,
            observer_controls::ObserverControlsPlugin,
            crate::reflect_registration::ReflectRegistrationPlugin,
            crate::ui::UiPlugin,
        ))
        .add_plugins((
            time_controls::TimeControlsPlugin,
            player_controls::PlayerControlsPlugin,
        ))
        // #345 ESC-2: drain the Lua notification accumulator into the
        // UI-owned Situation Center queue. This remains outside
        // `ScriptingPlugin` so headless simulation can run without UI
        // resources.
        .add_systems(
            Update,
            esc_notifications::drain_pending_esc_notifications
                .after(crate::time_system::advance_game_time)
                .after(crate::scripting::knowledge_dispatch::dispatch_knowledge_observed)
                .before(crate::knowledge::sweep_notified_event_ids),
        );

        #[cfg(feature = "remote")]
        {
            app.add_plugins(crate::remote::remote_plugin());
            app.add_plugins(bevy::remote::http::RemoteHttpPlugin::default());
            app.init_resource::<crate::remote::PendingInputReleases>();
            app.init_resource::<crate::remote::ScreenshotBuffer>();
            app.init_resource::<crate::ui::UiElementRegistry>();
            app.add_systems(PreUpdate, crate::remote::release_pending_inputs);
            app.add_systems(PreUpdate, crate::remote::clear_ui_element_registry);
            info!("BRP remote server enabled on localhost:15702");
        }
    }
}
