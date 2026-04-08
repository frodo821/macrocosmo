use bevy::prelude::*;

pub struct GameTimePlugin;

impl Plugin for GameTimePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GameClock::default())
            .insert_resource(GameSpeed::default())
            .add_systems(Update, (advance_game_time, handle_speed_controls));
    }
}

/// 1 sexadie = 6 days
/// 1 month = 5 sexadies = 30 days
/// 1 year = 12 months = 60 sexadies = 360 days
pub const SEXADIES_PER_MONTH: i64 = 5;
pub const MONTHS_PER_YEAR: i64 = 12;
pub const SEXADIES_PER_YEAR: i64 = SEXADIES_PER_MONTH * MONTHS_PER_YEAR; // 60

/// Game clock based on integer sexadies (6-day units)
#[derive(Resource, Default)]
pub struct GameClock {
    /// Total elapsed sexadies
    pub elapsed: i64,
    /// Sub-sexadie accumulator for smooth real-time integration
    accumulator: f64,
}

impl GameClock {
    pub fn year(&self) -> i64 {
        self.elapsed / SEXADIES_PER_YEAR
    }

    /// Month within the current year (1-based)
    pub fn month(&self) -> i64 {
        (self.elapsed % SEXADIES_PER_YEAR) / SEXADIES_PER_MONTH + 1
    }

    /// Sexadie within the current month (1-based)
    pub fn sexadie(&self) -> i64 {
        (self.elapsed % SEXADIES_PER_MONTH) + 1
    }

    /// Convert to fractional years (for physics calculations)
    pub fn as_years_f64(&self) -> f64 {
        self.elapsed as f64 / SEXADIES_PER_YEAR as f64
    }
}

#[derive(Resource)]
pub struct GameSpeed {
    /// Sexadies per real second. 0 = paused.
    pub sexadies_per_second: f64,
}

impl Default for GameSpeed {
    fn default() -> Self {
        Self {
            sexadies_per_second: 0.0, // Start paused
        }
    }
}

fn advance_game_time(
    real_time: Res<Time>,
    mut clock: ResMut<GameClock>,
    speed: Res<GameSpeed>,
) {
    if speed.sexadies_per_second <= 0.0 {
        return;
    }
    clock.accumulator += real_time.delta_secs_f64() * speed.sexadies_per_second;
    let steps = clock.accumulator as i64;
    if steps > 0 {
        clock.accumulator -= steps as f64;
        clock.elapsed += steps;
    }
}

fn handle_speed_controls(
    clock: Res<GameClock>,
    keys: Res<ButtonInput<KeyCode>>,
    mut speed: ResMut<GameSpeed>,
) {
    let mut changed = false;

    if keys.just_pressed(KeyCode::Space) {
        if speed.sexadies_per_second > 0.0 {
            speed.sexadies_per_second = 0.0;
        } else {
            speed.sexadies_per_second = 1.0;
        }
        changed = true;
    }
    if keys.just_pressed(KeyCode::Equal) {
        speed.sexadies_per_second = (speed.sexadies_per_second * 2.0).max(1.0);
        changed = true;
    }
    if keys.just_pressed(KeyCode::Minus) {
        speed.sexadies_per_second = (speed.sexadies_per_second / 2.0).max(0.0);
        changed = true;
    }

    if changed {
        let status = if speed.sexadies_per_second <= 0.0 {
            "PAUSED".to_string()
        } else {
            format!("x{:.0} sd/s", speed.sexadies_per_second)
        };
        info!(
            "Year {} Month {} Sexadie {} [{}]",
            clock.year(),
            clock.month(),
            clock.sexadie(),
            status
        );
    }
}
