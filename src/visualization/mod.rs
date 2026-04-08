use std::collections::HashMap;

use bevy::prelude::*;
use bevy::input::mouse::AccumulatedMouseScroll;

use crate::colony::{BuildOrder, BuildQueue, Colony, Production, ResourceStockpile};
use crate::components::Position;
use crate::galaxy::{ObscuredByGas, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::ship::{Ship, ShipState, ShipType};
use crate::time_system::{GameClock, GameSpeed, SEXADIES_PER_YEAR};

pub struct VisualizationPlugin;

impl Plugin for VisualizationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GalaxyView {
            scale: 5.0,
        })
        .insert_resource(SelectedSystem::default())
        .insert_resource(SelectedShip::default())
        .add_systems(Startup, setup_camera)
        .add_systems(PostStartup, spawn_star_visuals)
        .add_systems(Update, (
            click_select_system,
            camera_controls,
            update_star_colors,
            draw_galaxy_overlay,
            draw_ships,
            update_hud,
            update_info_panel,
            handle_ship_commands,
            handle_build_commands,
        ));
    }
}

#[derive(Resource, Default)]
struct SelectedSystem(Option<Entity>);

#[derive(Resource, Default)]
struct SelectedShip(Option<Entity>);

#[derive(Resource)]
struct GalaxyView {
    scale: f32,
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

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct InfoPanel;

fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.02, 0.02, 0.05)),
            ..default()
        },
    ));

    commands.spawn((
        HudText,
        Text::new(""),
        TextFont {
            font_size: 16.0,
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));

    commands.spawn((
        InfoPanel,
        Text::new(""),
        TextFont {
            font_size: 14.0,
            ..default()
        },
        TextColor(Color::srgb(0.9, 0.95, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            right: Val::Px(10.0),
            ..default()
        },
    ));
}

fn spawn_star_visuals(
    mut commands: Commands,
    stars: Query<(Entity, &StarSystem, &Position, Option<&ObscuredByGas>)>,
    view: Res<GalaxyView>,
) {
    for (entity, star, pos, obscured) in &stars {
        let x = pos.x as f32 * view.scale;
        let y = pos.y as f32 * view.scale;
        let is_obscured = obscured.is_some();

        let color = star_color(star, is_obscured);

        // Determine base size based on star status
        let size = if star.is_capital {
            16.0
        } else if star.colonized {
            14.0
        } else if star.surveyed {
            12.0
        } else {
            10.0
        };

        // Spawn glow halo behind the star (skip for obscured stars)
        if !is_obscured {
            let [r, g, b, _] = color.to_srgba().to_f32_array();
            let glow_alpha = if star.is_capital || star.colonized {
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
            } else if star.colonized {
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

fn star_color(star: &StarSystem, obscured: bool) -> Color {
    if obscured {
        Color::srgba(0.2, 0.2, 0.25, 0.15) // Barely visible
    } else if star.is_capital {
        Color::srgb(1.0, 0.84, 0.0) // Gold
    } else if star.colonized {
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
    knowledge: Res<KnowledgeStore>,
    clock: Res<GameClock>,
    camera_q: Query<&Projection, With<Camera2d>>,
) {
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
            let base_color = star_color(star, obscured.is_some());
            let alpha_multiplier = match knowledge.info_age(vis.system_entity, clock.elapsed) {
                None => 1.0, // No knowledge: keep base color as-is (already dim for unknown)
                Some(age) if age < 60 => 1.0, // Fresh (< 1 year)
                Some(age) => (1.0 - (age as f32 - 60.0) / 600.0).clamp(0.3, 1.0),
            };
            let [r, g, b, a] = base_color.to_srgba().to_f32_array();

            if glow.is_some() {
                // Glow sprites: use base color with low alpha, also apply age fading
                let glow_alpha = if star.is_capital || star.colonized {
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

fn camera_controls(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut camera_q: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
    scroll: Res<AccumulatedMouseScroll>,
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

    if scroll.delta.y != 0.0 {
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
    stars: Query<(&StarSystem, &Position)>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
    selected: Res<SelectedSystem>,
) {
    let Ok(stationed) = player_q.single() else {
        return;
    };
    let Ok((_player_star, player_pos)) = stars.get(stationed.system) else {
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

    // Draw rings around colonized stars
    for (star, star_pos) in &stars {
        if star.colonized && !star.is_capital {
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

    for (star, star_pos) in &stars {
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
        if let Ok((_star, sel_pos)) = stars.get(selected_entity) {
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
}

// #16: Ship drawing helpers and system

fn ship_color_rgb(ship_type: ShipType) -> (f32, f32, f32) {
    match ship_type {
        ShipType::Explorer => (0.2, 1.0, 0.2),
        ShipType::ColonyShip => (1.0, 1.0, 0.2),
        ShipType::Courier => (0.2, 1.0, 1.0),
    }
}

fn ship_color(ship_type: ShipType) -> Color {
    let (r, g, b) = ship_color_rgb(ship_type);
    Color::srgb(r, g, b)
}

fn draw_ships(
    mut gizmos: Gizmos,
    ships: Query<(&Ship, &ShipState)>,
    stars: Query<&Position, With<StarSystem>>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
) {
    // Group docked ships by system so we can offset them.
    let mut docked_counts: HashMap<Entity, Vec<ShipType>> = HashMap::new();
    // Also count ships per system for badge display.
    let mut system_ship_counts: HashMap<Entity, u32> = HashMap::new();

    for (ship, state) in &ships {
        match state {
            ShipState::Docked { system } => {
                docked_counts
                    .entry(*system)
                    .or_default()
                    .push(ship.ship_type);
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

                let (r, g, b) = ship_color_rgb(ship.ship_type);

                // Draw ship marker
                gizmos.circle_2d(Vec2::new(cx, cy), 3.5, Color::srgb(r, g, b));

                // Draw movement path as dashed line segments
                let dest_x = destination[0] as f32 * view.scale;
                let dest_y = destination[1] as f32 * view.scale;
                let start = Vec2::new(cx, cy);
                let end = Vec2::new(dest_x, dest_y);
                let diff = end - start;
                let length = diff.length();
                if length > 0.0 {
                    let dir = diff / length;
                    let dash_len = 4.0;
                    let gap_len = 4.0;
                    let mut d = 0.0;
                    while d < length {
                        let seg_start = start + dir * d;
                        let seg_end = start + dir * (d + dash_len).min(length);
                        gizmos.line_2d(
                            seg_start,
                            seg_end,
                            Color::srgba(r, g, b, 0.5),
                        );
                        d += dash_len + gap_len;
                    }
                }
            }
            ShipState::InFTL { .. } => {
                // Ships in FTL are undetectable; do not draw.
            }
            ShipState::Surveying { target_system, .. } => {
                if let Ok(sys_pos) = stars.get(*target_system) {
                    let sx = sys_pos.x as f32 * view.scale;
                    let sy = sys_pos.y as f32 * view.scale;
                    let (r, g, b) = ship_color_rgb(ship.ship_type);

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
        }
    }

    // Draw docked ships offset around their system.
    for (system_entity, ship_types) in &docked_counts {
        let Ok(sys_pos) = stars.get(*system_entity) else {
            continue;
        };
        let sx = sys_pos.x as f32 * view.scale;
        let sy = sys_pos.y as f32 * view.scale;
        let count = ship_types.len();

        for (i, ship_type) in ship_types.iter().enumerate() {
            let angle = if count == 1 {
                0.0
            } else {
                std::f32::consts::TAU * (i as f32) / (count as f32)
            };
            let offset_radius = 8.0;
            let ox = sx + angle.cos() * offset_radius;
            let oy = sy + angle.sin() * offset_radius;

            let color = ship_color(*ship_type);
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
}

// #14: Helper to collect ships docked at a given system
fn ships_docked_at(
    system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState)>,
) -> Vec<(Entity, String, ShipType)> {
    let mut result: Vec<(Entity, String, ShipType)> = ships
        .iter()
        .filter_map(|(e, ship, state)| {
            if let ShipState::Docked { system: s } = state {
                if *s == system {
                    return Some((e, ship.name.clone(), ship.ship_type));
                }
            }
            None
        })
        .collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    result
}

fn update_hud(
    clock: Res<GameClock>,
    speed: Res<GameSpeed>,
    player_q: Query<&StationedAt, With<Player>>,
    stars: Query<(&StarSystem, &Position)>,
    ships: Query<(Entity, &Ship, &ShipState)>,
    colonies: Query<(&Colony, &ResourceStockpile, &BuildQueue)>,
    selected_system: Res<SelectedSystem>,
    selected_ship: Res<SelectedShip>,
    knowledge: Res<KnowledgeStore>,
    mut hud: Query<(&mut Text, &mut TextColor), With<HudText>>,
) {
    let Ok((mut text, mut text_color)) = hud.single_mut() else {
        return;
    };

    let speed_str = if speed.sexadies_per_second <= 0.0 {
        "PAUSED".to_string()
    } else {
        format!("x{:.0} sd/s", speed.sexadies_per_second)
    };

    let location = if let Ok(stationed) = player_q.single() {
        if let Ok((star, _)) = stars.get(stationed.system) {
            star.name.clone()
        } else {
            "Unknown".to_string()
        }
    } else {
        "In transit".to_string()
    };

    // #17: Info age display
    let info_age_str = if let Ok(stationed) = player_q.single() {
        match knowledge.info_age(stationed.system, clock.elapsed) {
            Some(age) => {
                let years = age as f64 / SEXADIES_PER_YEAR as f64;
                let freshness = if age < 60 {
                    "FRESH"
                } else if age < 300 {
                    "AGING"
                } else if age < 600 {
                    "OLD"
                } else {
                    "VERY OLD"
                };
                format!(
                    "\nInformation age: {} sd ({:.1} years) [{}]",
                    age, years, freshness
                )
            }
            None => String::new(),
        }
    } else {
        String::new()
    };

    // #17: Color HUD text based on info freshness
    let hud_color = if let Ok(stationed) = player_q.single() {
        match knowledge.info_age(stationed.system, clock.elapsed) {
            Some(age) if age < 60 => Color::srgb(0.2, 1.0, 0.2),   // Green: fresh
            Some(age) if age < 300 => Color::srgb(1.0, 1.0, 0.2),  // Yellow: aging
            Some(_) => Color::srgb(1.0, 0.3, 0.3),                  // Red: old
            None => Color::srgba(0.6, 0.6, 0.6, 0.7),              // Gray: no info
        }
    } else {
        Color::WHITE
    };
    *text_color = TextColor(hud_color);

    let mut hud_text = format!(
        "Year {} Month {} Sexadie {} [{}]\nLocation: {}{}\n\nWASD: Pan | Scroll: Zoom | Space: Pause\n+/-: Speed | I: System Info | Home: Reset View\nClick: Select system | Esc: Deselect",
        clock.year(),
        clock.month(),
        clock.sexadie(),
        speed_str,
        location,
        info_age_str,
    );

    // #15: Build menu and resource display at player's colony
    if let Ok(stationed) = player_q.single() {
        for (colony, stockpile, build_queue) in &colonies {
            if colony.system == stationed.system {
                hud_text.push_str(&format!(
                    "\n\n--- Build Menu ---\nF1: Explorer (M:200 E:100)\nF2: Colony Ship (M:500 E:300)\nF3: Courier (M:100 E:50)\n"
                ));

                // Build queue status
                if build_queue.queue.is_empty() {
                    hud_text.push_str("\nBuild Queue: [empty]");
                } else {
                    hud_text.push_str("\nBuild Queue:");
                    for order in &build_queue.queue {
                        let m_pct = if order.minerals_cost > 0.0 {
                            (order.minerals_invested / order.minerals_cost * 100.0).min(100.0)
                        } else {
                            100.0
                        };
                        let e_pct = if order.energy_cost > 0.0 {
                            (order.energy_invested / order.energy_cost * 100.0).min(100.0)
                        } else {
                            100.0
                        };
                        let pct = m_pct.min(e_pct);
                        hud_text.push_str(&format!(
                            " [{}: {:.0}%]",
                            order.ship_type_name, pct,
                        ));
                    }
                }

                hud_text.push_str(&format!(
                    "\nResources: M:{:.1} E:{:.1} R:{:.1}",
                    stockpile.minerals, stockpile.energy, stockpile.research,
                ));

                break;
            }
        }
    }

    // #14: Show selected system info and ship list / ship details in HUD
    if let Some(sel_entity) = selected_system.0 {
        if let Ok((star, pos)) = stars.get(sel_entity) {
            hud_text.push_str(&format!(
                "\n\n=== {} ===\nPos: ({:.1}, {:.1}, {:.1}) ly",
                star.name, pos.x, pos.y, pos.z,
            ));
            if star.surveyed { hud_text.push_str(" [Surveyed]"); }
            if star.colonized { hud_text.push_str(" [Colonized]"); }

            // Show distance from player
            if let Ok(stationed) = player_q.single() {
                if let Ok((_, player_pos)) = stars.get(stationed.system) {
                    let dist = physics::distance_ly(player_pos, pos);
                    hud_text.push_str(&format!("\nDistance: {:.1} ly", dist));
                }
            }

            // If a ship is selected, show ship details instead of ship list
            if let Some(ship_entity) = selected_ship.0 {
                if let Ok((_, ship, state)) = ships.get(ship_entity) {
                    hud_text.push_str(&format!(
                        "\n\n--- Ship: {} ---\nType: {:?} | HP: {:.0}/{:.0}",
                        ship.name, ship.ship_type, ship.hp, ship.max_hp,
                    ));

                    let status = match state {
                        ShipState::Docked { .. } => "Docked".to_string(),
                        ShipState::SubLight { .. } => "Sub-light travel".to_string(),
                        ShipState::InFTL { .. } => "FTL travel".to_string(),
                        ShipState::Surveying { .. } => "Surveying".to_string(),
                    };
                    hud_text.push_str(&format!("\nStatus: {}", status));

                    if ship.ftl_range > 0.0 {
                        hud_text.push_str(&format!("\nFTL range: {:.1} ly", ship.ftl_range));
                    }
                    hud_text.push_str(&format!(
                        "\nSub-light speed: {:.0}% c",
                        ship.sublight_speed * 100.0,
                    ));

                    // Show available commands if docked
                    if let ShipState::Docked { system } = state {
                        hud_text.push_str("\n\nCommands:");
                        if ship.ftl_range > 0.0 {
                            hud_text.push_str("\n  F: FTL jump (select target system first)");
                        }
                        hud_text.push_str("\n  M: Sub-light move (select target system first)");
                        if ship.ship_type == ShipType::Explorer {
                            hud_text.push_str("\n  V: Survey nearby system");
                        }

                        // If the selected system is different from where ship is docked,
                        // show what the target would be
                        if sel_entity != *system {
                            if let Ok((target_star, target_pos)) = stars.get(sel_entity) {
                                if let Ok((_, dock_pos)) = stars.get(*system) {
                                    let dist = physics::distance_ly(dock_pos, target_pos);
                                    hud_text.push_str(&format!(
                                        "\n\nTarget: {} ({:.1} ly)",
                                        target_star.name, dist,
                                    ));
                                    if ship.ftl_range > 0.0 {
                                        if dist <= ship.ftl_range && target_star.surveyed {
                                            hud_text.push_str(" [FTL OK]");
                                        } else if dist > ship.ftl_range {
                                            hud_text.push_str(" [Out of FTL range]");
                                        } else if !target_star.surveyed {
                                            hud_text.push_str(" [Unsurveyed - no FTL]");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    hud_text.push_str("\n  Esc: Back to system view");
                } else {
                    hud_text.push_str("\n\n[Selected ship no longer exists]");
                }
            } else {
                // No ship selected - show docked ship list
                let docked = ships_docked_at(sel_entity, &ships);
                if !docked.is_empty() {
                    hud_text.push_str("\n\n--- Ships ---");
                    for (i, (_entity, name, ship_type)) in docked.iter().enumerate() {
                        if i >= 9 { break; }
                        hud_text.push_str(&format!(
                            "\n[{}] {} ({:?})",
                            i + 1, name, ship_type,
                        ));
                    }
                    hud_text.push_str("\nPress 1-9 to select ship");
                }
            }
        }
    }

    **text = hud_text;
}

fn click_select_system(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    stars: Query<(Entity, &Position, Option<&ObscuredByGas>), With<StarSystem>>,
    view: Res<GalaxyView>,
    mut selected: ResMut<SelectedSystem>,
    mut selected_ship: ResMut<SelectedShip>,
) {
    // Deselect on Escape (if no ship is selected; ship Esc is handled in handle_ship_commands)
    if keys.just_pressed(KeyCode::Escape) && selected_ship.0.is_none() {
        selected.0 = None;
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

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

    let click_radius = 15.0; // pixels
    let mut best: Option<(Entity, f32)> = None;

    for (entity, pos, obscured) in &stars {
        if obscured.is_some() {
            continue;
        }
        let star_px = Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale);
        let dist = world_pos.distance(star_px);
        if dist < click_radius {
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((entity, dist));
            }
        }
    }

    if let Some((entity, _)) = best {
        // If clicking a different system, deselect ship
        if selected.0 != Some(entity) {
            selected_ship.0 = None;
        }
        selected.0 = Some(entity);
    } else {
        selected.0 = None;
        selected_ship.0 = None;
    }
}

fn update_info_panel(
    selected: Res<SelectedSystem>,
    stars: Query<(&StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: Query<&StationedAt, With<Player>>,
    all_positions: Query<&Position>,
    colonies: Query<(&Colony, Option<&Production>)>,
    knowledge: Res<KnowledgeStore>,
    clock: Res<GameClock>,
    mut panel: Query<&mut Text, With<InfoPanel>>,
) {
    let Ok(mut text) = panel.single_mut() else {
        return;
    };

    let Some(selected_entity) = selected.0 else {
        **text = String::new();
        return;
    };

    let Ok((star, star_pos, attrs)) = stars.get(selected_entity) else {
        **text = String::new();
        return;
    };

    let mut info = format!("=== {} ===\n", star.name);

    // Distance from player
    if let Ok(stationed) = player_q.single() {
        if let Ok(player_pos) = all_positions.get(stationed.system) {
            let dist = physics::distance_ly(player_pos, star_pos);
            let delay_sd = physics::light_delay_sexadies(dist);
            let delay_yr = physics::light_delay_years(dist);
            info.push_str(&format!("Distance: {:.1} ly\n", dist));
            info.push_str(&format!(
                "Light delay: {} sd ({:.1} yr)\n",
                delay_sd, delay_yr
            ));
        }
    }

    // Survey status
    if star.surveyed {
        info.push_str("Status: Surveyed\n");
    } else {
        info.push_str("Status: Unsurveyed\n");
        info.push_str("Approximate position only.\nSurvey required.\n");
    }

    // Attributes (if surveyed and available)
    if star.surveyed {
        if let Some(attrs) = attrs {
            info.push_str(&format!(
                "Habitability: {:?}\n",
                attrs.habitability
            ));
            info.push_str(&format!(
                "Minerals: {:?}\n",
                attrs.mineral_richness
            ));
            info.push_str(&format!(
                "Energy: {:?}\n",
                attrs.energy_potential
            ));
            info.push_str(&format!(
                "Research: {:?}\n",
                attrs.research_potential
            ));
            info.push_str(&format!(
                "Building slots: {}\n",
                attrs.max_building_slots
            ));
        }
    }

    // Colony info
    if star.colonized {
        info.push_str("\n--- Colony ---\n");
        for (colony, production) in &colonies {
            if colony.system == selected_entity {
                info.push_str(&format!(
                    "Population: {:.0}\n",
                    colony.population
                ));
                if let Some(prod) = production {
                    info.push_str(&format!(
                        "Production: M {:.1} | E {:.1} | R {:.1}\n",
                        prod.minerals_per_sexadie,
                        prod.energy_per_sexadie,
                        prod.research_per_sexadie,
                    ));
                }
                break;
            }
        }
    }

    // Knowledge age with #17 freshness display
    if let Some(age) = knowledge.info_age(selected_entity, clock.elapsed) {
        let years = age as f64 / SEXADIES_PER_YEAR as f64;
        let freshness = if age < 60 {
            "FRESH"
        } else if age < 300 {
            "AGING"
        } else if age < 600 {
            "OLD"
        } else {
            "VERY OLD"
        };
        info.push_str(&format!(
            "\nInformation age: {} sd ({:.1} yr) [{}]\n",
            age, years, freshness
        ));
    }

    **text = info;
}

// #14: Ship command handling
fn handle_ship_commands(
    keys: Res<ButtonInput<KeyCode>>,
    selected_system: Res<SelectedSystem>,
    mut selected_ship: ResMut<SelectedShip>,
    ships_query: Query<(Entity, &Ship, &ShipState)>,
    mut ship_writer: Query<(&mut Ship, &mut ShipState)>,
    stars: Query<(Entity, &StarSystem, &Position)>,
    clock: Res<GameClock>,
) {
    // Esc deselects the ship
    if keys.just_pressed(KeyCode::Escape) {
        if selected_ship.0.is_some() {
            selected_ship.0 = None;
            return;
        }
    }

    let Some(sel_system) = selected_system.0 else { return };

    // Number keys 1-9 to select a ship from the docked list
    if selected_ship.0.is_none() {
        let digit_keys = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        for (i, key) in digit_keys.iter().enumerate() {
            if keys.just_pressed(*key) {
                let docked = ships_docked_at(sel_system, &ships_query);
                if i < docked.len() {
                    selected_ship.0 = Some(docked[i].0);
                    info!("Selected ship: {}", docked[i].1);
                }
                return;
            }
        }
        return;
    }

    // A ship is selected - handle command keys
    let ship_entity = selected_ship.0.unwrap();

    // Get ship data (immutable first for validation)
    let Ok((_, ship, state)) = ships_query.get(ship_entity) else {
        selected_ship.0 = None;
        return;
    };

    let ShipState::Docked { system: docked_system } = *state else {
        // Ship is not docked, no commands available
        return;
    };
    let docked_system = docked_system;

    // F: FTL jump
    if keys.just_pressed(KeyCode::KeyF) {
        if ship.ftl_range <= 0.0 {
            info!("Ship {} has no FTL capability", ship.name);
            return;
        }
        if sel_system == docked_system {
            info!("Select a different system as FTL target");
            return;
        }

        let Ok((_, target_star, target_pos)) = stars.get(sel_system) else { return };
        let Ok((_, _, dock_pos)) = stars.get(docked_system) else { return };

        if !target_star.surveyed {
            info!("Cannot FTL to unsurveyed system {}", target_star.name);
            return;
        }

        let dist = physics::distance_ly(dock_pos, target_pos);
        if dist > ship.ftl_range {
            info!(
                "Target {} is {:.1} ly away, FTL range is {:.1} ly",
                target_star.name, dist, ship.ftl_range,
            );
            return;
        }

        // Execute FTL
        let travel_time = physics::sublight_travel_sexadies(dist, 10.0); // FTL is ~10x light speed
        let travel_time = travel_time.max(1);
        let Ok((mut _ship_mut, mut state_mut)) = ship_writer.get_mut(ship_entity) else { return };
        *state_mut = ShipState::InFTL {
            origin_system: docked_system,
            destination_system: sel_system,
            departed_at: clock.elapsed,
            arrival_at: clock.elapsed + travel_time,
        };
        info!(
            "Ship {} jumping to {} (ETA: {} sd)",
            _ship_mut.name, target_star.name, travel_time,
        );
        selected_ship.0 = None;
        return;
    }

    // M: Sub-light move
    if keys.just_pressed(KeyCode::KeyM) {
        if sel_system == docked_system {
            info!("Select a different system as move target");
            return;
        }

        let Ok((_, target_star, target_pos)) = stars.get(sel_system) else { return };
        let Ok((_, _, dock_pos)) = stars.get(docked_system) else { return };

        let dist = physics::distance_ly(dock_pos, target_pos);
        let travel_time = physics::sublight_travel_sexadies(dist, ship.sublight_speed);

        let Ok((mut _ship_mut, mut state_mut)) = ship_writer.get_mut(ship_entity) else { return };
        *state_mut = ShipState::SubLight {
            origin: dock_pos.as_array(),
            destination: target_pos.as_array(),
            target_system: Some(sel_system),
            departed_at: clock.elapsed,
            arrival_at: clock.elapsed + travel_time,
        };
        info!(
            "Ship {} departing for {} at {:.0}% c (ETA: {} sd)",
            _ship_mut.name, target_star.name, _ship_mut.sublight_speed * 100.0, travel_time,
        );
        selected_ship.0 = None;
        return;
    }

    // V: Survey (Explorer only)
    if keys.just_pressed(KeyCode::KeyV) {
        if ship.ship_type != ShipType::Explorer {
            info!("Only Explorers can survey systems");
            return;
        }
        if sel_system == docked_system {
            info!("Select a target system to survey");
            return;
        }

        let Ok((_, target_star, target_pos)) = stars.get(sel_system) else { return };
        let Ok((_, _, dock_pos)) = stars.get(docked_system) else { return };

        if target_star.surveyed {
            info!("System {} is already surveyed", target_star.name);
            return;
        }

        let dist = physics::distance_ly(dock_pos, target_pos);
        // Survey time: light delay there and back + base survey time (5 sd)
        let survey_time = physics::light_delay_sexadies(dist) * 2 + 5;

        let Ok((mut _ship_mut, mut state_mut)) = ship_writer.get_mut(ship_entity) else { return };
        *state_mut = ShipState::Surveying {
            target_system: sel_system,
            started_at: clock.elapsed,
            completes_at: clock.elapsed + survey_time,
        };
        info!(
            "Ship {} surveying {} (ETA: {} sd)",
            _ship_mut.name, target_star.name, survey_time,
        );
        selected_ship.0 = None;
    }
}

// #15: Build command handling
fn handle_build_commands(
    keys: Res<ButtonInput<KeyCode>>,
    player_q: Query<&StationedAt, With<Player>>,
    mut colonies: Query<(&Colony, &mut BuildQueue)>,
) {
    let ship_request = if keys.just_pressed(KeyCode::F1) {
        Some(("Explorer", 200.0, 100.0))
    } else if keys.just_pressed(KeyCode::F2) {
        Some(("Colony Ship", 500.0, 300.0))
    } else if keys.just_pressed(KeyCode::F3) {
        Some(("Courier", 100.0, 50.0))
    } else {
        None
    };

    let Some((ship_name, minerals_cost, energy_cost)) = ship_request else {
        return;
    };

    let Ok(stationed) = player_q.single() else {
        return;
    };

    for (colony, mut build_queue) in &mut colonies {
        if colony.system == stationed.system {
            build_queue.queue.push(BuildOrder {
                ship_type_name: ship_name.to_string(),
                minerals_cost,
                minerals_invested: 0.0,
                energy_cost,
                energy_invested: 0.0,
            });
            info!("Build order added: {}", ship_name);
            return;
        }
    }
}
