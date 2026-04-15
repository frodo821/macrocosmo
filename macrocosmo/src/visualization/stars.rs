use bevy::prelude::*;

use super::GalaxyView;
use crate::colony::{BuildingRegistry, Buildings, Colony, SystemBuildings};
use crate::components::Position;
use crate::deep_space::{ConstructionPlatform, DeepSpaceStructure, Scrapyard, StructureHitpoints};
use crate::galaxy::{AtSystem, GalaxyConfig, Hostile, ObscuredByGas, Planet, StarSystem};
use crate::knowledge::KnowledgeStore;
use crate::player::{Player, PlayerEmpire, StationedAt};
use crate::ship::{Ship, ShipState};
use crate::technology::GlobalParams;
use crate::time_system::GameClock;

use super::{SelectedShip, SelectedSystem};

#[derive(Component)]
pub(super) struct StarVisual {
    pub system_entity: Entity,
}

/// Marks a sprite as a glow halo behind a star.
#[derive(Component)]
pub(super) struct StarGlow;

/// Stores the base pixel size of a star sprite so zoom-responsive scaling can reference it.
#[derive(Component)]
pub(super) struct BaseStarSize(pub f32);

pub fn spawn_star_visuals(
    mut commands: Commands,
    stars: Query<(Entity, &StarSystem, &Position, Option<&ObscuredByGas>)>,
    colonies: Query<&Colony>,
    planets: Query<&Planet>,
    view: Res<GalaxyView>,
) {
    // Build a set of colonized system entities
    let colonized_systems: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter_map(|c| c.system(&planets))
        .collect();

    for (entity, star, pos, obscured) in &stars {
        let x = pos.x as f32 * view.scale;
        let y = pos.y as f32 * view.scale;
        let is_obscured = obscured.is_some();
        let is_colonized = colonized_systems.contains(&entity);

        let color = star_color(star, is_colonized, is_obscured);

        // Determine base size based on star status
        let size = if star.is_capital {
            16.0
        } else if is_colonized {
            14.0
        } else if star.surveyed {
            12.0
        } else {
            10.0
        };

        // Spawn glow halo behind the star (skip for obscured stars)
        if !is_obscured {
            let [r, g, b, _] = color.to_srgba().to_f32_array();
            let glow_alpha = if star.is_capital || is_colonized {
                0.2
            } else {
                0.15
            };
            let glow_size = size * 3.0;
            commands.spawn((
                StarVisual { system_entity: entity },
                StarGlow,
                BaseStarSize(glow_size),
                Sprite {
                    color: Color::srgba(r, g, b, glow_alpha),
                    custom_size: Some(Vec2::splat(glow_size)),
                    ..default()
                },
                Transform::from_xyz(x, y, -0.1),
            ));
        }

        // Spawn main star dot
        commands.spawn((
            StarVisual { system_entity: entity },
            BaseStarSize(size),
            Sprite {
                color,
                custom_size: Some(Vec2::splat(size)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.0),
        ));

        // Labels: show for all surveyed stars, not just capital
        if star.is_capital || star.surveyed {
            let label_alpha = if star.is_capital {
                1.0
            } else if is_colonized {
                0.9
            } else {
                0.7
            };
            commands.spawn((
                StarVisual { system_entity: entity },
                Text2d::new(&star.name),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, label_alpha)),
                Transform::from_xyz(x, y + 14.0, 1.0),
            ));
        }
    }
}

pub(super) fn star_color(star: &StarSystem, colonized: bool, obscured: bool) -> Color {
    if obscured {
        Color::srgba(0.2, 0.2, 0.25, 0.15) // Barely visible
    } else if star.is_capital {
        Color::srgb(1.0, 0.84, 0.0) // Gold
    } else if colonized {
        Color::srgb(0.3, 1.0, 0.3) // Bright green, more saturated
    } else if star.surveyed {
        Color::srgb(0.5, 0.7, 1.0) // Bright blue
    } else {
        Color::srgba(0.5, 0.5, 0.55, 0.4) // Dim, small, unsurveyed
    }
}

