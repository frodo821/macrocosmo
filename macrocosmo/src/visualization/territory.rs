use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dPlugin};

use crate::colony::Colony;
use crate::components::Position;
use crate::galaxy::Planet;
use crate::knowledge::KnowledgeStore;
use crate::player::PlayerEmpire;
use crate::ship::{Owner, Ship, ShipHitpoints, ShipState};
use crate::time_system::GameClock;

use super::GalaxyView;

pub const MAX_COLONIES: usize = 64;
pub const MAX_EMPIRES: usize = 4;

/// Void authority constant — baseline score that "void" has everywhere.
/// Colonies must exceed this to claim territory (in `ly²`).
pub const VOID_CONSTANT: f32 = 0.1;

// ---------------------------------------------------------------------------
// Authority score tweakables (ly²-scale values; GPU scaling applied separately)
// ---------------------------------------------------------------------------

/// Base authority score for a minimally-populated colony with no garrison and
/// fresh communications. Sized so that the `1/r²` border lands at
/// `sqrt(BASE / VOID_CONSTANT)` ly (≈ 7.07 ly with default constants).
pub const AUTHORITY_BASE: f32 = 5.0;

/// Divisor applied to `ln(max(population, 1))` when computing the population
/// factor. Smaller = populations scale authority faster. With divisor 10,
/// population 100 → factor 1.46, 1000 → factor 1.69, 10_000 → factor 1.92.
pub const AUTHORITY_POP_LN_DIVISOR: f32 = 10.0;

/// Each `AUTHORITY_GARRISON_HP_UNIT` of docked hull HP contributes this much
/// additive bonus to the garrison factor (which multiplies base authority as
/// `1 + garrison_factor`).
pub const AUTHORITY_GARRISON_PER_UNIT: f32 = 0.1;
pub const AUTHORITY_GARRISON_HP_UNIT: f32 = 100.0;

/// Freshness thresholds (in hexadies) and their corresponding multipliers.
/// `info_age < FRESH_AGE` → full authority; older ages decay in steps.
pub const AUTHORITY_FRESH_AGE: i64 = 60;
pub const AUTHORITY_AGING_AGE: i64 = 300;
pub const AUTHORITY_OLD_AGE: i64 = 600;

pub const AUTHORITY_FRESHNESS_FRESH: f32 = 1.0;
pub const AUTHORITY_FRESHNESS_AGING: f32 = 0.7;
pub const AUTHORITY_FRESHNESS_OLD: f32 = 0.4;
pub const AUTHORITY_FRESHNESS_VERY_OLD: f32 = 0.15;

/// Fallback palette for empire territory colors (indexed by empire_id). Only
/// used until faction-owned coloring is wired through from Lua definitions.
pub const EMPIRE_COLOR_PALETTE: [[f32; 4]; MAX_EMPIRES] = [
    [0.30, 0.55, 1.00, 1.0], // blue — player
    [1.00, 0.45, 0.25, 1.0], // orange
    [0.35, 0.90, 0.45, 1.0], // green
    [0.95, 0.35, 0.80, 1.0], // magenta
];

/// Compute the authority freshness multiplier for a colony's home system
/// based on how stale the empire's knowledge of it is.
///
/// - `None` → the empire has no snapshot at all. We treat this as the home
///   system being locally observed (freshness = 1.0) rather than stale; the
///   capital colony typically has no entry in `KnowledgeStore` because its
///   data is always "live".
pub fn authority_freshness(info_age: Option<i64>) -> f32 {
    match info_age {
        None => AUTHORITY_FRESHNESS_FRESH,
        Some(age) if age < AUTHORITY_FRESH_AGE => AUTHORITY_FRESHNESS_FRESH,
        Some(age) if age < AUTHORITY_AGING_AGE => AUTHORITY_FRESHNESS_AGING,
        Some(age) if age < AUTHORITY_OLD_AGE => AUTHORITY_FRESHNESS_OLD,
        Some(_) => AUTHORITY_FRESHNESS_VERY_OLD,
    }
}

