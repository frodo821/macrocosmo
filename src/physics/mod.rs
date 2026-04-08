/// Light speed in light-years per game-year (by definition, 1.0)
pub const LIGHT_SPEED: f64 = 1.0;

/// Distance between two points in 3D space (in light-years)
pub fn distance_ly(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Calculate light-speed communication delay in game-years
pub fn light_delay_years(distance_ly: f64) -> f64 {
    distance_ly / LIGHT_SPEED
}

/// Calculate travel time at sub-light speed
pub fn sublight_travel_years(distance_ly: f64, speed_fraction: f64) -> f64 {
    distance_ly / (LIGHT_SPEED * speed_fraction)
}
