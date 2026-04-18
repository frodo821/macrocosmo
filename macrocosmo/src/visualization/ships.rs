use std::collections::HashMap;

use bevy::prelude::*;

use super::{GalaxyView, SelectedShip};
use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship::{CommandQueue, QueuedCommand, Ship, ShipState, ShipStats};
use crate::time_system::GameClock;

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

/// Returns true when a ship is immobile (station / infrastructure core).
fn is_station(ship: &Ship) -> bool {
    ship.sublight_speed <= 0.0 && ship.ftl_range <= 0.0
}

/// Returns true when a ship acts as a harbour (harbour_capacity > 0).
fn is_harbour(stats: Option<&ShipStats>) -> bool {
    stats
        .map(|s| s.harbour_capacity.cached().raw() > 0)
        .unwrap_or(false)
}

/// Per-ship metadata stashed while grouping docked ships by system.
struct DockedShipInfo {
    design_id: String,
    is_station: bool,
    is_harbour: bool,
}

pub fn draw_ships(
    mut gizmos: Gizmos,
    ships: Query<(
        Entity,
        &Ship,
        &ShipState,
        Option<&CommandQueue>,
        Option<&ShipStats>,
    )>,
    stars: Query<&Position, With<StarSystem>>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
    selected_ship: Res<SelectedShip>,
) {
    // Group docked ships by system so we can offset them.
    let mut docked_counts: HashMap<Entity, Vec<DockedShipInfo>> = HashMap::new();
    // Also count ships per system for badge display.
    // Immobile ships (stations) are excluded from the badge count.
    let mut system_ship_counts: HashMap<Entity, u32> = HashMap::new();

    for (_entity, ship, state, _queue, stats) in &ships {
        let station = is_station(ship);
        let harbour = is_harbour(stats);
        match state {
            ShipState::InSystem { system } => {
                docked_counts
                    .entry(*system)
                    .or_default()
                    .push(DockedShipInfo {
                        design_id: ship.design_id.clone(),
                        is_station: station,
                        is_harbour: harbour,
                    });
                if !station {
                    *system_ship_counts.entry(*system).or_insert(0) += 1;
                }
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
                    gizmos.circle_2d(Vec2::new(sx, sy), 6.0, Color::srgba(r, g, b, pulse));
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
                    gizmos.circle_2d(Vec2::new(sx, sy), 6.0, Color::srgba(r, g, b, pulse));

                    // Ship marker
                    gizmos.circle_2d(Vec2::new(sx, sy), 3.5, Color::srgb(r, g, b));
                }
            }
            ShipState::Refitting { system, .. } => {
                // Refitting ships are docked — show them at the system
                docked_counts
                    .entry(*system)
                    .or_default()
                    .push(DockedShipInfo {
                        design_id: ship.design_id.clone(),
                        is_station: station,
                        is_harbour: harbour,
                    });
                if !station {
                    *system_ship_counts.entry(*system).or_insert(0) += 1;
                }
            }
            // #185: Loitering ships are drawn as a small marker at their deep-space coordinate.
            ShipState::Loitering { position } => {
                let cx = position[0] as f32 * view.scale;
                let cy = position[1] as f32 * view.scale;
                let (r, g, b) = ship_color_rgb(&ship.design_id);
                gizmos.circle_2d(Vec2::new(cx, cy), 3.0, Color::srgb(r, g, b));
                // Faint outer halo to distinguish "loitering" from in-transit.
                gizmos.circle_2d(Vec2::new(cx, cy), 5.5, Color::srgba(r, g, b, 0.25));
            }
            // #217: Scouting — display at the target system like docked.
            ShipState::Scouting { target_system, .. } => {
                docked_counts
                    .entry(*target_system)
                    .or_default()
                    .push(DockedShipInfo {
                        design_id: ship.design_id.clone(),
                        is_station: station,
                        is_harbour: harbour,
                    });
                if !station {
                    *system_ship_counts.entry(*target_system).or_insert(0) += 1;
                }
            }
        }
    }

    // Draw docked ships offset around their system.
    for (system_entity, ship_infos) in &docked_counts {
        let Ok(sys_pos) = stars.get(*system_entity) else {
            continue;
        };
        let sx = sys_pos.x as f32 * view.scale;
        let sy = sys_pos.y as f32 * view.scale;
        let count = ship_infos.len();

        for (i, info) in ship_infos.iter().enumerate() {
            let angle = if count == 1 {
                0.0
            } else {
                std::f32::consts::TAU * (i as f32) / (count as f32)
            };
            let offset_radius = if info.is_station { 10.0 } else { 8.0 };
            let ox = sx + angle.cos() * offset_radius;
            let oy = sy + angle.sin() * offset_radius;

            if info.is_station || info.is_harbour {
                // Station / harbour ships: gold diamond, larger
                let gold = Color::srgb(1.0, 0.85, 0.2);
                let radius = 5.5;
                let center = Vec2::new(ox, oy);
                // Draw a diamond shape (rotated square)
                let top = center + Vec2::new(0.0, radius);
                let right = center + Vec2::new(radius, 0.0);
                let bottom = center + Vec2::new(0.0, -radius);
                let left = center + Vec2::new(-radius, 0.0);
                gizmos.line_2d(top, right, gold);
                gizmos.line_2d(right, bottom, gold);
                gizmos.line_2d(bottom, left, gold);
                gizmos.line_2d(left, top, gold);
            } else {
                let color = ship_color(&info.design_id);
                gizmos.circle_2d(Vec2::new(ox, oy), 3.0, color);
            }
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
        if let Ok((_entity, ship, state, Some(queue), _stats)) = ships.get(selected_entity) {
            if !queue.commands.is_empty() {
                // Determine the ship's current screen position from its state
                let current_pos = match state {
                    ShipState::InSystem { system } => stars
                        .get(*system)
                        .ok()
                        .map(|pos| Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)),
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
                    ShipState::Settling { system, .. } => stars
                        .get(*system)
                        .ok()
                        .map(|pos| Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)),
                    ShipState::Surveying { target_system, .. } => stars
                        .get(*target_system)
                        .ok()
                        .map(|pos| Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)),
                    ShipState::Refitting { system, .. } => stars
                        .get(*system)
                        .ok()
                        .map(|pos| Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)),
                    // #185: Loitering ship's current screen pos for queue overlay.
                    ShipState::Loitering { position } => Some(Vec2::new(
                        position[0] as f32 * view.scale,
                        position[1] as f32 * view.scale,
                    )),
                    // #217: Scouting ships render at the target system.
                    ShipState::Scouting { target_system, .. } => stars
                        .get(*target_system)
                        .ok()
                        .map(|pos| Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)),
                };

                if let Some(mut prev_pos) = current_pos {
                    let (r, g, b) = ship_color_rgb(&ship.design_id);

                    for cmd in &queue.commands {
                        let target_screen = match cmd {
                            QueuedCommand::MoveTo { system, .. }
                            | QueuedCommand::Survey { system, .. }
                            | QueuedCommand::Colonize { system, .. }
                            | QueuedCommand::LoadDeliverable { system, .. } => {
                                let Ok(target_pos) = stars.get(*system) else {
                                    continue;
                                };
                                Vec2::new(
                                    target_pos.x as f32 * view.scale,
                                    target_pos.y as f32 * view.scale,
                                )
                            }
                            // #217: Scout targets a star system like MoveTo.
                            QueuedCommand::Scout { target_system, .. } => {
                                let Ok(target_pos) = stars.get(*target_system) else {
                                    continue;
                                };
                                Vec2::new(
                                    target_pos.x as f32 * view.scale,
                                    target_pos.y as f32 * view.scale,
                                )
                            }
                            // #185: Loitering target — render directly from coordinates.
                            QueuedCommand::MoveToCoordinates { target }
                            | QueuedCommand::DeployDeliverable {
                                position: target, ..
                            } => Vec2::new(
                                target[0] as f32 * view.scale,
                                target[1] as f32 * view.scale,
                            ),
                            // #223: In-place actions draw no destination marker.
                            QueuedCommand::TransferToStructure { .. }
                            | QueuedCommand::LoadFromScrapyard { .. } => {
                                continue;
                            }
                        };

                        // Dashed path line from previous position to target
                        draw_dashed_line(
                            &mut gizmos,
                            prev_pos,
                            target_screen,
                            Color::srgba(r, g, b, 0.3),
                        );

                        // Command-specific markers
                        match cmd {
                            QueuedCommand::MoveTo { .. }
                            | QueuedCommand::MoveToCoordinates { .. } => {
                                gizmos.circle_2d(target_screen, 4.0, Color::srgba(r, g, b, 0.5));
                            }
                            // #217: Scout marker — magenta accent to distinguish from Survey.
                            QueuedCommand::Scout { .. } => {
                                gizmos.circle_2d(
                                    target_screen,
                                    6.0,
                                    Color::srgba(1.0, 0.3, 1.0, 0.4),
                                );
                                gizmos.circle_2d(
                                    target_screen,
                                    3.0,
                                    Color::srgba(1.0, 0.3, 1.0, 0.6),
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
                            // #223: Deliverable deploy marker — orange diamond-ish ring.
                            QueuedCommand::DeployDeliverable { .. } => {
                                gizmos.circle_2d(
                                    target_screen,
                                    5.0,
                                    Color::srgba(1.0, 0.6, 0.2, 0.6),
                                );
                            }
                            QueuedCommand::LoadDeliverable { .. } => {
                                gizmos.circle_2d(
                                    target_screen,
                                    4.0,
                                    Color::srgba(0.2, 0.8, 1.0, 0.5),
                                );
                            }
                            // TransferToStructure / LoadFromScrapyard continue'd above.
                            QueuedCommand::TransferToStructure { .. }
                            | QueuedCommand::LoadFromScrapyard { .. } => {}
                        }

                        prev_pos = target_screen;
                    }
                }
            }
        }
    }
}