/// Pure helper that computes a colony's effective authority score.
///
/// Kept as a free function of simple numeric inputs so it can be unit-tested
/// without constructing a Bevy world. Callers supply:
/// - `population` — colony population (>= 0)
/// - `garrison_hull_hp` — total `hull_max` of friendly ships docked at the
///   colony's system (>= 0)
/// - `freshness` — multiplier from [`authority_freshness`] (0..=1)
///
/// Returns authority in `ly²` units. The caller must multiply by
/// `view.scale²` before pushing to the GPU so that the shader's
/// `strength / dist_sq` comparison operates in the same (world-unit) space.
pub fn colony_effective_authority(
    population: f32,
    garrison_hull_hp: f32,
    freshness: f32,
) -> f32 {
    let pop = population.max(1.0);
    let pop_factor = 1.0 + pop.ln().max(0.0) / AUTHORITY_POP_LN_DIVISOR;
    let garrison_factor = (garrison_hull_hp.max(0.0) / AUTHORITY_GARRISON_HP_UNIT)
        * AUTHORITY_GARRISON_PER_UNIT;
    AUTHORITY_BASE * pop_factor * (1.0 + garrison_factor) * freshness.max(0.0)
}

/// Computes 1/r^2 authority contribution from a single colony at a given point.
/// Returns `colony_authority / distance_squared`, clamped to avoid division by zero.
pub fn authority_contribution(colony_pos: Vec2, point: Vec2, colony_authority: f32) -> f32 {
    let diff = point - colony_pos;
    let dist_sq = diff.dot(diff).max(0.01);
    colony_authority / dist_sq
}

/// Computes total authority for each empire at a given point.
/// Returns an array of authority values indexed by empire ID.
pub fn compute_authority(
    colonies: &[(Vec2, f32, usize)], // (position, authority, empire_id)
    point: Vec2,
    num_empires: usize,
) -> [f32; MAX_EMPIRES] {
    let mut auth = [0.0f32; MAX_EMPIRES];
    for &(pos, strength, empire_id) in colonies {
        if empire_id < num_empires && empire_id < MAX_EMPIRES {
            auth[empire_id] += authority_contribution(pos, point, strength);
        }
    }
    auth
}

/// Determines the owner of a point. Returns `None` if void wins.
pub fn territory_owner(
    colonies: &[(Vec2, f32, usize)],
    point: Vec2,
    num_empires: usize,
    void_constant: f32,
) -> Option<usize> {
    let auth = compute_authority(colonies, point, num_empires);
    let mut max_auth = void_constant;
    let mut owner = None;
    for e in 0..num_empires.min(MAX_EMPIRES) {
        if auth[e] > max_auth {
            max_auth = auth[e];
            owner = Some(e);
        }
    }
    owner
}

/// GPU-side colony data: xy = world position, z = authority strength, w = empire_id.
#[derive(Clone, Copy, ShaderType)]
pub struct ColonyDataGpu {
    pub data: [Vec4; MAX_COLONIES],
}

impl Default for ColonyDataGpu {
    fn default() -> Self {
        Self {
            data: [Vec4::ZERO; MAX_COLONIES],
        }
    }
}

/// GPU-side empire colors.
#[derive(Clone, Copy, Default, ShaderType)]
pub struct EmpireColorsGpu {
    pub colors: [Vec4; MAX_EMPIRES],
}

/// GPU-side parameters: x = void_constant, y = colony_count, z = empire_count, w = unused.
#[derive(Clone, Copy, Default, ShaderType)]
pub struct TerritoryParamsGpu {
    pub values: Vec4,
}

#[derive(AsBindGroup, Asset, TypePath, Clone)]
pub struct TerritoryMaterial {
    #[uniform(0)]
    pub colony_data: ColonyDataGpu,
    #[uniform(1)]
    pub empire_colors: EmpireColorsGpu,
    #[uniform(2)]
    pub params: TerritoryParamsGpu,
}

impl Material2d for TerritoryMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/territory.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}

/// Marker component for the territory overlay quad.
#[derive(Component)]
pub struct TerritoryQuad;

pub struct TerritoryPlugin;

impl Plugin for TerritoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(Material2dPlugin::<TerritoryMaterial>::default())
            .add_systems(Startup, spawn_territory_quad)
            .add_systems(Update, sync_territory_material);
    }
}

