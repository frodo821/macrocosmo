use bevy::prelude::*;

use crate::time_system::{GameClock, GameSpeed};

pub struct TimeControlsPlugin;

impl Plugin for TimeControlsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, handle_speed_controls);
    }
}

pub fn handle_speed_controls(
    clock: Res<GameClock>,
    keys: Res<ButtonInput<KeyCode>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    mut speed: ResMut<GameSpeed>,
) {
    let mut changed = false;

    // #347: lookups via the keybinding registry. The registry resource is
    // optional so headless tests that don't install `KeybindingPlugin`
    // remain functional — they just won't see any speed-control keypresses.
    let Some(keybindings) = keybindings else {
        return;
    };
    use crate::input::actions;

    if keybindings.is_just_pressed(actions::TIME_TOGGLE_PAUSE, &keys) {
        if speed.is_paused() {
            speed.unpause();
        } else {
            speed.pause();
        }
        changed = true;
    }
    if keybindings.is_just_pressed(actions::TIME_SPEED_UP, &keys) {
        let new_speed = (speed.hexadies_per_second * 2.0).max(1.0).min(16.0);
        speed.hexadies_per_second = new_speed;
        speed.previous_speed = new_speed;
        changed = true;
    }
    if keybindings.is_just_pressed(actions::TIME_SPEED_DOWN, &keys) {
        let new_speed = speed.hexadies_per_second / 2.0;
        if new_speed >= 0.5 {
            speed.hexadies_per_second = new_speed;
            speed.previous_speed = new_speed;
        }
        changed = true;
    }

    if changed {
        let status = if speed.hexadies_per_second <= 0.0 {
            "PAUSED".to_string()
        } else {
            format!("x{:.0} sd/s", speed.hexadies_per_second)
        };
        info!(
            "Year {} Month {} Hexadies {} [{}]",
            clock.year(),
            clock.month(),
            clock.hexadies(),
            status
        );
    }
}
