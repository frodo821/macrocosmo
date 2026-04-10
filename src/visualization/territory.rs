use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dPlugin};

use crate::colony::Colony;
use crate::components::Position;
use crate::galaxy::Planet;

use super::GalaxyView;

pub const MAX_COLONIES: usize = 64;
pub const MAX_EMPIRES: usize = 4;

/// Void authority constant — baseline score that "void" has everywhere.
/// Colonies must exceed this to claim territory.
pub const VOID_CONSTANT: f32 = 0.1;

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
    view: Res<GalaxyView>,
    mut materials: ResMut<Assets<TerritoryMaterial>>,
    material_handles: Query<&MeshMaterial2d<TerritoryMaterial>>,
) {
    for handle in material_handles.iter() {
        if let Some(mat) = materials.get_mut(&handle.0) {
            let mut colony_count = 0usize;
            // Reset colony data
            mat.colony_data = ColonyDataGpu::default();

            for colony in colony_q.iter() {
                if colony_count >= MAX_COLONIES {
                    break;
                }
                // Colony -> Planet -> StarSystem -> Position
                if let Ok(planet) = planets.get(colony.planet) {
                    if let Ok(pos) = positions.get(planet.system) {
                        mat.colony_data.data[colony_count] = Vec4::new(
                            pos.x as f32 * view.scale,
                            pos.y as f32 * view.scale,
                            1.0,  // authority strength (could be based on population)
                            0.0,  // empire_id (player = 0 for now)
                        );
                        colony_count += 1;
                    }
                }
            }

            mat.params.values = Vec4::new(
                VOID_CONSTANT,
                colony_count as f32,
                1.0, // empire_count (just player for now)
                0.0,
            );

            // Player empire color: blue
            mat.empire_colors.colors[0] = Vec4::new(0.2, 0.5, 1.0, 1.0);
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
