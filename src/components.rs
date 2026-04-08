use bevy::prelude::*;

/// Position in 3D space, measured in light-years.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Position {
    /// Euclidean distance to another position, in light-years.
    pub fn distance_to(&self, other: &Position) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// Convert to a plain array `[x, y, z]`.
    pub fn as_array(&self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
}

impl From<[f64; 3]> for Position {
    fn from(arr: [f64; 3]) -> Self {
        Self {
            x: arr[0],
            y: arr[1],
            z: arr[2],
        }
    }
}

/// Describes the movement state of a ship or mobile entity.
#[derive(Component, Debug, Clone)]
pub enum MovementState {
    /// Docked at a star system.
    Docked { system: Entity },
    /// Travelling at sub-light speed between two points.
    SubLight {
        origin: Position,
        destination: Position,
        speed_fraction: f64,
        departed_at: i64,
    },
    /// Travelling via FTL jump to a destination system.
    FTL {
        destination: Entity,
        departed_at: i64,
        arrives_at: i64,
    },
}
