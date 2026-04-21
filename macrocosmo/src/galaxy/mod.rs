pub mod biome;
mod generation;
pub mod region;
mod types;

use bevy::prelude::*;

use crate::modifier::ScopedModifiers;
use crate::scripting::galaxy_api::{PlanetTypeRegistry, StarTypeRegistry};
use crate::scripting::map_api::{MapTypeRegistry, PredefinedSystemRegistry};
use crate::ship::Owner;

// Re-exports for backward compatibility
pub use biome::{
    Biome, BiomeDefinition, BiomeRegistry, DEFAULT_BIOME_ID, resolve_biome_id,
    resolve_default_biome_id,
};
pub use generation::{generate_galaxy, place_forbidden_regions, poisson_sample};
pub use region::{
    ForbiddenRegion, RegionBlockSnapshot, RegionSpecQueue, RegionTypeRegistry, effective_radius,
};
pub use types::{load_biome_registry, load_galaxy_types};

pub struct GalaxyPlugin;

impl Plugin for GalaxyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StarTypeRegistry>()
            .init_resource::<PlanetTypeRegistry>()
            .init_resource::<BiomeRegistry>()
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
                load_biome_registry.after(crate::scripting::load_all_scripts),
            )
            .add_systems(
                Startup,
                generate_galaxy
                    .after(load_galaxy_types)
                    .after(load_biome_registry)
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

/// #296 (S-3): Offset from a star system's center at which Infrastructure Core
/// ships are placed on deploy (in light-years). Chosen to sit strictly inside
/// [`SYSTEM_RADIUS_LY`] so the Core is inside the system's gravity well for
/// visualization purposes, yet clear of the origin to keep separate from other
/// entities that might also snap to the system coordinate.
pub const INNER_ORBIT_OFFSET_LY: f64 = 0.05;

/// #296 (S-3): Nominal radius of a star system in light-years. Used by
/// position helpers (Core deploy) and by higher-level "is position inside
/// system?" checks. NOT currently enforced as a hard physics boundary.
pub const SYSTEM_RADIUS_LY: f64 = 0.1;

/// #296 (S-3): Deterministic inner-orbit position for entities that must spawn
/// near a star system's center without stomping on the system's own
/// coordinate.
///
/// Currently returns `system_center + (INNER_ORBIT_OFFSET_LY, 0, 0)` — a pure
/// +X offset — so repeated calls always produce the same coordinate for the
/// same system. If an entity later needs angular distribution around the star
/// (multiple cores on different axes), this can be extended with a
/// per-entity angle parameter without breaking callers that rely on the
/// deterministic default.
///
/// Returns `[0.0, 0.0, 0.0]` when `system` has no `Position` component (e.g.
/// the entity is not a star system); callers should validate beforehand.
pub fn system_inner_orbit_position(system: Entity, world: &bevy::ecs::world::World) -> [f64; 3] {
    use crate::components::Position;
    let Some(pos) = world.get::<Position>(system) else {
        return [0.0, 0.0, 0.0];
    };
    [pos.x + INNER_ORBIT_OFFSET_LY, pos.y, pos.z]
}
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

#[cfg(test)]
mod inner_orbit_tests {
    use super::*;
    use crate::components::Position;
    use bevy::ecs::world::World;

    #[test]
    fn inner_orbit_position_is_deterministic_and_offset() {
        let mut world = World::new();
        let sys = world
            .spawn(Position {
                x: 10.0,
                y: -3.0,
                z: 7.5,
            })
            .id();
        let pos = system_inner_orbit_position(sys, &world);
        // Deterministic: repeated calls return the same coord.
        let pos2 = system_inner_orbit_position(sys, &world);
        assert_eq!(pos, pos2);
        // Exact offset vs the system's own Position.
        let eps = 1e-12;
        assert!((pos[0] - (10.0 + INNER_ORBIT_OFFSET_LY)).abs() < eps);
        assert!((pos[1] - (-3.0)).abs() < eps);
        assert!((pos[2] - 7.5).abs() < eps);
    }

    #[test]
    fn inner_orbit_offset_is_inside_system_radius() {
        // Offset must be strictly inside the nominal system radius; otherwise
        // the Core would visually sit outside its own system.
        assert!(INNER_ORBIT_OFFSET_LY < SYSTEM_RADIUS_LY);
        assert!(INNER_ORBIT_OFFSET_LY > 0.0);
    }

    #[test]
    fn inner_orbit_position_returns_zero_for_missing_system() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        assert_eq!(system_inner_orbit_position(sys, &world), [0.0, 0.0, 0.0]);
    }
}
