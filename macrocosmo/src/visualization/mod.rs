pub mod territory;
mod camera;
mod ships;
mod stars;

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

/// #229: Pending deploy request set by the ship panel "Deploy" button.
/// When `Some`, the next star click is interpreted as "deploy at this star's
/// coordinates" and pushes a `QueuedCommand::DeployDeliverable` onto the ship's
/// CommandQueue. Escape cancels. Only used for V1 (star-coordinate deploys;
/// arbitrary deep-space coordinates are a future issue).
#[derive(Clone, Copy, Debug)]
pub struct DeployPending {
    pub ship: Entity,
    pub item_index: usize,
}

#[derive(Resource, Default)]
pub struct DeployMode(pub Option<DeployPending>);

pub struct VisualizationPlugin;

impl Plugin for VisualizationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(territory::TerritoryPlugin)
        .insert_resource(GalaxyView {
            scale: 7.0,
        })
        .insert_resource(SelectedSystem::default())
        .insert_resource(SelectedShip::default())
        .insert_resource(SelectedPlanet::default())
        .insert_resource(ContextMenu::default())
        .insert_resource(DeployMode::default())
        .insert_resource(OutlineExpandedSystems::default())
        .add_systems(Startup, camera::setup_camera)
        .add_systems(PostStartup, (stars::spawn_star_visuals, camera::center_camera_on_capital))
        .add_systems(Update, (
            click_select_system,
            camera::camera_controls,
            stars::update_star_colors,
            stars::draw_galaxy_overlay,
            ships::draw_ships,
            stars::draw_deep_space_structures,
            stars::draw_forbidden_regions,
        ));
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
            // #185: Loitering ships are selectable in deep space.
            ShipState::Loitering { position } => {
                Vec2::new(position[0] as f32 * view.scale, position[1] as f32 * view.scale)
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

    // #229: Deploy mode — if the user has just clicked "Deploy" on a cargo
    // item and then clicks on a star, interpret that as "deploy at this
    // star's coordinates". V1 restriction: deploys are always to star
    // coordinates; arbitrary deep-space points are a future issue.
    if let Some(pending) = deploy_mode.0 {
        if let Some((star_entity, _)) = best_star {
            if let Ok(star_pos) = star_positions.get(star_entity) {
                if let Ok(mut queue) = command_queues.get_mut(pending.ship) {
                    let target_pos = star_pos.as_array();
                    // Use direct push (no predicted-position tracker) because
                    // we already know the exact coordinate from the star.
                    queue.commands.push(QueuedCommand::DeployDeliverable {
                        position: target_pos,
                        item_index: pending.item_index,
                    });
                    queue.predicted_position = target_pos;
                    queue.predicted_system = None;
                    info!(
                        "Deploy queued: ship {:?} -> cargo idx {} at star {:?}",
                        pending.ship, pending.item_index, star_entity,
                    );
                }
            }
        }
        // Whether or not the click landed on a star, leave deploy mode.
        // Clicking empty space simply cancels the pending deploy.
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
