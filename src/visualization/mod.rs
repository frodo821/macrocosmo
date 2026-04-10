pub mod territory;

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy_egui::EguiContexts;

use crate::colony::{Buildings, Colony, SystemBuildings};
use crate::components::Position;
use crate::galaxy::{ObscuredByGas, Planet, StarSystem};
use crate::knowledge::KnowledgeStore;
use crate::player::{Player, PlayerEmpire, StationedAt};
use crate::ship::{CommandQueue, QueuedCommand, Ship, ShipState};
use crate::technology::GlobalParams;
use crate::time_system::GameClock;

/// Context menu shown when left-clicking a star while a ship is selected.
#[derive(Resource, Default)]
pub struct ContextMenu {
    pub open: bool,
    pub position: [f32; 2],
    pub target_system: Option<Entity>,
    /// When true, execute the default action immediately instead of showing the menu.
    pub execute_default: bool,
}

pub struct VisualizationPlugin;

impl Plugin for VisualizationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(territory::TerritoryPlugin)
        .insert_resource(GalaxyView {
            scale: 5.0,
        })
        .insert_resource(SelectedSystem::default())
        .insert_resource(SelectedShip::default())
        .insert_resource(SelectedPlanet::default())
        .insert_resource(ContextMenu::default())
        .add_systems(Startup, setup_camera)
        .add_systems(PostStartup, (spawn_star_visuals, center_camera_on_capital))
        .add_systems(Update, (
            click_select_system,
            camera_controls,
            update_star_colors,
            draw_galaxy_overlay,
            draw_ships,
        ));
    }
}

#[derive(Resource, Default)]
pub struct SelectedSystem(pub Option<Entity>);

#[derive(Resource, Default)]
pub struct SelectedShip(pub Option<Entity>);

#[derive(Resource, Default)]
pub struct SelectedPlanet(pub Option<Entity>);

#[derive(Resource)]
pub struct GalaxyView {
    pub scale: f32,
}

#[derive(Component)]
struct StarVisual {
    system_entity: Entity,
}

/// Marks a sprite as a glow halo behind a star.
#[derive(Component)]
struct StarGlow;

/// Stores the base pixel size of a star sprite so zoom-responsive scaling can reference it.
#[derive(Component)]
struct BaseStarSize(f32);

fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.02, 0.02, 0.05)),
            ..default()
        },
    ));
}

fn center_camera_on_capital(
    mut camera_q: Query<&mut Transform, With<Camera2d>>,
    capitals: Query<(&StarSystem, &Position)>,
    view: Res<GalaxyView>,
) {
    for (star, pos) in &capitals {
        if star.is_capital {
            for mut transform in &mut camera_q {
                transform.translation.x = pos.x as f32 * view.scale;
                transform.translation.y = pos.y as f32 * view.scale;
            }
            break;
        }
    }
}