// #17: Enhanced update_star_colors with KnowledgeStore-based alpha fading
// #40: Also handles zoom-responsive sizing and glow color updates
// #176: Uses KnowledgeStore for remote system colonized status
pub fn update_star_colors(
    stars: Query<(Entity, &StarSystem, Option<&ObscuredByGas>)>,
    mut visuals: Query<(&StarVisual, &mut Sprite, Option<&StarGlow>, Option<&BaseStarSize>)>,
    empire_q: Query<&KnowledgeStore, With<PlayerEmpire>>,
    colonies: Query<&Colony>,
    planets: Query<&Planet>,
    clock: Res<GameClock>,
    camera_q: Query<&Projection, With<Camera2d>>,
    player_q: Query<&StationedAt, With<Player>>,
) {
    crate::prof_span!("update_star_colors");
    let Ok(knowledge) = empire_q.single() else {
        return;
    };
    let player_system = player_q.iter().next().map(|s| s.system);

    // Build colonized systems set for local system only (real-time)
    let local_colonized: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter_map(|c| c.system(&planets))
        .collect();

    // Get the current camera scale for zoom-responsive sizing
    let camera_scale = camera_q
        .iter()
        .find_map(|proj| {
            if let Projection::Orthographic(ref ortho) = *proj {
                Some(ortho.scale)
            } else {
                None
            }
        })
        .unwrap_or(1.0);

    // Zoom-responsive scale factor: when zoomed out, make stars proportionally larger
    let zoom_factor = (1.0 + (camera_scale - 1.0) * 0.5).max(1.0);

    for (vis, mut sprite, glow, base_size) in &mut visuals {
        if let Ok((_, star, obscured)) = stars.get(vis.system_entity) {
            // #176: Local system uses real-time colonized status, remote uses KnowledgeStore
            let is_colonized = if player_system == Some(vis.system_entity) {
                local_colonized.contains(&vis.system_entity)
            } else {
                knowledge.get(vis.system_entity)
                    .map(|k| k.data.colonized)
                    .unwrap_or(false)
            };
            // #176: For remote systems, use knowledge-based survey status
            let effective_surveyed = if player_system == Some(vis.system_entity) {
                star.surveyed
            } else {
                knowledge.get(vis.system_entity)
                    .map(|k| k.data.surveyed)
                    .unwrap_or(star.surveyed)
            };
            // Create a temporary view of the star with knowledge-based survey status
            let effective_star = StarSystem {
                name: star.name.clone(),
                star_type: star.star_type.clone(),
                surveyed: effective_surveyed,
                is_capital: star.is_capital,
            };
            let base_color = star_color(&effective_star, is_colonized, obscured.is_some());
            let alpha_multiplier = match knowledge.info_age(vis.system_entity, clock.elapsed) {
                None => 1.0, // No knowledge: keep base color as-is (already dim for unknown)
                Some(age) if age < 60 => 1.0, // Fresh (< 1 year)
                Some(age) => (1.0 - (age as f32 - 60.0) / 600.0).clamp(0.3, 1.0),
            };
            let [r, g, b, a] = base_color.to_srgba().to_f32_array();

            if glow.is_some() {
                // Glow sprites: use base color with low alpha, also apply age fading
                let glow_alpha = if star.is_capital || is_colonized {
                    0.2
                } else {
                    0.15
                };
                sprite.color = Color::srgba(r, g, b, glow_alpha * alpha_multiplier);
            } else {
                sprite.color = Color::srgba(r, g, b, a * alpha_multiplier);
            }

            // Apply zoom-responsive sizing
            if let Some(base) = base_size {
                let scaled = base.0 * zoom_factor;
                sprite.custom_size = Some(Vec2::splat(scaled));
            }
        }
    }
}

