mod generation;
pub mod region;
mod types;

use bevy::prelude::*;

use crate::modifier::ScopedModifiers;
use crate::scripting::galaxy_api::{PlanetTypeRegistry, StarTypeRegistry};
use crate::scripting::map_api::{MapTypeRegistry, PredefinedSystemRegistry};
use crate::ship::Owner;

// Re-exports for backward compatibility
pub use generation::{generate_galaxy, place_forbidden_regions, poisson_sample};
pub use region::{
    effective_radius, ForbiddenRegion, RegionBlockSnapshot, RegionSpecQueue, RegionTypeRegistry,
};
pub use types::load_galaxy_types;

pub struct GalaxyPlugin;

impl Plugin for GalaxyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StarTypeRegistry>()
            .init_resource::<PlanetTypeRegistry>()
            .init_resource::<PredefinedSystemRegistry>()
            .init_resource::<MapTypeRegistry>()
            .init_resource::<RegionTypeRegistry>()
            .init_resource::<RegionSpecQueue>()
            .add_systems(
                Startup,
                load_galaxy_types.after(crate::scripting::load_all_scripts),
            )
            .add_systems(
                Startup,
                generate_galaxy
                    .after(load_galaxy_types)
                    .after(crate::scripting::load_predefined_system_registry)
                    .after(crate::scripting::load_map_type_registry)
                    .after(crate::faction::spawn_hostile_factions),
            )
            .add_systems(
                Startup,
                place_forbidden_regions
                    .after(generate_galaxy)
                    .after(crate::scripting::load_region_type_registry)
                    .after(crate::scripting::load_region_spec_queue),
            );
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
#[derive(Component, Clone, Debug)]
pub struct SystemAttributes {
    /// Habitability score: 0.0 (uninhabitable) to 1.0 (ideal).
    pub habitability: f64,
    /// Mineral richness: 0.0 to 1.0.
    pub mineral_richness: f64,
    /// Energy potential: 0.0 to 1.0.
    pub energy_potential: f64,
    /// Research potential: 0.0 to 1.0.
    pub research_potential: f64,
    pub max_building_slots: u8,
}

/// Maximum population that a colony can support at hab_score 1.0.
pub const BASE_CARRYING_CAPACITY: f64 = 200.0;
/// Food consumed per population per hexadies (as Amt: 0.100).
pub const FOOD_PER_POP_PER_HEXADIES: crate::amount::Amt = crate::amount::Amt::new(0, 100);

/// Map a numeric habitability value to a human-readable label.
pub fn habitability_label(value: f64) -> &'static str {
    if value >= 0.9 {
        "Ideal"
    } else if value >= 0.6 {
        "Adequate"
    } else if value >= 0.3 {
        "Marginal"
    } else if value > 0.0 {
        "Barren"
    } else {
        "Uninhabitable"
    }
}

/// Map a numeric resource level value to a human-readable label.
pub fn resource_label(value: f64) -> &'static str {
    if value >= 0.7 {
        "Rich"
    } else if value >= 0.4 {
        "Moderate"
    } else if value > 0.0 {
        "Poor"
    } else {
        "None"
    }
}

/// Returns true if the habitability value allows colonization (> 0.0).
pub fn is_habitable(habitability: f64) -> bool {
    habitability > 0.0
}

/// Returns true if colonization is feasible (not barren or uninhabitable).
/// Threshold: habitability >= 0.3 (Marginal or better).
pub fn is_colonizable(habitability: f64) -> bool {
    habitability >= 0.3
}

/// Sovereignty status of a star system
#[derive(Component, Default)]
pub struct Sovereignty {
    pub owner: Option<Owner>,
    pub control_score: f64,
}

// ---------------------------------------------------------------------------
// #293: Hostile entity components
// ---------------------------------------------------------------------------
// Hostile entities carry `(AtSystem, FactionOwner, HostileHitpoints,
// HostileStats, Hostile)`. Readers use `Query<..., With<Hostile>>` to stay
// disjoint from ship queries. Combat strength / evasion live on
// `HostileStats` (populated from `FactionTypeDefinition.strength/evasion`
// scaled by an environmental multiplier at galaxy generation time).

/// Component declaring which star system this entity occupies. Attached to
/// hostile entities (space_creature / ancient_defense) so the visibility /
/// combat / knowledge layers can key their per-system maps.
#[derive(Component, Clone, Copy, Debug)]
pub struct AtSystem(pub Entity);

/// Hitpoints for a hostile entity. Separate from [`crate::ship::ShipHitpoints`]
/// which applies to player ships.
#[derive(Component, Clone, Copy, Debug)]
pub struct HostileHitpoints {
    pub hp: f64,
    pub max_hp: f64,
}

/// Per-entity combat stats for a hostile. `strength` is derived from
/// `FactionTypeDefinition.strength` scaled by an environmental modifier
/// at galaxy generation time (distance-from-center). `evasion` comes
/// straight from the faction type definition.
#[derive(Component, Clone, Copy, Debug)]
pub struct HostileStats {
    pub strength: f64,
    pub evasion: f64,
}

/// Zero-sized marker distinguishing hostile entities from other
/// `FactionOwner`-bearing entities (ships, structures). Hostile-side queries
/// use `With<Hostile>` to stay disjoint from ship-side queries and avoid
/// Bevy B0001 conflicts.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct Hostile;

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

/// Raw star-type modifier definitions attached to a system at generation.
/// Retained for inspection and for targets that are not yet wired into
/// typed scopes (e.g. "system.research_bonus"). Targets with known typed
/// scopes are additionally applied to `SystemModifiers` etc.
#[derive(Component, Default, Clone, Debug)]
pub struct StarTypeModifierSet {
    pub entries: Vec<crate::scripting::galaxy_api::StarTypeModifier>,
}
