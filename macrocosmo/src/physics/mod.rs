use crate::components::Position;
use crate::time_system::HEXADIES_PER_YEAR;

/// Light speed: 1 light-year per year = 1/60 light-year per hexadies
pub const LIGHT_SPEED_LY_PER_HEXADIES: f64 = 1.0 / HEXADIES_PER_YEAR as f64;

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

/// Light-speed communication delay in hexadies
pub fn light_delay_hexadies(distance: f64) -> i64 {
    (distance / LIGHT_SPEED_LY_PER_HEXADIES).ceil() as i64
}

/// Travel time at sub-light speed in hexadies
pub fn sublight_travel_hexadies(distance: f64, speed_fraction: f64) -> i64 {
    (distance / (LIGHT_SPEED_LY_PER_HEXADIES * speed_fraction)).ceil() as i64
}

/// Light delay in years (for display convenience)
pub fn light_delay_years(distance: f64) -> f64 {
    distance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_delay_1_ly() {
        assert_eq!(light_delay_hexadies(1.0), 60);
    }

    #[test]
    fn light_delay_10_ly() {
        assert_eq!(light_delay_hexadies(10.0), 600);
    }

    #[test]
    fn sublight_half_c_1_ly() {
        // 1 LY at 0.5c → 120 sd
        assert_eq!(sublight_travel_hexadies(1.0, 0.5), 120);
    }

    #[test]
    fn sublight_three_quarter_c_1_ly() {
        // 1 LY at 0.75c → 80 sd
        assert_eq!(sublight_travel_hexadies(1.0, 0.75), 80);
    }

    #[test]
    fn distance_ly_known_positions() {
        let a = Position {
            x: 3.0,
            y: 4.0,
            z: 0.0,
        };
        let b = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        assert!((distance_ly(&a, &b) - 5.0).abs() < 1e-10);
    }

    #[test]
    fn distance_ly_arr_known_positions() {
        let a = [3.0, 4.0, 0.0];
        let b = [0.0, 0.0, 0.0];
        assert!((distance_ly_arr(a, b) - 5.0).abs() < 1e-10);
    }
}