pub fn draw_galaxy_overlay(
    mut gizmos: Gizmos,
    player_q: Query<&StationedAt, With<Player>>,
    stars: Query<(Entity, &StarSystem, &Position)>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
    selected: Res<SelectedSystem>,
    selected_ship: Res<SelectedShip>,
    ships: Query<(Entity, &Ship, &ShipState)>,
    empire_params_q: Query<(Entity, &GlobalParams, &KnowledgeStore), With<PlayerEmpire>>,
    system_buildings: Query<(Entity, &SystemBuildings)>,
    colonies: Query<(&Colony, &Buildings)>,
    planets: Query<&Planet>,
    galaxy_config: Option<Res<GalaxyConfig>>,
    hostiles: Query<(&AtSystem, Option<&crate::faction::FactionOwner>), With<Hostile>>,
    faction_relations: Res<crate::faction::FactionRelations>,
    building_registry: Res<BuildingRegistry>,
) {
    // Galaxy outline: center marker and boundary circle
    if let Some(ref config) = galaxy_config {
        let boundary_radius = config.radius as f32 * view.scale;
        // Galaxy center crosshair
        gizmos.circle_2d(Vec2::ZERO, 3.0, Color::srgba(0.5, 0.5, 0.5, 0.15));
        gizmos.line_2d(
            Vec2::new(-5.0, 0.0),
            Vec2::new(5.0, 0.0),
            Color::srgba(0.5, 0.5, 0.5, 0.1),
        );
        gizmos.line_2d(
            Vec2::new(0.0, -5.0),
            Vec2::new(0.0, 5.0),
            Color::srgba(0.5, 0.5, 0.5, 0.1),
        );
        // Galaxy boundary circle
        gizmos.circle_2d(
            Vec2::ZERO,
            boundary_radius,
            Color::srgba(0.3, 0.3, 0.5, 0.08),
        );
    }

    let Ok((viewer_empire, global_params, knowledge)) = empire_params_q.single() else {
        return;
    };
    let Ok(stationed) = player_q.single() else {
        return;
    };
    let player_system = stationed.system;
    let Ok((_, _player_star, player_pos)) = stars.get(player_system) else {
        return;
    };

    let px = player_pos.x as f32 * view.scale;
    let py = player_pos.y as f32 * view.scale;

    // Capital pulsing ring (larger to match new star sizes)
    let pulse = (clock.as_years_f64() as f32 * 3.0).sin() * 0.3 + 0.7;
    gizmos.circle_2d(
        Vec2::new(px, py),
        20.0,
        Color::srgba(1.0, 0.84, 0.0, pulse),
    );

    // #176: Build colonized systems set using KnowledgeStore for remote, real-time for local
    let local_colonized: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter_map(|(c, _)| c.system(&planets))
        .collect();

    // Draw rings around colonized stars
    for (entity, star, star_pos) in &stars {
        let is_colonized = if entity == player_system {
            local_colonized.contains(&entity)
        } else {
            knowledge.get(entity)
                .map(|k| k.data.colonized)
                .unwrap_or(false)
        };
        if is_colonized && !star.is_capital {
            let sx = star_pos.x as f32 * view.scale;
            let sy = star_pos.y as f32 * view.scale;
            gizmos.circle_2d(
                Vec2::new(sx, sy),
                18.0,
                Color::srgba(0.3, 1.0, 0.3, 0.6),
            );
        }
    }

    for &radius_ly in &[5.0_f32, 10.0, 25.0, 50.0] {
        let radius_px = radius_ly * view.scale;
        gizmos.circle_2d(
            Vec2::new(px, py),
            radius_px,
            Color::srgba(0.3, 0.5, 1.0, 0.15),
        );
    }

    gizmos.circle_2d(
        Vec2::new(px, py),
        5.0 * view.scale,
        Color::srgba(0.2, 1.0, 0.2, 0.25),
    );

    // #17: Information horizon ring (where knowledge becomes older than 5 years)
    let info_horizon_ly = 5.0_f32;
    let info_horizon_px = info_horizon_ly * view.scale;
    let horizon_pulse = (clock.as_years_f64() as f32 * 1.5).sin() * 0.05 + 0.2;
    gizmos.circle_2d(
        Vec2::new(px, py),
        info_horizon_px,
        Color::srgba(1.0, 0.6, 0.0, horizon_pulse),
    );

    // #176: Survey lines use KnowledgeStore for remote systems
    for (entity, star, star_pos) in &stars {
        let is_surveyed = if entity == player_system {
            star.surveyed
        } else {
            knowledge.get(entity)
                .map(|k| k.data.surveyed)
                .unwrap_or(false)
        };
        if is_surveyed && !star.is_capital {
            let sx = star_pos.x as f32 * view.scale;
            let sy = star_pos.y as f32 * view.scale;
            gizmos.line_2d(
                Vec2::new(px, py),
                Vec2::new(sx, sy),
                Color::srgba(0.4, 0.6, 1.0, 0.15),
            );
        }
    }

    // Selection ring around selected system
    if let Some(selected_entity) = selected.0 {
        if let Ok((_, _star, sel_pos)) = stars.get(selected_entity) {
            let sx = sel_pos.x as f32 * view.scale;
            let sy = sel_pos.y as f32 * view.scale;
            let sel_pulse = (clock.as_years_f64() as f32 * 4.0).sin() * 0.2 + 0.8;
            gizmos.circle_2d(
                Vec2::new(sx, sy),
                22.0,
                Color::srgba(0.0, 1.0, 1.0, sel_pulse),
            );
        }
    }

    // #48: FTL range circle around selected ship
    if let Some(ship_entity) = selected_ship.0 {
        if let Ok((_, ship, state)) = ships.get(ship_entity) {
            let effective_range = ship.ftl_range + global_params.ftl_range_bonus;
            if effective_range > 0.0 {
                let ship_pos = match state {
                    ShipState::Docked { system } => {
                        stars.get(*system).ok().map(|(_, _, pos)| {
                            Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)
                        })
                    }
                    _ => None,
                };
                if let Some(ship_pos_px) = ship_pos {
                    let range_px = effective_range as f32 * view.scale;
                    gizmos.circle_2d(
                        ship_pos_px,
                        range_px,
                        Color::srgba(0.3, 0.5, 1.0, 0.1),
                    );
                }
            }
        }
    }

    // #52/#56/#176: Hostile presence markers — red X on surveyed systems with hostiles
    // #293: Local system: query (AtSystem, FactionOwner, With<Hostile>) filtered by
    // FactionRelations. Remote: use KnowledgeStore.
    {
        // Local system hostiles (real-time)
        for (at_system, owner) in &hostiles {
            if at_system.0 != player_system {
                continue;
            }
            // Only draw if the empire considers this faction hostile.
            // Hostiles without FactionOwner fall through (drawn) — matches
            // legacy behavior when FactionRelations isn't populated yet.
            if let Some(o) = owner {
                if !faction_relations
                    .get_or_default(viewer_empire, o.0)
                    .can_attack_aggressive()
                {
                    continue;
                }
            }
            let Ok((_, star, star_pos)) = stars.get(at_system.0) else {
                continue;
            };
            if !star.surveyed {
                continue;
            }
            let sx = star_pos.x as f32 * view.scale;
            let sy = star_pos.y as f32 * view.scale;
            let hostile_color = Color::srgba(1.0, 0.2, 0.2, 0.7);
            let s = 5.0_f32;
            gizmos.line_2d(Vec2::new(sx - s, sy - s), Vec2::new(sx + s, sy + s), hostile_color);
            gizmos.line_2d(Vec2::new(sx - s, sy + s), Vec2::new(sx + s, sy - s), hostile_color);
        }
        // Remote system hostiles (from KnowledgeStore)
        for (_entity, k) in knowledge.iter() {
            if k.system == player_system {
                continue;
            }
            if !k.data.has_hostile || !k.data.surveyed {
                continue;
            }
            let Ok((_, _, star_pos)) = stars.get(k.system) else {
                continue;
            };
            let sx = star_pos.x as f32 * view.scale;
            let sy = star_pos.y as f32 * view.scale;
            let hostile_color = Color::srgba(1.0, 0.2, 0.2, 0.7);
            let s = 5.0_f32;
            gizmos.line_2d(Vec2::new(sx - s, sy - s), Vec2::new(sx + s, sy + s), hostile_color);
            gizmos.line_2d(Vec2::new(sx - s, sy + s), Vec2::new(sx + s, sy - s), hostile_color);
        }
    }

    // #46/#176: Port facility markers - draw a diamond icon on systems with ports
    // Local system: read SystemBuildings directly. Remote: use KnowledgeStore.
    {
        // Collect port systems: local from ECS, remote from knowledge
        let mut port_system_entities: Vec<Entity> = Vec::new();
        // Local system ports (real-time)
        for (entity, sb) in &system_buildings {
            if entity == player_system && sb.has_port(&building_registry) {
                port_system_entities.push(entity);
            }
        }
        // Remote system ports (from KnowledgeStore)
        for (_entity, k) in knowledge.iter() {
            if k.system == player_system {
                continue;
            }
            if k.data.has_port {
                port_system_entities.push(k.system);
            }
        }

        for system_entity in &port_system_entities {
            if let Ok((_, _star, star_pos)) = stars.get(*system_entity) {
                let sx = star_pos.x as f32 * view.scale;
                let sy = star_pos.y as f32 * view.scale;
                let port_pulse = (clock.as_years_f64() as f32 * 2.0).sin() * 0.15 + 0.6;
                let d = 6.0_f32;
                let top = Vec2::new(sx, sy + d);
                let right = Vec2::new(sx + d, sy);
                let bottom = Vec2::new(sx, sy - d);
                let left = Vec2::new(sx - d, sy);
                let port_color = Color::srgba(0.8, 0.5, 1.0, port_pulse);
                gizmos.line_2d(top, right, port_color);
                gizmos.line_2d(right, bottom, port_color);
                gizmos.line_2d(bottom, left, port_color);
                gizmos.line_2d(left, top, port_color);
            }
        }
    }
}

