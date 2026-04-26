use bevy::prelude::*;

pub struct GameTimePlugin;

impl Plugin for GameTimePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GameClock::default())
            .insert_resource(GameSpeed::default())
            .add_systems(Update, (advance_game_time, handle_speed_controls));
    }
}

/// 1 hexadies = 6 days
/// 1 month = 5 hexadies = 30 days
/// 1 year = 12 months = 60 hexadies = 360 days
pub const HEXADIES_PER_MONTH: i64 = 5;
pub const MONTHS_PER_YEAR: i64 = 12;
pub const HEXADIES_PER_YEAR: i64 = HEXADIES_PER_MONTH * MONTHS_PER_YEAR; // 60

/// Game clock based on integer hexadies (6-day units)
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct GameClock {
    /// Total elapsed hexadies
    pub elapsed: i64,
    /// Sub-hexadies accumulator for smooth real-time integration
    accumulator: f64,
}

impl GameClock {
    pub fn new(elapsed: i64) -> Self {
        Self {
            elapsed,
            accumulator: 0.0,
        }
    }

    pub fn year(&self) -> i64 {
        self.elapsed / HEXADIES_PER_YEAR
    }

    /// Month within the current year (1-based)
    pub fn month(&self) -> i64 {
        (self.elapsed % HEXADIES_PER_YEAR) / HEXADIES_PER_MONTH + 1
    }

    /// Hexadies within the current month (1-based)
    pub fn hexadies(&self) -> i64 {
        (self.elapsed % HEXADIES_PER_MONTH) + 1
    }

    /// Convert to fractional years (for physics calculations)
    pub fn as_years_f64(&self) -> f64 {
        self.elapsed as f64 / HEXADIES_PER_YEAR as f64
    }
}

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct GameSpeed {
    /// Hexadies per real second. 0 = paused.
    pub hexadies_per_second: f64,
    /// Speed before pausing (restored on unpause)
    pub previous_speed: f64,
}

impl Default for GameSpeed {
    fn default() -> Self {
        Self {
            hexadies_per_second: 0.0, // Start paused
            previous_speed: 1.0,
        }
    }
}

impl GameSpeed {
    /// Pause the game, remembering current speed.
    pub fn pause(&mut self) {
        if self.hexadies_per_second > 0.0 {
            self.previous_speed = self.hexadies_per_second;
            self.hexadies_per_second = 0.0;
        }
    }

    /// Unpause, restoring previous speed.
    pub fn unpause(&mut self) {
        if self.hexadies_per_second <= 0.0 {
            self.hexadies_per_second = self.previous_speed;
        }
    }

    pub fn is_paused(&self) -> bool {
        self.hexadies_per_second <= 0.0
    }
}

pub fn advance_game_time(
    real_time: Res<Time>,
    mut clock: ResMut<GameClock>,
    speed: Res<GameSpeed>,
    pending_routes: Query<(), With<crate::ship::routing::PendingRoute>>,
) {
    if speed.hexadies_per_second <= 0.0 {
        return;
    }
    // #128: Suppress time advancement while route calculations are pending.
    // Existence-based check (no counter) — structurally leak-proof: if every
    // ship holding a `PendingRoute` despawns, the query is empty regardless
    // of what tore them down.
    if !pending_routes.is_empty() {
        return;
    }
    clock.accumulator += real_time.delta_secs_f64() * speed.hexadies_per_second;
    let steps = clock.accumulator as i64;
    if steps > 0 {
        clock.accumulator -= steps as f64;
        clock.elapsed += steps;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_zero() {
        let clock = GameClock::new(0);
        assert_eq!(clock.year(), 0);
        assert_eq!(clock.month(), 1);
        assert_eq!(clock.hexadies(), 1);
    }

    #[test]
    fn elapsed_59() {
        let clock = GameClock::new(59);
        assert_eq!(clock.year(), 0);
        assert_eq!(clock.month(), 12);
        assert_eq!(clock.hexadies(), 5);
    }

    #[test]
    fn elapsed_60_is_year_1() {
        let clock = GameClock::new(60);
        assert_eq!(clock.year(), 1);
        assert_eq!(clock.month(), 1);
        assert_eq!(clock.hexadies(), 1);
    }

    #[test]
    fn as_years_f64_half_year() {
        let clock = GameClock::new(30);
        assert!((clock.as_years_f64() - 0.5).abs() < 1e-10);
    }
}