fn spawn_territory_quad(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TerritoryMaterial>>,
) {
    let quad = meshes.add(Rectangle::new(2000.0, 2000.0));
    let material = materials.add(TerritoryMaterial {
        colony_data: ColonyDataGpu::default(),
        empire_colors: EmpireColorsGpu::default(),
        params: TerritoryParamsGpu {
            values: Vec4::new(VOID_CONSTANT, 0.0, 0.0, 0.0),
        },
    });
    commands.spawn((
        TerritoryQuad,
        Mesh2d(quad),
        MeshMaterial2d(material),
        Transform::from_xyz(0.0, 0.0, -10.0),
    ));
}

fn sync_territory_material(
    colony_q: Query<&Colony>,
    planets: Query<&Planet>,
    positions: Query<&Position>,
    ships: Query<(&Ship, &ShipState, &ShipHitpoints)>,
    empire_q: Query<(Entity, &KnowledgeStore), With<PlayerEmpire>>,
    clock: Res<GameClock>,
    view: Res<GalaxyView>,
    mut materials: ResMut<Assets<TerritoryMaterial>>,
    material_handles: Query<&MeshMaterial2d<TerritoryMaterial>>,
) {
    // For now only the player empire owns colonies. If this changes, expand
    // into a (empire_entity -> empire_id) mapping below.
    let Ok((player_entity, knowledge)) = empire_q.single() else {
        return;
    };

    // Precompute garrison hull HP per star system: sum of hull_max for all
    // friendly ships docked at that system.
    let mut garrison_by_system: std::collections::HashMap<Entity, f32> =
        std::collections::HashMap::new();
    for (ship, state, hp) in ships.iter() {
        let Owner::Empire(owner_entity) = ship.owner else { continue };
        if owner_entity != player_entity {
            continue;
        }
        if let ShipState::Docked { system } = state {
            *garrison_by_system.entry(*system).or_insert(0.0) += hp.hull_max as f32;
        }
    }

    let scale_sq = view.scale * view.scale;

    for handle in material_handles.iter() {
        let Some(mat) = materials.get_mut(&handle.0) else { continue };

        let mut colony_count = 0usize;
        // Reset colony data
        mat.colony_data = ColonyDataGpu::default();

        for colony in colony_q.iter() {
            if colony_count >= MAX_COLONIES {
                break;
            }
            let Ok(planet) = planets.get(colony.planet) else { continue };
            let system = planet.system;
            let Ok(pos) = positions.get(system) else { continue };

            let garrison_hp = garrison_by_system.get(&system).copied().unwrap_or(0.0);
            let freshness = authority_freshness(knowledge.info_age(system, clock.elapsed));
            let authority_ly2 =
                colony_effective_authority(colony.population as f32, garrison_hp, freshness);
            // Convert ly² authority to world-unit² authority so that the
            // shader's `strength / dist_sq` comparison (which runs in
            // world coordinates) has matching units.
            let authority_world = authority_ly2 * scale_sq;

            // empire_id = 0 (player) for now. Widen when non-player
            // empires can own colonies.
            let empire_id = 0.0f32;

            mat.colony_data.data[colony_count] = Vec4::new(
                pos.x as f32 * view.scale,
                pos.y as f32 * view.scale,
                authority_world,
                empire_id,
            );
            colony_count += 1;
        }

        // Multiply the void constant by scale² as well so the threshold
        // comparison stays consistent across world/ly spaces.
        let void_world = VOID_CONSTANT * scale_sq;
        mat.params.values = Vec4::new(
            void_world,
            colony_count as f32,
            1.0, // empire_count (just player for now)
            0.0,
        );

        // Populate the fallback palette. Slots beyond the number of known
        // empires simply stay unused.
        for (i, color) in EMPIRE_COLOR_PALETTE.iter().enumerate() {
            mat.empire_colors.colors[i] = Vec4::new(color[0], color[1], color[2], color[3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authority_at_colony_is_high() {
        let colony_pos = Vec2::new(0.0, 0.0);
        let point = Vec2::new(0.01, 0.0); // Very close to colony
        let auth = authority_contribution(colony_pos, point, 1.0);
        // At distance 0.01, authority = 1.0 / 0.01^2 = 10000, but clamped dist_sq = max(0.0001, 0.01)
        assert!(auth > 10.0, "Authority near colony should be high, got {auth}");
    }

    #[test]
    fn test_authority_falls_off_with_distance() {
        let colony_pos = Vec2::new(0.0, 0.0);
        let near = Vec2::new(1.0, 0.0);
        let far = Vec2::new(10.0, 0.0);
        let auth_near = authority_contribution(colony_pos, near, 1.0);
        let auth_far = authority_contribution(colony_pos, far, 1.0);
        assert!(
            auth_near > auth_far,
            "Near authority ({auth_near}) should be greater than far ({auth_far})"
        );
        // At distance 1, authority = 1.0; at distance 10, authority = 0.01
        let expected_ratio = 100.0;
        let actual_ratio = auth_near / auth_far;
        assert!(
            (actual_ratio - expected_ratio).abs() < 0.1,
            "Authority should follow 1/r^2: expected ratio {expected_ratio}, got {actual_ratio}"
        );
    }

    #[test]
    fn test_void_wins_at_large_distance() {
        let colonies = vec![(Vec2::new(0.0, 0.0), 1.0, 0usize)];
        let far_point = Vec2::new(100.0, 0.0);
        // At distance 100, authority = 1.0 / 10000 = 0.0001, well below VOID_CONSTANT (0.1)
        let owner = territory_owner(&colonies, far_point, 1, VOID_CONSTANT);
        assert_eq!(owner, None, "Void should own points far from all colonies");
    }

    #[test]
    fn test_colony_wins_near() {
        let colonies = vec![(Vec2::new(0.0, 0.0), 1.0, 0usize)];
        let near_point = Vec2::new(1.0, 0.0);
        // At distance 1, authority = 1.0 > VOID_CONSTANT (0.1)
        let owner = territory_owner(&colonies, near_point, 1, VOID_CONSTANT);
        assert_eq!(owner, Some(0), "Empire 0 should own points near its colony");
    }

    #[test]
    fn test_contested_territory() {
        let colonies = vec![
            (Vec2::new(-5.0, 0.0), 1.0, 0usize),
            (Vec2::new(5.0, 0.0), 1.0, 1usize),
        ];
        // Point equidistant from both — both have equal authority
        let midpoint = Vec2::new(0.0, 0.0);
        let auth = compute_authority(&colonies, midpoint, 2);
        let diff = (auth[0] - auth[1]).abs();
        assert!(
            diff < 0.001,
            "Authority should be equal at midpoint, got {} vs {}",
            auth[0],
            auth[1]
        );
    }

    #[test]
    fn test_colony_effective_authority_baseline() {
        // Minimal population, no garrison, fresh comms: factor ≈ base.
        let a = colony_effective_authority(1.0, 0.0, AUTHORITY_FRESHNESS_FRESH);
        assert!((a - AUTHORITY_BASE).abs() < 1e-4, "baseline should equal AUTHORITY_BASE, got {a}");
    }

    #[test]
    fn test_colony_effective_authority_increases_with_population() {
        let low = colony_effective_authority(100.0, 0.0, AUTHORITY_FRESHNESS_FRESH);
        let high = colony_effective_authority(1000.0, 0.0, AUTHORITY_FRESHNESS_FRESH);
        assert!(
            high > low,
            "Higher population should yield higher authority (got {low} vs {high})"
        );
        // ln(1000)/10 ≈ 0.691, ln(100)/10 ≈ 0.461 → ratio ≈ 1.69/1.46 ≈ 1.158
        let ratio = high / low;
        assert!(
            (ratio - 1.158).abs() < 0.05,
            "Population factor ratio off: expected ~1.158, got {ratio}"
        );
    }

    #[test]
    fn test_colony_effective_authority_decreases_with_comm_loss() {
        let fresh = colony_effective_authority(100.0, 0.0, AUTHORITY_FRESHNESS_FRESH);
        let aging = colony_effective_authority(100.0, 0.0, AUTHORITY_FRESHNESS_AGING);
        let old = colony_effective_authority(100.0, 0.0, AUTHORITY_FRESHNESS_OLD);
        let very_old = colony_effective_authority(100.0, 0.0, AUTHORITY_FRESHNESS_VERY_OLD);
        assert!(fresh > aging, "FRESH should exceed AGING ({fresh} vs {aging})");
        assert!(aging > old, "AGING should exceed OLD ({aging} vs {old})");
        assert!(old > very_old, "OLD should exceed VERY_OLD ({old} vs {very_old})");
        // VERY_OLD should be roughly 15% of FRESH (freshness multiplier).
        let ratio = very_old / fresh;
        assert!(
            (ratio - AUTHORITY_FRESHNESS_VERY_OLD).abs() < 1e-3,
            "VERY_OLD/FRESH ratio should match freshness mult, got {ratio}"
        );
    }

    #[test]
    fn test_colony_effective_authority_scales_with_garrison() {
        let no_ships = colony_effective_authority(100.0, 0.0, AUTHORITY_FRESHNESS_FRESH);
        let with_ships = colony_effective_authority(100.0, 500.0, AUTHORITY_FRESHNESS_FRESH);
        assert!(
            with_ships > no_ships,
            "Garrisoned colony should have higher authority ({no_ships} vs {with_ships})"
        );
        // 500 hull HP → garrison factor = (500/100)*0.1 = 0.5 → *1.5 multiplier
        let ratio = with_ships / no_ships;
        assert!(
            (ratio - 1.5).abs() < 1e-3,
            "Garrison multiplier should be 1.5 for 500 hull HP, got {ratio}"
        );
    }

    #[test]
    fn test_authority_freshness_thresholds() {
        assert_eq!(authority_freshness(None), AUTHORITY_FRESHNESS_FRESH);
        assert_eq!(authority_freshness(Some(0)), AUTHORITY_FRESHNESS_FRESH);
        assert_eq!(
            authority_freshness(Some(AUTHORITY_FRESH_AGE - 1)),
            AUTHORITY_FRESHNESS_FRESH
        );
        assert_eq!(
            authority_freshness(Some(AUTHORITY_FRESH_AGE)),
            AUTHORITY_FRESHNESS_AGING
        );
        assert_eq!(
            authority_freshness(Some(AUTHORITY_AGING_AGE)),
            AUTHORITY_FRESHNESS_OLD
        );
        assert_eq!(
            authority_freshness(Some(AUTHORITY_OLD_AGE)),
            AUTHORITY_FRESHNESS_VERY_OLD
        );
        assert_eq!(
            authority_freshness(Some(100_000)),
            AUTHORITY_FRESHNESS_VERY_OLD
        );
    }

    #[test]
    fn test_authority_border_distance_matches_target() {
        // Sanity check: with default constants, a colony of population 100
        // with no garrison and fresh comms should claim a border of roughly
        // 7–12 ly. The border satisfies `authority_ly2 / dist_ly^2 =
        // VOID_CONSTANT`, so dist_ly = sqrt(authority_ly2 / VOID_CONSTANT).
        let authority = colony_effective_authority(100.0, 0.0, AUTHORITY_FRESHNESS_FRESH);
        let border_ly = (authority / VOID_CONSTANT).sqrt();
        assert!(
            (7.0..=12.0).contains(&border_ly),
            "border should land in 7–12 ly for pop=100, got {border_ly} ly"
        );
    }

    #[test]
    fn test_multiple_colonies_stack() {
        let colonies = vec![
            (Vec2::new(0.0, 0.0), 1.0, 0usize),
            (Vec2::new(1.0, 0.0), 1.0, 0usize),
        ];
        let single = vec![(Vec2::new(0.0, 0.0), 1.0, 0usize)];
        let point = Vec2::new(0.5, 0.0);
        let auth_multi = compute_authority(&colonies, point, 1);
        let auth_single = compute_authority(&single, point, 1);
        assert!(
            auth_multi[0] > auth_single[0],
            "Multiple colonies should produce higher authority"
        );
    }
}