pub fn draw_deep_space_structures(
    mut gizmos: Gizmos,
    structures: Query<(
        &DeepSpaceStructure,
        &Position,
        &StructureHitpoints,
        Option<&ConstructionPlatform>,
        Option<&Scrapyard>,
    )>,
    view: Res<GalaxyView>,
) {
    for (_structure, pos, _hp, platform, scrap) in &structures {
        let x = pos.x as f32 * view.scale;
        let y = pos.y as f32 * view.scale;
        // Draw a small diamond marker. #229: colour encodes lifecycle state —
        // yellow while a ConstructionPlatform is accumulating resources, red
        // while the structure is a drained/being-drained Scrapyard, blue for
        // fully active structures.
        let size = 4.0;
        let color = if platform.is_some() {
            Color::srgba(1.0, 0.9, 0.2, 0.85) // yellow — under construction
        } else if scrap.is_some() {
            Color::srgba(1.0, 0.3, 0.3, 0.85) // red — dismantled / scrapyard
        } else {
            Color::srgba(0.7, 0.7, 1.0, 0.6) // blue — active
        };
        gizmos.line_2d(Vec2::new(x, y - size), Vec2::new(x + size, y), color);
        gizmos.line_2d(Vec2::new(x + size, y), Vec2::new(x, y + size), color);
        gizmos.line_2d(Vec2::new(x, y + size), Vec2::new(x - size, y), color);
        gizmos.line_2d(Vec2::new(x - size, y), Vec2::new(x, y - size), color);
    }
}

