mod camera;
mod ships;
mod stars;
pub mod territory;

pub use camera::camera_controls;

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use crate::components::Position;
use crate::galaxy::{ObscuredByGas, StarSystem};
use crate::player::Player;
use crate::ship::{CommandQueue, QueuedCommand, Ship, ShipState};
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

/// #229 / #240: Pending deploy request set by the ship panel "Deploy" button.
/// When `Some`, the next map click is interpreted as a deploy target and pushes
/// a `QueuedCommand::DeployDeliverable` onto the ship's CommandQueue. Clicks
/// that land close to a star snap to the star's coordinates; clicks on empty
/// space deploy at the cursor's world position (z = 0). Escape cancels.
#[derive(Clone, Copy, Debug)]
pub struct DeployPending {
    pub ship: Entity,
    pub item_index: usize,
}

/// Pixel radius around a star within which a deploy click is snapped to the
/// star's coordinates. Shared between the click resolver and the preview gizmo
/// so the two always agree on snap behavior.
pub const DEPLOY_STAR_SNAP_RADIUS_PX: f32 = 15.0;

/// Pure helper used by `click_select_system` and tests to decide where a
/// deploy click lands. If `snapped_star_world` is `Some`, the deploy snaps to
/// that star's galactic coordinates (z unchanged). Otherwise the cursor's
/// render-world position is divided by `view_scale` to recover the galactic
/// coordinate before being stored (see #254).
///
/// Why `view_scale`: every star / ship is drawn at `Position × view.scale`
/// (`visualization::stars::spawn_star_visuals`). `camera.viewport_to_world_2d`
/// returns render-world space, so treating the cursor position as a galactic
/// coordinate directly over-counts the scale factor, and the deliverable ends
/// up `view.scale`× farther than the player clicked.
pub fn resolve_deploy_target(
    snapped_star_world: Option<[f64; 3]>,
    cursor_world: Vec2,
    view_scale: f32,
) -> [f64; 3] {
    match snapped_star_world {
        Some(p) => p,
        None => {
            // Guard against a pathological scale of zero; callers always have
            // a positive scale in practice (see `GalaxyView::scale` default).
            let scale = if view_scale.abs() < f32::EPSILON {
                1.0
            } else {
                view_scale
            };
            [
                (cursor_world.x / scale) as f64,
                (cursor_world.y / scale) as f64,
                0.0,
            ]
        }
    }
}

#[derive(Resource, Default)]
pub struct DeployMode(pub Option<DeployPending>);

pub struct VisualizationPlugin;

impl Plugin for VisualizationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(territory::TerritoryPlugin)
            .insert_resource(GalaxyView { scale: 15.0 })
            .insert_resource(SelectedSystem::default())
            .insert_resource(SelectedShip::default())
            .insert_resource(SelectedPlanet::default())
            .insert_resource(ContextMenu::default())
            .insert_resource(DeployMode::default())
            .insert_resource(OutlineExpandedSystems::default())
            .add_systems(Startup, camera::setup_camera)
            .add_systems(
                PostStartup,
                (stars::spawn_star_visuals, camera::center_camera_on_capital),
            )
            .add_systems(
                Update,
                (
                    click_select_system,
                    camera::camera_controls,
                    stars::update_star_colors,
                    stars::draw_galaxy_overlay,
                    ships::draw_ships,
                    stars::draw_deep_space_structures,
                    stars::draw_forbidden_regions,
                    draw_deploy_preview_gizmo,
                ),
            );
    }
}

#[derive(Resource, Default)]
pub struct SelectedSystem(pub Option<Entity>);

#[derive(Resource, Default)]
pub struct SelectedShip(pub Option<Entity>);

#[derive(Resource, Default)]
pub struct SelectedPlanet(pub Option<Entity>);

/// Tracks which systems are expanded in the outline panel.
#[derive(Resource)]
pub struct OutlineExpandedSystems(pub std::collections::HashSet<Entity>);

impl Default for OutlineExpandedSystems {
    fn default() -> Self {
        Self(std::collections::HashSet::new())
    }
}