fn spawn_star_visuals(
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

fn star_color(star: &StarSystem, colonized: bool, obscured: bool) -> Color {
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
fn update_star_colors(
    stars: Query<(Entity, &StarSystem, Option<&ObscuredByGas>)>,
    mut visuals: Query<(&StarVisual, &mut Sprite, Option<&StarGlow>, Option<&BaseStarSize>)>,
    empire_q: Query<&KnowledgeStore, With<PlayerEmpire>>,
    colonies: Query<&Colony>,
    planets: Query<&Planet>,
    clock: Res<GameClock>,
    camera_q: Query<&Projection, With<Camera2d>>,
) {
    let Ok(knowledge) = empire_q.single() else {
        return;
    };
    // Build colonized systems set
    let colonized_systems: std::collections::HashSet<Entity> = colonies
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
            let is_colonized = colonized_systems.contains(&vis.system_entity);
            let base_color = star_color(star, is_colonized, obscured.is_some());
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

/// Resource set each frame by the UI system to indicate egui is consuming pointer input.
/// Camera controls check this to avoid scroll-zoom when the pointer is over a UI panel.
#[derive(Resource, Default)]
pub struct EguiWantsPointer(pub bool);

pub fn camera_controls(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut camera_q: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
    scroll: Res<AccumulatedMouseScroll>,
    egui_wants_pointer: Option<Res<EguiWantsPointer>>,
) {
    let Ok((mut transform, mut projection)) = camera_q.single_mut() else {
        return;
    };

    let current_scale = if let Projection::Orthographic(ref ortho) = *projection {
        ortho.scale
    } else {
        1.0
    };

    let pan_speed = 300.0 * current_scale * time.delta_secs();
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        transform.translation.y += pan_speed;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        transform.translation.y -= pan_speed;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        transform.translation.x -= pan_speed;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        transform.translation.x += pan_speed;
    }

    let egui_wants_input = egui_wants_pointer.is_some_and(|r| r.0);

    if scroll.delta.y != 0.0 && !egui_wants_input {
        let zoom_delta = -scroll.delta.y * 0.1;
        if let Projection::Orthographic(ref mut ortho) = *projection {
            ortho.scale = (ortho.scale + zoom_delta).clamp(0.2, 10.0);
        }
    }

    if keys.just_pressed(KeyCode::Home) {
        transform.translation.x = 0.0;
        transform.translation.y = 0.0;
        if let Projection::Orthographic(ref mut ortho) = *projection {
            ortho.scale = 1.0;
        }
    }
}

fn draw_galaxy_overlay(
    mut gizmos: Gizmos,
    player_q: Query<&StationedAt, With<Player>>,
    stars: Query<(Entity, &StarSystem, &Position)>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
    selected: Res<SelectedSystem>,
    selected_ship: Res<SelectedShip>,
    ships: Query<(Entity, &Ship, &ShipState)>,
    empire_params_q: Query<&GlobalParams, With<PlayerEmpire>>,
    system_buildings: Query<(Entity, &SystemBuildings)>,
    colonies: Query<(&Colony, &Buildings)>,
    planets: Query<&Planet>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };
    let Ok(stationed) = player_q.single() else {
        return;
    };
    let Ok((_, _player_star, player_pos)) = stars.get(stationed.system) else {
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

    // Build colonized systems set
    let colonized_system_set: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter_map(|(c, _)| c.system(&planets))
        .collect();

    // Draw rings around colonized stars
    for (entity, star, star_pos) in &stars {
        if colonized_system_set.contains(&entity) && !star.is_capital {
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

    for (_, star, star_pos) in &stars {
        if star.surveyed && !star.is_capital {
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

    // #46: Port facility markers - draw a diamond icon on systems with ports
    let port_systems: Vec<Entity> = system_buildings
        .iter()
        .filter(|(_, sb)| sb.has_port())
        .map(|(entity, _)| entity)
        .collect();

    for system_entity in &port_systems {
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

// #16: Ship drawing helpers and system

fn ship_color_rgb(design_id: &str) -> (f32, f32, f32) {
    match design_id {
        "explorer_mk1" => (0.2, 1.0, 0.2),
        "colony_ship_mk1" => (1.0, 1.0, 0.2),
        "courier_mk1" => (0.2, 1.0, 1.0),
        _ => (0.8, 0.8, 0.8), // default gray for unknown designs
    }
}

fn ship_color(design_id: &str) -> Color {
    let (r, g, b) = ship_color_rgb(design_id);
    Color::srgb(r, g, b)
}

fn draw_dashed_line(gizmos: &mut Gizmos, start: Vec2, end: Vec2, color: Color) {
    let diff = end - start;
    let length = diff.length();
    if length <= 0.0 {
        return;
    }
    let dir = diff / length;
    let dash_len = 4.0;
    let gap_len = 4.0;
    let mut d = 0.0;
    while d < length {
        let seg_start = start + dir * d;
        let seg_end = start + dir * (d + dash_len).min(length);
        gizmos.line_2d(seg_start, seg_end, color);
        d += dash_len + gap_len;
    }
}

fn draw_ships(
    mut gizmos: Gizmos,
    ships: Query<(Entity, &Ship, &ShipState, Option<&CommandQueue>)>,
    stars: Query<&Position, With<StarSystem>>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
    selected_ship: Res<SelectedShip>,
) {
    // Group docked ships by system so we can offset them.
    let mut docked_counts: HashMap<Entity, Vec<String>> = HashMap::new();
    // Also count ships per system for badge display.
    let mut system_ship_counts: HashMap<Entity, u32> = HashMap::new();

    for (_entity, ship, state, _queue) in &ships {
        match state {
            ShipState::Docked { system } => {
                docked_counts
                    .entry(*system)
                    .or_default()
                    .push(ship.design_id.clone());
                *system_ship_counts.entry(*system).or_insert(0) += 1;
            }
            ShipState::SubLight {
                origin,
                destination,
                departed_at,
                arrival_at,
                ..
            } => {
                let total = (*arrival_at - *departed_at) as f64;
                let elapsed = (clock.elapsed - *departed_at) as f64;
                let t = if total > 0.0 {
                    (elapsed / total).clamp(0.0, 1.0)
                } else {
                    1.0
                };

                let cx = (origin[0] + (destination[0] - origin[0]) * t) as f32 * view.scale;
                let cy = (origin[1] + (destination[1] - origin[1]) * t) as f32 * view.scale;

                let (r, g, b) = ship_color_rgb(&ship.design_id);

                // Draw ship marker
                gizmos.circle_2d(Vec2::new(cx, cy), 3.5, Color::srgb(r, g, b));

                // Draw movement path as dashed line segments
                let dest_x = destination[0] as f32 * view.scale;
                let dest_y = destination[1] as f32 * view.scale;
                draw_dashed_line(
                    &mut gizmos,
                    Vec2::new(cx, cy),
                    Vec2::new(dest_x, dest_y),
                    Color::srgba(r, g, b, 0.5),
                );
            }
            ShipState::InFTL {
                origin_system,
                destination_system,
                departed_at,
                arrival_at,
            } => {
                // #31: Ghost marker showing estimated FTL position
                let (Ok(origin_pos), Ok(dest_pos)) =
                    (stars.get(*origin_system), stars.get(*destination_system))
                else {
                    continue;
                };

                let total = (*arrival_at - *departed_at) as f64;
                let elapsed = (clock.elapsed - *departed_at) as f64;
                let t = if total > 0.0 {
                    (elapsed / total).clamp(0.0, 1.0)
                } else {
                    1.0
                };

                let cx = (origin_pos.x + (dest_pos.x - origin_pos.x) * t) as f32 * view.scale;
                let cy = (origin_pos.y + (dest_pos.y - origin_pos.y) * t) as f32 * view.scale;

                let (r, g, b) = ship_color_rgb(&ship.design_id);

                // Ghost marker: semi-transparent, smaller circle
                gizmos.circle_2d(Vec2::new(cx, cy), 3.0, Color::srgba(r, g, b, 0.4));

                // Dashed trajectory line from current position to destination
                let dest_x = dest_pos.x as f32 * view.scale;
                let dest_y = dest_pos.y as f32 * view.scale;
                draw_dashed_line(
                    &mut gizmos,
                    Vec2::new(cx, cy),
                    Vec2::new(dest_x, dest_y),
                    Color::srgba(r, g, b, 0.25),
                );
            }
            ShipState::Settling { system, .. } => {
                // Draw settling ships at the target system with a pulsing indicator
                if let Ok(sys_pos) = stars.get(*system) {
                    let sx = sys_pos.x as f32 * view.scale;
                    let sy = sys_pos.y as f32 * view.scale;
                    let (r, g, b) = ship_color_rgb(&ship.design_id);
                    let pulse = (clock.as_years_f64() as f32 * 3.0).sin() * 0.3 + 0.7;
                    gizmos.circle_2d(
                        Vec2::new(sx, sy),
                        6.0,
                        Color::srgba(r, g, b, pulse),
                    );
                    gizmos.circle_2d(Vec2::new(sx, sy), 3.5, Color::srgb(r, g, b));
                }
            }
            ShipState::Surveying { target_system, .. } => {
                if let Ok(sys_pos) = stars.get(*target_system) {
                    let sx = sys_pos.x as f32 * view.scale;
                    let sy = sys_pos.y as f32 * view.scale;
                    let (r, g, b) = ship_color_rgb(&ship.design_id);

                    // Pulsing indicator
                    let pulse = (clock.as_years_f64() as f32 * 5.0).sin() * 0.3 + 0.7;
                    gizmos.circle_2d(
                        Vec2::new(sx, sy),
                        6.0,
                        Color::srgba(r, g, b, pulse),
                    );

                    // Ship marker
                    gizmos.circle_2d(Vec2::new(sx, sy), 3.5, Color::srgb(r, g, b));
                }
            }
            ShipState::Refitting { system, .. } => {
                // Refitting ships are docked — show them at the system
                docked_counts
                    .entry(*system)
                    .or_default()
                    .push(ship.design_id.clone());
                *system_ship_counts.entry(*system).or_insert(0) += 1;
            }
        }
    }

    // Draw docked ships offset around their system.
    for (system_entity, design_ids) in &docked_counts {
        let Ok(sys_pos) = stars.get(*system_entity) else {
            continue;
        };
        let sx = sys_pos.x as f32 * view.scale;
        let sy = sys_pos.y as f32 * view.scale;
        let count = design_ids.len();

        for (i, design_id) in design_ids.iter().enumerate() {
            let angle = if count == 1 {
                0.0
            } else {
                std::f32::consts::TAU * (i as f32) / (count as f32)
            };
            let offset_radius = 8.0;
            let ox = sx + angle.cos() * offset_radius;
            let oy = sy + angle.sin() * offset_radius;

            let color = ship_color(design_id);
            gizmos.circle_2d(Vec2::new(ox, oy), 3.0, color);
        }
    }

    // Draw ship count badges near systems with docked ships.
    for (system_entity, count) in &system_ship_counts {
        if *count == 0 {
            continue;
        }
        let Ok(sys_pos) = stars.get(*system_entity) else {
            continue;
        };
        let sx = sys_pos.x as f32 * view.scale;
        let sy = sys_pos.y as f32 * view.scale;

        // Draw a small badge background circle offset to the upper-right
        let badge_x = sx + 12.0;
        let badge_y = sy + 12.0;
        let badge_radius = 5.0;
        gizmos.circle_2d(
            Vec2::new(badge_x, badge_y),
            badge_radius,
            Color::srgba(0.1, 0.1, 0.3, 0.8),
        );
        // Draw dots inside the badge to represent count (up to 4, then filled circle)
        if *count <= 4 {
            for j in 0..*count {
                let dot_angle = std::f32::consts::TAU * (j as f32) / (*count as f32);
                let dot_r = 2.0;
                let dx = badge_x + dot_angle.cos() * dot_r;
                let dy = badge_y + dot_angle.sin() * dot_r;
                gizmos.circle_2d(Vec2::new(dx, dy), 1.0, Color::WHITE);
            }
        } else {
            // Filled circle for 5+ ships
            gizmos.circle_2d(Vec2::new(badge_x, badge_y), 3.5, Color::WHITE);
        }
    }

    // #104: Command queue overlay for selected ship
    if let Some(selected_entity) = selected_ship.0 {
        if let Ok((_entity, ship, state, Some(queue))) = ships.get(selected_entity) {
            if !queue.commands.is_empty() {
                // Determine the ship's current screen position from its state
                let current_pos = match state {
                    ShipState::Docked { system } => {
                        stars.get(*system).ok().map(|pos| {
                            Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)
                        })
                    }
                    ShipState::SubLight {
                        origin,
                        destination,
                        departed_at,
                        arrival_at,
                        ..
                    } => {
                        let total = (*arrival_at - *departed_at) as f64;
                        let elapsed = (clock.elapsed - *departed_at) as f64;
                        let t = if total > 0.0 {
                            (elapsed / total).clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        let cx =
                            (origin[0] + (destination[0] - origin[0]) * t) as f32 * view.scale;
                        let cy =
                            (origin[1] + (destination[1] - origin[1]) * t) as f32 * view.scale;
                        Some(Vec2::new(cx, cy))
                    }
                    ShipState::InFTL {
                        origin_system,
                        destination_system,
                        departed_at,
                        arrival_at,
                    } => {
                        if let (Ok(origin_pos), Ok(dest_pos)) =
                            (stars.get(*origin_system), stars.get(*destination_system))
                        {
                            let total = (*arrival_at - *departed_at) as f64;
                            let elapsed = (clock.elapsed - *departed_at) as f64;
                            let t = if total > 0.0 {
                                (elapsed / total).clamp(0.0, 1.0)
                            } else {
                                1.0
                            };
                            let cx = (origin_pos.x + (dest_pos.x - origin_pos.x) * t) as f32
                                * view.scale;
                            let cy = (origin_pos.y + (dest_pos.y - origin_pos.y) * t) as f32
                                * view.scale;
                            Some(Vec2::new(cx, cy))
                        } else {
                            None
                        }
                    }
                    ShipState::Settling { system, .. } => {
                        stars.get(*system).ok().map(|pos| {
                            Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)
                        })
                    }
                    ShipState::Surveying { target_system, .. } => {
                        stars.get(*target_system).ok().map(|pos| {
                            Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)
                        })
                    }
                    ShipState::Refitting { system, .. } => {
                        stars.get(*system).ok().map(|pos| {
                            Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)
                        })
                    }
                };

                if let Some(mut prev_pos) = current_pos {
                    let (r, g, b) = ship_color_rgb(&ship.design_id);

                    for cmd in &queue.commands {
                        let target_system = match cmd {
                            QueuedCommand::MoveTo { system, .. }
                            | QueuedCommand::Survey { system, .. }
                            | QueuedCommand::Colonize { system, .. } => *system,
                        };

                        let Ok(target_pos) = stars.get(target_system) else {
                            continue;
                        };
                        let tx = target_pos.x as f32 * view.scale;
                        let ty = target_pos.y as f32 * view.scale;
                        let target_screen = Vec2::new(tx, ty);

                        // Dashed path line from previous position to target
                        draw_dashed_line(
                            &mut gizmos,
                            prev_pos,
                            target_screen,
                            Color::srgba(r, g, b, 0.3),
                        );

                        // Command-specific markers
                        match cmd {
                            QueuedCommand::MoveTo { .. } => {
                                gizmos.circle_2d(
                                    target_screen,
                                    4.0,
                                    Color::srgba(r, g, b, 0.5),
                                );
                            }
                            QueuedCommand::Survey { .. } => {
                                gizmos.circle_2d(
                                    target_screen,
                                    6.0,
                                    Color::srgba(0.2, 1.0, 0.2, 0.4),
                                );
                                gizmos.circle_2d(
                                    target_screen,
                                    3.0,
                                    Color::srgba(0.2, 1.0, 0.2, 0.6),
                                );
                            }
                            QueuedCommand::Colonize { .. } => {
                                gizmos.circle_2d(
                                    target_screen,
                                    5.0,
                                    Color::srgba(1.0, 1.0, 0.2, 0.5),
                                );
                            }
                        }

                        prev_pos = target_screen;
                    }
                }
            }
        }
    }
}

pub fn click_select_system(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    stars: Query<(Entity, &StarSystem, &Position, Option<&ObscuredByGas>)>,
    ship_q: Query<(Entity, &Ship, &ShipState)>,
    star_positions: Query<&Position, With<StarSystem>>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
    mut selected: ResMut<SelectedSystem>,
    mut selected_ship: ResMut<SelectedShip>,
    mut context_menu: ResMut<ContextMenu>,
    mut egui_contexts: EguiContexts,
) {
    // Escape handling
    if keys.just_pressed(KeyCode::Escape) {
        if selected_ship.0.is_some() {
            selected_ship.0 = None;
            context_menu.open = false;
        } else {
            selected.0 = None;
        }
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    // Don't process clicks that landed on egui panels
    if let Ok(ctx) = egui_contexts.ctx_mut() {
        if ctx.is_pointer_over_area() {
            return;
        }
    }

    // Close any open context menu on any click outside egui
    context_menu.open = false;

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, global_transform)) = camera_q.single() else {
        return;
    };
    let Ok(world_pos) = camera.viewport_to_world_2d(global_transform, cursor_pos) else {
        selected.0 = None;
        selected_ship.0 = None;
        return;
    };

    let cmd_held = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    // Check in-transit and active ships (docked ships are selected via the outline panel)
    let ship_click_radius = 12.0;
    let mut best_ship: Option<(Entity, f32)> = None;
    for (entity, _ship, state) in &ship_q {
        let ship_px = match state {
            ShipState::SubLight {
                origin,
                destination,
                departed_at,
                arrival_at,
                ..
            } => {
                let total = (*arrival_at - *departed_at) as f64;
                let elapsed = (clock.elapsed - *departed_at) as f64;
                let t = if total > 0.0 {
                    (elapsed / total).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let cx = (origin[0] + (destination[0] - origin[0]) * t) as f32 * view.scale;
                let cy = (origin[1] + (destination[1] - origin[1]) * t) as f32 * view.scale;
                Vec2::new(cx, cy)
            }
            ShipState::Settling { system, .. } | ShipState::Surveying { target_system: system, .. } => {
                let Ok(sys_pos) = star_positions.get(*system) else {
                    continue;
                };
                Vec2::new(sys_pos.x as f32 * view.scale, sys_pos.y as f32 * view.scale)
            }
            ShipState::InFTL {
                origin_system,
                destination_system,
                departed_at,
                arrival_at,
            } => {
                let (Ok(origin_pos), Ok(dest_pos)) =
                    (star_positions.get(*origin_system), star_positions.get(*destination_system))
                else {
                    continue;
                };
                let total = (*arrival_at - *departed_at) as f64;
                let elapsed = (clock.elapsed - *departed_at) as f64;
                let t = if total > 0.0 {
                    (elapsed / total).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let cx = (origin_pos.x + (dest_pos.x - origin_pos.x) * t) as f32 * view.scale;
                let cy = (origin_pos.y + (dest_pos.y - origin_pos.y) * t) as f32 * view.scale;
                Vec2::new(cx, cy)
            }
            // Docked ships selected via outline panel
            _ => continue,
        };

        let dist = world_pos.distance(ship_px);
        if dist < ship_click_radius {
            if best_ship.is_none() || dist < best_ship.unwrap().1 {
                best_ship = Some((entity, dist));
            }
        }
    }

    // Clicking on a ship always selects that ship (regardless of current selection)
    if let Some((ship_entity, _)) = best_ship {
        selected_ship.0 = Some(ship_entity);
        return;
    }

    // Find the closest star under cursor
    let click_radius = 15.0;
    let mut best_star: Option<(Entity, f32)> = None;

    for (entity, _star, pos, obscured) in &stars {
        if obscured.is_some() {
            continue;
        }
        let star_px = Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale);
        let dist = world_pos.distance(star_px);
        if dist < click_radius {
            if best_star.is_none() || dist < best_star.unwrap().1 {
                best_star = Some((entity, dist));
            }
        }
    }

    // When a ship IS selected AND Cmd is held: context menu / default action
    if selected_ship.0.is_some() && cmd_held {
        if let Some((star_entity, _)) = best_star {
            context_menu.open = true;
            context_menu.position = [cursor_pos.x, cursor_pos.y];
            context_menu.target_system = Some(star_entity);
            context_menu.execute_default = shift_held; // Cmd+Shift = default action
            return;
        }
    }

    // Normal click: select star system (works whether or not ship is selected)
    if let Some((entity, _)) = best_star {
        selected.0 = Some(entity);
        // Keep ship selected — star becomes command target
    } else {
        // Clicked empty space
        selected.0 = None;
        selected_ship.0 = None;
    }
}

