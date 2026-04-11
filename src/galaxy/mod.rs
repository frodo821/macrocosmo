mod generation;
mod types;

use bevy::prelude::*;

use crate::modifier::ScopedModifiers;
use crate::scripting::galaxy_api::{PlanetTypeRegistry, StarTypeRegistry};
use crate::ship::Owner;

// Re-exports for backward compatibility
pub use generation::{generate_galaxy, poisson_sample};
pub use types::load_galaxy_types;

pub struct GalaxyPlugin;

impl Plugin for GalaxyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StarTypeRegistry>()
            .init_resource::<PlanetTypeRegistry>()
            .add_systems(
                Startup,
                load_galaxy_types.after(crate::scripting::load_all_scripts),
            )
            .add_systems(Startup, generate_galaxy.after(load_galaxy_types));
    }
}

/// Galaxy configuration resource, inserted by generate_galaxy so other systems
/// (e.g. visualization) can reference galaxy parameters.
#[derive(Resource)]
pub struct GalaxyConfig {
    pub radius: f64,
    pub num_systems: usize,
}

/// A star system in the galaxy
#[derive(Component)]
pub struct StarSystem {
    pub name: String,
    /// Whether this system has been surveyed (precise data available)
    pub surveyed: bool,
    /// Whether this system is the capital
    pub is_capital: bool,
    /// Star type id from Lua definitions (e.g. "yellow_dwarf")
    pub star_type: String,
}

/// A planet orbiting a star system.
#[derive(Component)]
pub struct Planet {
    pub name: String,
    /// The parent star system entity.
    pub system: Entity,
    /// Planet type id from Lua definitions (e.g. "terrestrial")
    pub planet_type: String,
}

/// Convert a 1-based index to a Roman numeral string (up to 12).
pub fn roman_numeral(n: usize) -> &'static str {
    match n {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        6 => "VI",
        7 => "VII",
        8 => "VIII",
        9 => "IX",
        10 => "X",
        11 => "XI",
        12 => "XII",
        _ => "?",
    }
}

/// Physical and economic attributes of a star system.
#[derive(Component, Clone)]
pub struct SystemAttributes {
    pub habitability: Habitability,
    pub mineral_richness: ResourceLevel,
    pub energy_potential: ResourceLevel,
    pub research_potential: ResourceLevel,
    pub max_building_slots: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Habitability {
    Ideal,
    Adequate,
    Marginal,
    Barren,
    GasGiant,
}

/// Maximum population that a colony can support at hab_score 1.0.
pub const BASE_CARRYING_CAPACITY: f64 = 200.0;
/// Food consumed per population per hexadies (as Amt: 0.100).
pub const FOOD_PER_POP_PER_HEXADIES: crate::amount::Amt = crate::amount::Amt::new(0, 100);

impl Habitability {
    /// Continuous habitability score in 0.0..=1.0.
    /// Used for carrying capacity and growth rate scaling.
    /// Technology bonuses can be added on top of this base value.
    pub fn base_score(&self) -> f64 {
        match self {
            Habitability::Ideal => 1.0,
            Habitability::Adequate => 0.7,
            Habitability::Marginal => 0.4,
            Habitability::Barren => 0.15,
            Habitability::GasGiant => 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceLevel {
    Rich,
    Moderate,
    Poor,
    None,
}

/// Sovereignty status of a star system
#[derive(Component, Default)]
pub struct Sovereignty {
    pub owner: Option<Owner>,
    pub control_score: f64,
}

/// A hostile presence at a star system that player ships must fight.
#[derive(Component)]
pub struct HostilePresence {
    pub system: Entity,
    pub strength: f64,
    pub hp: f64,
    pub max_hp: f64,
    pub hostile_type: HostileType,
    pub evasion: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostileType {
    SpaceCreature,
    AncientDefense,
}

/// Marker for systems obscured by interstellar gas
#[derive(Component)]
pub struct ObscuredByGas;

/// Marker for systems that have port facilities
#[derive(Component)]
pub struct PortFacility {
    /// The other star system entity this port connects to
    pub partner: Entity,
}

/// Persistent anomalies/points of interest discovered during surveys.
#[derive(Component, Default, Clone, Debug)]
pub struct Anomalies {
    pub discoveries: Vec<Anomaly>,
}

/// A single anomaly discovered during a survey.
#[derive(Clone, Debug)]
pub struct Anomaly {
    pub id: String,
    pub name: String,
    pub description: String,
    pub discovered_at: i64,
}

/// Modifiers that apply to all ships in a star system.
/// Example: solar storm reducing speed, nebula boosting shields.
#[derive(Component, Default)]
pub struct SystemModifiers {
    pub ship_speed: ScopedModifiers,
    pub ship_attack: ScopedModifiers,
    pub ship_defense: ScopedModifiers,
}
