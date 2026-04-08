use bevy::prelude::*;

pub struct GameTimePlugin;

impl Plugin for GameTimePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GameClock::default())
            .insert_resource(GameSpeed::default())
            .add_systems(Update, (advance_game_time, handle_speed_controls));
    }
}

/// Minimum time unit: ~6 days = 6/365.25 game-years
const MIN_TIME_UNIT_YEARS: f64 = 6.0 / 365.25;

/// Game clock tracking elapsed game-years
#[derive(Resource)]
pub struct GameClock {
    /// Total elapsed game-years
    pub elapsed_years: f64,
}

impl Default for GameClock {
    fn default() -> Self {
        Self {
            elapsed_years: 0.0,
        }
    }
}

impl GameClock {
    /// Current game year (integer part)
    pub fn year(&self) -> i64 {
        self.elapsed_years as i64
    }

    /// Day within the current year (1-based, in 6-day increments)
    pub fn day(&self) -> u32 {
        let frac = self.elapsed_years - self.elapsed_years.floor();
        let day = (frac * 365.25) as u32;
        // Snap to 6-day increments
        (day / 6) * 6 + 1
    }
}

#[derive(Resource)]
pub struct GameSpeed {
    /// Game-years per real second. 0 = paused.
    pub years_per_second: f64,
}

impl Default for GameSpeed {
    fn default() -> Self {
        Self {
            years_per_second: 0.0, // Start paused
        }
    }
}

fn advance_game_time(
    real_time: Res<Time>,
    mut clock: ResMut<GameClock>,
    speed: Res<GameSpeed>,
) {
    if speed.years_per_second <= 0.0 {
        return;
    }
    let dt_years = real_time.delta_secs_f64() * speed.years_per_second;
    // Snap to minimum time unit
    let steps = (dt_years / MIN_TIME_UNIT_YEARS).floor();
    clock.elapsed_years += steps * MIN_TIME_UNIT_YEARS;
}

fn handle_speed_controls(
    clock: Res<GameClock>,
    keys: Res<ButtonInput<KeyCode>>,
    mut speed: ResMut<GameSpeed>,
) {
    let mut changed = false;

    if keys.just_pressed(KeyCode::Space) {
        if speed.years_per_second > 0.0 {
            speed.years_per_second = 0.0;
        } else {
            speed.years_per_second = 1.0;
        }
        changed = true;
    }
    if keys.just_pressed(KeyCode::Equal) {
        speed.years_per_second = (speed.years_per_second * 2.0).max(0.5);
        changed = true;
    }
    if keys.just_pressed(KeyCode::Minus) {
        speed.years_per_second = (speed.years_per_second / 2.0).max(0.0);
        changed = true;
    }

    if changed {
        let status = if speed.years_per_second <= 0.0 {
            "PAUSED".to_string()
        } else {
            format!("x{:.1} yr/s", speed.years_per_second)
        };
        info!(
            "Year {} Day {} [{}]",
            clock.year(),
            clock.day(),
            status
        );
    }
}
