use bevy::prelude::*;

use crate::observer::{
    NonOmniscientKind, ObserverMode, ObserverModeKind, ObserverView, RngSeed, in_observer_mode,
};

/// Registers observer systems owned by the interaction layer.
pub struct ObserverControlsPlugin;

impl Plugin for ObserverControlsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ObserverMode>()
            .init_resource::<ObserverView>()
            .init_resource::<RngSeed>()
            .add_systems(
                Update,
                (esc_to_exit, sync_observer_view_to_governor).run_if(in_observer_mode),
            )
            // #490: Omniscient toggle (default F9). Runs in every
            // observer-spawn-architecture mode so a dev can flip into
            // god view from either normal play or `--no-player`. The
            // `run_if(in_state(InGame))` keeps the toggle inert during
            // main-menu / loading, matching other UI-action systems.
            .add_systems(
                Update,
                toggle_omniscient_mode.run_if(in_state(crate::game_state::GameState::InGame)),
            );
    }
}

/// Immediate exit on the configured "observer.exit" keybind (default
/// Escape). The key-input resource is optional so this system is inert
/// in headless test apps that don't register `InputPlugin`. The
/// keybinding registry is also optional so observer-mode tests that don't
/// install `KeybindingPlugin` still see the legacy Escape behaviour.
pub fn esc_to_exit(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(keys) = keys else { return };
    let pressed = match keybindings.as_deref() {
        Some(kb) => kb.is_just_pressed(crate::input::actions::OBSERVER_EXIT, &keys),
        None => keys.just_pressed(KeyCode::Escape),
    };
    if pressed {
        info!("Observer mode: exit keybind pressed, exiting");
        exit.write(AppExit::Success);
    }
}

/// One-way mirror from `ObserverView.viewing` (Faction entity) to
/// `AiDebugUi::GovernorState::faction` (`u32` from `to_ai_faction`). This
/// makes the F10 Governor tab follow the top-bar selector.
///
/// The `AiDebugUi` resource is optional so this system can run in
/// headless test apps that don't register `UiPlugin`.
pub fn sync_observer_view_to_governor(
    view: Res<ObserverView>,
    ui: Option<ResMut<crate::ui::ai_debug::AiDebugUi>>,
) {
    let Some(mut ui) = ui else {
        return;
    };
    if let Some(faction_entity) = view.viewing {
        let id = crate::ai::convert::to_ai_faction(faction_entity);
        ui.governor.faction = id.0;
    }
}

/// #490: Toggle [`ObserverModeKind::Omniscient`] on/off.
///
/// * If currently `Omniscient` -> restore `previous_kind` (or `Disabled`
///   if unset).
/// * Otherwise -> save current kind into `previous_kind` and switch to
///   `Omniscient`.
///
/// Bound to `ui.toggle_omniscient` (default `F9`) via the keybinding
/// registry. Falls back to a hardcoded `F9` check when the registry is
/// not present (headless tests with no `KeybindingPlugin`).
pub fn toggle_omniscient_mode(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    mut mode: ResMut<ObserverMode>,
) {
    // `ButtonInput<KeyCode>` is missing in headless test apps that only
    // load `MinimalPlugins`; the wrapper lets the system be a no-op in
    // that case instead of panicking on missing resource.
    let Some(keys) = keys else {
        return;
    };
    let pressed = match keybindings.as_deref() {
        Some(kb) => kb.is_just_pressed(crate::input::actions::UI_TOGGLE_OMNISCIENT, &keys),
        None => keys.just_pressed(KeyCode::F9),
    };
    if !pressed {
        return;
    }
    if mode.is_omniscient() {
        let restore = mode
            .previous_kind
            .take()
            .unwrap_or(NonOmniscientKind::Disabled);
        mode.kind = restore.to_observer_kind();
        info!("Omniscient mode OFF (restored {:?})", mode.kind);
    } else {
        mode.previous_kind = NonOmniscientKind::from_observer_kind(mode.kind);
        mode.kind = ObserverModeKind::Omniscient;
        info!("Omniscient mode ON (god view)");
    }
}
