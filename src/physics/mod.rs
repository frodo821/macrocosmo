use crate::time_system::SEXADIES_PER_YEAR;

/// Light speed: 1 light-year per year = 1/60 light-year per sexadie
pub const LIGHT_SPEED_LY_PER_SEXADIE: f64 = 1.0 / SEXADIES_PER_YEAR as f64;

/// Distance between two points in 3D space (in light-years)
pub fn distance_ly(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Light-speed communication delay in sexadies
pub fn light_delay_sexadies(distance_ly: f64) -> i64 {
    (distance_ly / LIGHT_SPEED_LY_PER_SEXADIE).ceil() as i64
}

/// Travel time at sub-light speed in sexadies
pub fn sublight_travel_sexadies(distance_ly: f64, speed_fraction: f64) -> i64 {
    (distance_ly / (LIGHT_SPEED_LY_PER_SEXADIE * speed_fraction)).ceil() as i64
}

/// Convenience: light delay in years (for display)
pub fn light_delay_years(distance_ly: f64) -> f64 {
    distance_ly
}
