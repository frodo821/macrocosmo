//! Observer mode (#214) — run the game without a Player entity.
//!
//! Observer mode is activated via `--no-player` on the command line. It:
//!
//! * spawns one full NPC Empire per registered Faction (no `PlayerEmpire`);
//! * gates player-specific systems and command-issuance UI;
//! * provides a top-bar faction selector synced with the AI Debug UI Governor tab;
//! * exits automatically on `--time-horizon`, all-empires-eliminated, or Esc.
//!
//! Reproducibility helpers (also available outside observer mode):
//!
//! * `--seed N` — deterministic galaxy generation seed
//! * `--speed S` — initial game speed (hexadies per real second)
//!
//! See `cli.rs` for the CLI parser.

pub mod cli;
mod exit;

pub use cli::CliArgs;
pub use exit::{check_all_empires_eliminated, check_time_horizon, esc_to_exit};

use bevy::prelude::*;

use crate::time_system::GameSpeed;

/// Global observer-mode resource. `enabled = false` in normal play.
#[derive(Resource, Debug, Clone, Default, Reflect)]
#[reflect(Resource)]
pub struct ObserverMode {
    pub enabled: bool,
    /// Optional deterministic seed (copied from `RngSeed` for convenience).
    pub seed: Option<u64>,
    /// Auto-exit hexadies. `None` = manual termination only.
    pub time_horizon: Option<i64>,
    /// Initial `GameSpeed.hexadies_per_second`. Applied at Startup.
    pub initial_speed: Option<f64>,
    /// When `true`, the UI is read-only: context menus and ship panel
    /// commands are suppressed. Set by `--observer`.
    pub read_only: bool,
}

/// Current faction the observer is inspecting. One-way mirrored to
/// `AiDebugUi::governor::GovernorState::faction` so the F10 panel follows
/// the top-bar selector.
#[derive(Resource, Debug, Clone, Default, Reflect)]
#[reflect(Resource)]
pub struct ObserverView {
    /// The `Faction` entity being focused. `None` until the selector has
    /// been initialised from the spawned empire list.
    pub viewing: Option<Entity>,
}

/// Global RNG seed for galaxy generation. Populated from the CLI whether
/// or not observer mode is enabled so the flag is useful for bug repros.
#[derive(Resource, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Resource)]
pub struct RngSeed(pub Option<u64>);

/// Run-condition: observer mode is active.
pub fn in_observer_mode(o: Res<ObserverMode>) -> bool {
    o.enabled
}

/// Run-condition: observer mode is not active (normal single-player).
pub fn not_in_observer_mode(o: Res<ObserverMode>) -> bool {
    !o.enabled
}

/// Bevy plugin that registers observer resources, exit systems, and
/// wiring that must run regardless of whether observer mode is enabled
/// (run-conditions short-circuit inside each system).
pub struct ObserverPlugin;

impl Plugin for ObserverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ObserverMode>()
            .init_resource::<ObserverView>()
            .init_resource::<RngSeed>()
            // #439 Phase 3: gameplay config (initial speed) is a
            // new-game construction step.
            .add_systems(
                OnEnter(crate::game_state::GameState::NewGame),
                apply_initial_speed.run_if(in_observer_mode),
            )
            .add_systems(
                Update,
                (
                    check_time_horizon,
                    check_all_empires_eliminated,
                    esc_to_exit,
                    sync_observer_view_to_governor,
                )
                    .run_if(in_observer_mode),
            );
    }
}

/// Startup system that applies `ObserverMode.initial_speed` to
/// `GameSpeed`. Gated on `in_observer_mode` at registration.
pub fn apply_initial_speed(mode: Res<ObserverMode>, mut speed: ResMut<GameSpeed>) {
    if let Some(s) = mode.initial_speed {
        speed.hexadies_per_second = s;
        if s > 0.0 {
            speed.previous_speed = s;
        }
        info!("Observer mode: initial speed set to {} hd/s", s);
    }
}

/// Run-condition: observer mode is active AND read-only.
pub fn in_observer_read_only(o: Res<ObserverMode>) -> bool {
    o.enabled && o.read_only
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_mode_default_is_inactive() {
        let mode = ObserverMode::default();
        assert!(!mode.enabled);
        assert!(!mode.read_only);
        assert!(mode.seed.is_none());
        assert!(mode.time_horizon.is_none());
        assert!(mode.initial_speed.is_none());
    }

    #[test]
    fn observer_mode_read_only_field() {
        let mode = ObserverMode {
            enabled: true,
            read_only: true,
            ..Default::default()
        };
        assert!(mode.enabled);
        assert!(mode.read_only);
    }

    #[test]
    fn observer_mode_no_player_without_read_only() {
        // --no-player sets enabled=true but read_only=false
        let mode = ObserverMode {
            enabled: true,
            read_only: false,
            ..Default::default()
        };
        assert!(mode.enabled);
        assert!(!mode.read_only);
    }
}
