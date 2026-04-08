use bevy::prelude::*;

pub struct ShipPlugin;

impl Plugin for ShipPlugin {
    fn build(&self, _app: &mut App) {
        // No systems yet — data model only
    }
}

/// Ship type determines capabilities
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShipType {
    /// Can survey unsurveyed star systems
    Explorer,
    /// Consumed on arrival to establish a colony
    ColonyShip,
    /// High sub-light speed, carries messages
    Courier,
}

impl ShipType {
    /// Default sub-light speed as a fraction of light speed
    pub fn default_sublight_speed(&self) -> f64 {
        match self {
            ShipType::Explorer => 0.75,
            ShipType::ColonyShip => 0.5,
            ShipType::Courier => 0.85,
        }
    }

    /// Default FTL range in light-years (0.0 means no FTL capability)
    pub fn default_ftl_range(&self) -> f64 {
        match self {
            ShipType::Explorer => 0.0,
            ShipType::ColonyShip => 30.0,
            ShipType::Courier => 0.0,
        }
    }

    /// Default hit points
    pub fn default_hp(&self) -> f32 {
        match self {
            ShipType::Explorer => 50.0,
            ShipType::ColonyShip => 100.0,
            ShipType::Courier => 20.0,
        }
    }
}

/// Ship ownership
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Player,
    // Future: AI nations
}

/// Core ship component
#[derive(Component)]
pub struct Ship {
    pub name: String,
    pub ship_type: ShipType,
    pub owner: Owner,
    /// Sub-light speed as fraction of light speed (e.g., 0.75)
    pub sublight_speed: f64,
    /// Maximum FTL range in light-years
    pub ftl_range: f64,
    /// Hit points
    pub hp: f32,
    pub max_hp: f32,
    /// Whether the player is aboard this ship
    pub player_aboard: bool,
}

/// Ship's current movement/location state
#[derive(Component)]
pub enum ShipState {
    /// Docked at a star system
    Docked { system: Entity },
    /// Traveling at sub-light speed
    SubLight {
        origin: [f64; 3],
        destination: [f64; 3],
        target_system: Option<Entity>,
        /// Departure time in sexadies
        departed_at: i64,
        /// Arrival time in sexadies
        arrival_at: i64,
    },
    /// In FTL ballistic flight (no communication possible)
    InFTL {
        origin_system: Entity,
        destination_system: Entity,
        /// Departure time in sexadies
        departed_at: i64,
        /// Arrival time in sexadies
        arrival_at: i64,
    },
    /// Performing survey of a nearby system
    Surveying {
        target_system: Entity,
        /// Survey start time in sexadies
        started_at: i64,
        /// Survey completion time in sexadies
        completes_at: i64,
    },
}

/// Spawn a new ship docked at the given star system with default stats for its type.
pub fn spawn_ship(
    commands: &mut Commands,
    ship_type: ShipType,
    name: String,
    system: Entity,
) -> Entity {
    let hp = ship_type.default_hp();
    commands
        .spawn((
            Ship {
                name,
                ship_type,
                owner: Owner::Player,
                sublight_speed: ship_type.default_sublight_speed(),
                ftl_range: ship_type.default_ftl_range(),
                hp,
                max_hp: hp,
                player_aboard: false,
            },
            ShipState::Docked { system },
        ))
        .id()
}