/// #145: Draw forbidden regions as a loose union of 2D discs.
///
/// Each effective sphere (`sphere_radius / sqrt(threshold)`) is rendered as a
/// filled-looking circle via nested outlines at decreasing alpha. A proper
/// metaball iso-surface shader is a 1.0.0 task — this is intentionally coarse
/// but makes the no-go volume unambiguous on the galaxy map.
pub fn draw_forbidden_regions(
    mut gizmos: Gizmos,
    regions: Query<&crate::galaxy::ForbiddenRegion>,
    region_types: Res<crate::galaxy::RegionTypeRegistry>,
    view: Res<GalaxyView>,
) {
    for region in &regions {
        // Visual style from the type definition (fallback to muted purple).
        let (r, g, b, density) = match region_types.types.get(&region.type_id) {
            Some(t) => (
                t.visual_color[0],
                t.visual_color[1],
                t.visual_color[2],
                t.visual_density,
            ),
            None => (0.3, 0.1, 0.5, 0.6),
        };
        let base_alpha = (density * 0.6).clamp(0.1, 0.9);

        for (center, radius) in &region.spheres {
            let eff = crate::galaxy::effective_radius(*radius, region.threshold);
            if eff <= 0.0 {
                continue;
            }
            let cx = center[0] as f32 * view.scale;
            let cy = center[1] as f32 * view.scale;
            let screen_r = eff as f32 * view.scale;

            // Nested rings: outer faint, inner denser.
            for (i, frac) in [1.0_f32, 0.75, 0.5, 0.25].iter().enumerate() {
                let alpha = base_alpha * (0.25 + i as f32 * 0.18);
                gizmos.circle_2d(
                    Vec2::new(cx, cy),
                    screen_r * frac,
                    Color::srgba(r, g, b, alpha),
                );
            }
        }
    }
}
