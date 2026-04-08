use bevy::prelude::*;
use bevy::input::mouse::AccumulatedMouseScroll;

use crate::colony::{Colony, Production};
use crate::components::Position;
use crate::galaxy::{ObscuredByGas, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::time_system::{GameClock, GameSpeed};

pub struct VisualizationPlugin;

impl Plugin for VisualizationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GalaxyView {
            scale: 5.0,
        })
        .insert_resource(SelectedSystem::default())
        .add_systems(Startup, setup_camera)
        .add_systems(PostStartup, spawn_star_visuals)
        .add_systems(Update, (
            click_select_system,
            camera_controls,
            update_star_colors,
            draw_galaxy_overlay,
            update_hud,
            update_info_panel,
        ));
    }
}

#[derive(Resource, Default)]
struct SelectedSystem(Option<Entity>);

#[derive(Resource)]
struct GalaxyView {
    scale: f32,
}

#[derive(Component)]
struct StarVisual {
    system_entity: Entity,
}

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct InfoPanel;

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);

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

        let color = star_color(star, obscured.is_some());
        let size = if star.is_capital { 8.0 } else { 5.0 };

        commands.spawn((
            StarVisual { system_entity: entity },
            Sprite {
                color,
                custom_size: Some(Vec2::splat(size)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.0),
        ));

        if star.is_capital || star.surveyed {
            commands.spawn((
                StarVisual { system_entity: entity },
                Text2d::new(&star.name),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.7)),
                Transform::from_xyz(x, y + 10.0, 1.0),
            ));
        }
    }
}

fn star_color(star: &StarSystem, obscured: bool) -> Color {
    if obscured {
        Color::srgba(0.3, 0.3, 0.3, 0.2)
    } else if star.is_capital {
        Color::srgb(1.0, 0.84, 0.0)
    } else if star.colonized {
        Color::srgb(0.2, 0.8, 0.2)
    } else if star.surveyed {
        Color::srgb(0.4, 0.6, 1.0)
    } else {
        Color::srgba(0.6, 0.6, 0.6, 0.5)
    }
}

fn update_star_colors(
    stars: Query<(Entity, &StarSystem, Option<&ObscuredByGas>)>,
    mut visuals: Query<(&StarVisual, &mut Sprite)>,
) {
    for (vis, mut sprite) in &mut visuals {
        if let Ok((_, star, obscured)) = stars.get(vis.system_entity) {
            sprite.color = star_color(star, obscured.is_some());
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

    let pulse = (clock.as_years_f64() as f32 * 3.0).sin() * 0.3 + 0.7;
    gizmos.circle_2d(
        Vec2::new(px, py),
        12.0,
        Color::srgba(1.0, 0.84, 0.0, pulse),
    );

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
                14.0,
                Color::srgba(0.0, 1.0, 1.0, sel_pulse),
            );
        }
    }
}

fn update_hud(
    clock: Res<GameClock>,
    speed: Res<GameSpeed>,
    player_q: Query<&StationedAt, With<Player>>,
    stars: Query<&StarSystem>,
    mut hud: Query<&mut Text, With<HudText>>,
) {
    let Ok(mut text) = hud.single_mut() else {
        return;
    };

    let speed_str = if speed.sexadies_per_second <= 0.0 {
        "PAUSED".to_string()
    } else {
        format!("x{:.0} sd/s", speed.sexadies_per_second)
    };

    let location = if let Ok(stationed) = player_q.single() {
        if let Ok(star) = stars.get(stationed.system) {
            star.name.clone()
        } else {
            "Unknown".to_string()
        }
    } else {
        "In transit".to_string()
    };

    **text = format!(
        "Year {} Month {} Sexadie {} [{}]\nLocation: {}\n\nWASD: Pan | Scroll: Zoom | Space: Pause\n+/-: Speed | I: System Info | Home: Reset View\nClick: Select system | Esc: Deselect",
        clock.year(),
        clock.month(),
        clock.sexadie(),
        speed_str,
        location,
    );
}

fn click_select_system(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    stars: Query<(Entity, &Position, Option<&ObscuredByGas>), With<StarSystem>>,
    view: Res<GalaxyView>,
    mut selected: ResMut<SelectedSystem>,
) {
    // Deselect on Escape
    if keys.just_pressed(KeyCode::Escape) {
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

    selected.0 = best.map(|(e, _)| e);
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

    // Knowledge age
    if let Some(age) = knowledge.info_age(selected_entity, clock.elapsed) {
        info.push_str(&format!("\nInformation age: {} sexadies\n", age));
    }

    **text = info;
}
