//! Observer mode exit conditions.
//!
//! * `check_time_horizon` — auto-exit once `GameClock.elapsed` reaches the
//!   user-supplied `--time-horizon`.
//! * `check_all_empires_eliminated` — auto-exit once no `Empire` entities
//!   remain. Gated on `clock.elapsed > 0` to avoid firing on the first
//!   frame before NPC empires spawn.
//! * `esc_to_exit` — immediate exit on Escape key press.
//!
//! All three are registered by [`crate::observer::ObserverPlugin`] with
//! `.run_if(in_observer_mode)`.

use bevy::prelude::*;

use super::ObserverMode;
use crate::player::Empire;
use crate::time_system::GameClock;

/// Exit once `GameClock.elapsed >= mode.time_horizon` (if set).
pub fn check_time_horizon(
    mode: Res<ObserverMode>,
    clock: Res<GameClock>,
    mut exit: MessageWriter<AppExit>,
) {
    if let Some(horizon) = mode.time_horizon {
        if clock.elapsed >= horizon {
            info!(
                "Observer mode: time horizon {} reached (elapsed={}), exiting",
                horizon, clock.elapsed
            );
            exit.write(AppExit::Success);
        }
    }
}

/// Exit once every `Empire` has been despawned. Only triggers after the
/// first hexadies has elapsed so we don't fire during Startup, before
/// `run_all_factions_on_game_start` has run.
pub fn check_all_empires_eliminated(
    clock: Res<GameClock>,
    empires: Query<(), With<Empire>>,
    mut exit: MessageWriter<AppExit>,
) {
    if clock.elapsed <= 0 {
        return;
    }
    if empires.iter().next().is_none() {
        info!("Observer mode: all empires eliminated, exiting");
        exit.write(AppExit::Success);
    }
}

/// Immediate exit on Escape key.
pub fn esc_to_exit(keys: Res<ButtonInput<KeyCode>>, mut exit: MessageWriter<AppExit>) {
    if keys.just_pressed(KeyCode::Escape) {
        info!("Observer mode: Esc pressed, exiting");
        exit.write(AppExit::Success);
    }
}
