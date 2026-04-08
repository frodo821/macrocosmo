use crate::components::Position;
use crate::time_system::SEXADIES_PER_YEAR;

/// Light speed: 1 light-year per year = 1/60 light-year per sexadie
pub const LIGHT_SPEED_LY_PER_SEXADIE: f64 = 1.0 / SEXADIES_PER_YEAR as f64;

/// Distance between two Positions (in light-years)
pub fn distance_ly(a: &Position, b: &Position) -> f64 {
    a.distance_to(b)
}

/// Distance between two points as arrays (convenience)
pub fn distance_ly_arr(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Light-speed communication delay in sexadies
pub fn light_delay_sexadies(distance: f64) -> i64 {
    (distance / LIGHT_SPEED_LY_PER_SEXADIE).ceil() as i64
}

/// Travel time at sub-light speed in sexadies
pub fn sublight_travel_sexadies(distance: f64, speed_fraction: f64) -> i64 {
    (distance / (LIGHT_SPEED_LY_PER_SEXADIE * speed_fraction)).ceil() as i64
}

/// Light delay in years (for display convenience)
pub fn light_delay_years(distance: f64) -> f64 {
    distance
}
