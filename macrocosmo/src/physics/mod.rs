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

/// #468: Light-speed command delay from an issuer (Ruler) to a specific
/// ship.
///
/// Centralises the "Ruler → ship" light-delay computation used by the
/// player-issued command path (`ui::context_menu`), the Lua-scripted
/// command path (`scripting::gamestate_scope::compute_request_light_delay`),
/// and the AI ship-command dispatch path (`ai::command_outbox`). Prior to
/// the #468 hoist each of those sites duplicated the
/// `light_delay_hexadies(distance_ly_arr(...))` pair inline; collapsing
/// them through a single helper makes the three paths share one
/// definition of "command travel time".
pub fn light_delay_ruler_to_ship(ruler_pos: [f64; 3], ship_pos: [f64; 3]) -> i64 {
    light_delay_hexadies(distance_ly_arr(ruler_pos, ship_pos))
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

    #[test]
    fn light_delay_ruler_to_ship_zero_when_coincident() {
        let p = [1.0, 2.0, 3.0];
        assert_eq!(light_delay_ruler_to_ship(p, p), 0);
    }

    #[test]
    fn light_delay_ruler_to_ship_matches_light_delay_hexadies() {
        let ruler = [0.0, 0.0, 0.0];
        let ship = [5.0, 0.0, 0.0];
        let expected = light_delay_hexadies(distance_ly_arr(ruler, ship));
        assert_eq!(light_delay_ruler_to_ship(ruler, ship), expected);
    }
}