#[derive(Resource)]
pub struct GalaxyView {
    pub scale: f32,
}

/// Resource set each frame by the UI system to indicate egui is consuming pointer input.
/// Camera controls check this to avoid scroll-zoom when the pointer is over a UI panel.
#[derive(Resource, Default)]
pub struct EguiWantsPointer(pub bool);

#[allow(clippy::too_many_arguments)]
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
    mut deploy_mode: ResMut<DeployMode>,
    mut command_queues: Query<&mut CommandQueue>,
    mut egui_contexts: EguiContexts,
) {
    // Escape handling — cancel deploy mode first if active, otherwise fall
    // through to the existing ship / system deselection logic.
    if keys.just_pressed(KeyCode::Escape) {
        if deploy_mode.0.is_some() {
            deploy_mode.0 = None;
            return;
        }
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
            ShipState::Settling { system, .. }
            | ShipState::Surveying {
                target_system: system,
                ..
            } => {
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
                let (Ok(origin_pos), Ok(dest_pos)) = (
                    star_positions.get(*origin_system),
                    star_positions.get(*destination_system),
                ) else {
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
            // #185: Loitering ships are selectable in deep space.
            ShipState::Loitering { position } => Vec2::new(
                position[0] as f32 * view.scale,
                position[1] as f32 * view.scale,
            ),
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
    let click_radius = DEPLOY_STAR_SNAP_RADIUS_PX;
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

    // #229 / #240: Deploy mode — if the user has just clicked "Deploy" on a
    // cargo item and then clicks on the map, push a DeployDeliverable command.
    // Clicks close to a star snap to that star's coordinates; clicks on empty
    // space deploy at the cursor's world position (z = 0).
    if let Some(pending) = deploy_mode.0 {
        let snapped_star_world = best_star
            .and_then(|(star_entity, _)| star_positions.get(star_entity).ok())
            .map(|p| p.as_array());
        let target_pos = resolve_deploy_target(snapped_star_world, world_pos, view.scale);
        if let Ok(mut queue) = command_queues.get_mut(pending.ship) {
            // Use direct push (bypassing `CommandQueue::push`) because we
            // already know the exact coordinate and don't need the system
            // position lookup helper.
            queue.commands.push(QueuedCommand::DeployDeliverable {
                position: target_pos,
                item_index: pending.item_index,
            });
            queue.predicted_position = target_pos;
            queue.predicted_system = None;
            if snapped_star_world.is_some() {
                info!(
                    "Deploy queued: ship {:?} -> cargo idx {} at star {:?}",
                    pending.ship,
                    pending.item_index,
                    best_star.map(|(e, _)| e),
                );
            } else {
                info!(
                    "Deploy queued: ship {:?} -> cargo idx {} at deep space {:?}",
                    pending.ship, pending.item_index, target_pos,
                );
            }
        }
        deploy_mode.0 = None;
        return;
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

/// #240: Preview marker drawn while a deploy is pending. The marker tracks the
/// cursor and switches between two visuals:
/// - **Deep space (no snap):** an orange cross (+) at the cursor indicating a
///   free-form deploy.
/// - **Star snap:** a dashed orange ring around the nearest star within the
///   snap radius, signalling that a click here will deploy at the star's
///   coordinates (same radius `DEPLOY_STAR_SNAP_RADIUS_PX` the click handler
///   uses).
pub fn draw_deploy_preview_gizmo(
    deploy_mode: Res<DeployMode>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    stars: Query<(&StarSystem, &Position, Option<&ObscuredByGas>)>,
    view: Res<GalaxyView>,
    mut gizmos: Gizmos,
) {
    if deploy_mode.0.is_none() {
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
        return;
    };

    // Find the nearest visible star within the snap radius.
    let mut best: Option<(Vec2, f32)> = None;
    for (_star, pos, obscured) in &stars {
        if obscured.is_some() {
            continue;
        }
        let star_px = Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale);
        let dist = world_pos.distance(star_px);
        if dist < DEPLOY_STAR_SNAP_RADIUS_PX && best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((star_px, dist));
        }
    }

    let orange = Color::srgba(1.0, 0.6, 0.2, 0.9);
    let orange_dim = Color::srgba(1.0, 0.6, 0.2, 0.45);

    if let Some((star_px, _)) = best {
        // Snap indicator: ring around the star plus a faint connector from
        // cursor to show the snap is active.
        gizmos.circle_2d(star_px, DEPLOY_STAR_SNAP_RADIUS_PX + 2.0, orange);
        gizmos.circle_2d(star_px, DEPLOY_STAR_SNAP_RADIUS_PX - 1.0, orange_dim);
    } else {
        // Free-form cross (+) at the cursor.
        let arm = 8.0;
        gizmos.line_2d(
            Vec2::new(world_pos.x - arm, world_pos.y),
            Vec2::new(world_pos.x + arm, world_pos.y),
            orange,
        );
        gizmos.line_2d(
            Vec2::new(world_pos.x, world_pos.y - arm),
            Vec2::new(world_pos.x, world_pos.y + arm),
            orange,
        );
        // Small center dot for precision.
        gizmos.circle_2d(world_pos, 1.5, orange);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deploy_at_arbitrary_coordinate() {
        // #240/#254: Empty-space click returns the galactic coordinate
        // (world_pos / view.scale) with z = 0.
        let world = Vec2::new(12.5, -4.25);
        let target = resolve_deploy_target(None, world, 1.0);
        assert_eq!(target, [12.5, -4.25, 0.0]);
    }

    #[test]
    fn test_deploy_star_click_still_snaps() {
        // #240: Star click keeps the V1 behavior — snaps to the star's
        // galactic coordinates (including the star's z) regardless of the
        // cursor's world position or the current view scale.
        let star = [7.0, 3.0, 0.5];
        let cursor = Vec2::new(99.0, -99.0);
        let target = resolve_deploy_target(Some(star), cursor, 7.0);
        assert_eq!(target, star);
    }

    #[test]
    fn test_deploy_target_z_is_zero_for_deep_space() {
        // Explicitly document the 2D-projection invariant: deep-space deploys
        // always land on z = 0 even if the cursor would otherwise carry a
        // different depth.
        let target = resolve_deploy_target(None, Vec2::new(0.0, 0.0), 1.0);
        assert_eq!(target[2], 0.0);
    }

    #[test]
    fn test_deploy_target_divides_cursor_by_view_scale() {
        // #254: Cursor is in render-world coordinates (galactic × view.scale);
        // the deploy must store the galactic coordinate. With scale = 7 and
        // cursor at (70, 35), the deploy target is (10, 5, 0).
        let cursor = Vec2::new(70.0, 35.0);
        let target = resolve_deploy_target(None, cursor, 7.0);
        assert_eq!(target, [10.0, 5.0, 0.0]);
    }

    #[test]
    fn test_deploy_target_scale_zero_falls_back_to_identity() {
        // Defensive: a degenerate scale must not produce NaN / Inf. The helper
        // treats near-zero as 1.0 so the deploy still lands somewhere sensible.
        let cursor = Vec2::new(3.0, 4.0);
        let target = resolve_deploy_target(None, cursor, 0.0);
        assert_eq!(target, [3.0, 4.0, 0.0]);
    }

    /// #364 regression guard: `bevy_gizmos` enables only the `Gizmos`
    /// SystemParam / API surface; the actual GPU rendering pipeline lives in
    /// `bevy_gizmos_render::GizmoRenderPlugin`. If the `bevy_gizmos_render`
    /// feature is dropped from `Cargo.toml`, gizmo draw calls (ships, deploy
    /// preview, system overlays, etc.) become silent no-ops and the galaxy
    /// map appears blank. Referencing the type here makes the build fail
    /// instead of silently producing an invisible ship layer.
    #[test]
    fn bevy_gizmos_render_feature_is_enabled() {
        let _: bevy::gizmos_render::GizmoRenderPlugin = bevy::gizmos_render::GizmoRenderPlugin;
    }
}
