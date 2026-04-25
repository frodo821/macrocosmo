mod camera;
mod ships;
mod stars;
pub mod territory;

pub use camera::camera_controls;
pub use stars::{cleanup_star_visuals, spawn_star_visuals};

use std::time::Instant;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::EguiContexts;

use crate::components::Position;
use crate::deep_space::DeepSpaceStructure;
use crate::galaxy::StarSystem;
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
            .insert_resource(SelectedShips::default())
            .insert_resource(SelectedPlanet::default())
            .insert_resource(ContextMenu::default())
            .insert_resource(DeployMode::default())
            .insert_resource(OutlineExpandedSystems::default())
            .insert_resource(CycleSelection::default())
            .add_systems(Startup, camera::setup_camera)
            // #439 Phase 3: star visuals and camera centering read
            // constructed StarSystem entities, so they run after the
            // world build completes — OnEnter(InGame).
            .add_systems(
                OnEnter(crate::game_state::GameState::InGame),
                (stars::spawn_star_visuals, camera::center_camera_on_capital),
            )
            // #439 Phase 4: despawn star sprites / labels on scene exit
            // so a fresh scene doesn't stack duplicate visuals. Registered
            // here next to `spawn_star_visuals` so the lifecycle pair is
            // obvious — the centralised `GameSetupPlugin` teardown only
            // handles SaveableMarker-tagged entities.
            .add_systems(
                OnExit(crate::game_state::GameState::InGame),
                stars::cleanup_star_visuals,
            )
            .add_systems(
                Update,
                (
                    click_select_system,
                    sync_selected_ship_from_ships.after(click_select_system),
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

/// #407: Multi-ship selection resource. Maintains an ordered list of selected
/// ship entities. `SelectedShip` is kept in sync with `primary()` via
/// `sync_selected_ship_from_ships`.
#[derive(Resource, Default, Debug, Clone)]
pub struct SelectedShips(pub Vec<Entity>);

impl SelectedShips {
    /// The first (primary) selected ship, if any.
    pub fn primary(&self) -> Option<Entity> {
        self.0.first().copied()
    }

    /// Whether `e` is in the selection set.
    pub fn contains(&self, e: Entity) -> bool {
        self.0.contains(&e)
    }

    /// Add if absent, remove if present.
    pub fn toggle(&mut self, e: Entity) {
        if let Some(idx) = self.0.iter().position(|x| *x == e) {
            self.0.remove(idx);
        } else {
            self.0.push(e);
        }
    }

    /// Replace selection with a single entity.
    pub fn set_single(&mut self, e: Entity) {
        self.0 = vec![e];
    }

    /// Clear selection.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entity> {
        self.0.iter()
    }
}

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

/// Distinguishes candidate types in `CycleSelection` for proper selection dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleKind {
    Ship,
    StarSystem,
    DeepSpaceStructure,
}

/// The priority for cycle ordering. Lower value = selected first on single click.
impl CycleKind {
    fn priority(self) -> u8 {
        match self {
            CycleKind::Ship => 0,
            CycleKind::StarSystem => 1,
            CycleKind::DeepSpaceStructure => 2,
        }
    }
}

/// A selectable entity near the click position.
#[derive(Debug, Clone, Copy)]
pub struct CycleCandidate {
    pub entity: Entity,
    pub kind: CycleKind,
    pub distance: f32,
}

/// #368: Tracks double-click state for cycling through overlapping objects.
///
/// On single click, the nearest/highest-priority candidate is selected (normal
/// behavior). On double-click (same position within `DOUBLE_CLICK_TIME` and
/// `DOUBLE_CLICK_RADIUS_PX`), the next candidate in the cycle is selected.
#[derive(Resource)]
pub struct CycleSelection {
    /// All candidates near the last click position, sorted by cycle order.
    pub candidates: Vec<CycleCandidate>,
    /// Index into `candidates` of the currently selected candidate.
    pub current_index: usize,
    /// World-space position of the last click (for proximity check).
    pub click_world_pos: Vec2,
    /// Wall-clock time of the last click (for double-click detection).
    pub last_click_time: Instant,
}

impl Default for CycleSelection {
    fn default() -> Self {
        Self {
            candidates: Vec::new(),
            current_index: 0,
            click_world_pos: Vec2::ZERO,
            last_click_time: Instant::now(),
        }
    }
}

/// Maximum time between clicks for double-click detection (seconds).
const DOUBLE_CLICK_TIME: f32 = 0.4;
/// Maximum pixel distance between clicks for double-click detection.
const DOUBLE_CLICK_RADIUS_PX: f32 = 5.0;

#[derive(Resource)]
pub struct GalaxyView {
    pub scale: f32,
}

/// Resource set each frame by the UI system to indicate egui is consuming pointer input.
/// Camera controls check this to avoid scroll-zoom when the pointer is over a UI panel.
#[derive(Resource, Default)]
pub struct EguiWantsPointer(pub bool);

/// #368: Bundled selection state to keep `click_select_system` under Bevy's
/// 16-parameter limit.
#[derive(SystemParam)]
pub struct SelectionState<'w> {
    pub selected: ResMut<'w, SelectedSystem>,
    pub selected_ship: ResMut<'w, SelectedShip>,
    pub selected_ships: ResMut<'w, SelectedShips>,
    pub context_menu: ResMut<'w, ContextMenu>,
    pub deploy_mode: ResMut<'w, DeployMode>,
    pub cycle: ResMut<'w, CycleSelection>,
}

/// Compute the pixel position of a ship given its current state.
/// Returns `None` for docked ships (selected via outline panel, not map click).
fn ship_pixel_position(
    state: &ShipState,
    star_positions: &Query<&Position, With<StarSystem>>,
    clock: &GameClock,
    view_scale: f32,
) -> Option<Vec2> {
    match state {
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
            let cx = (origin[0] + (destination[0] - origin[0]) * t) as f32 * view_scale;
            let cy = (origin[1] + (destination[1] - origin[1]) * t) as f32 * view_scale;
            Some(Vec2::new(cx, cy))
        }
        ShipState::Settling { system, .. }
        | ShipState::Surveying {
            target_system: system,
            ..
        } => {
            let sys_pos = star_positions.get(*system).ok()?;
            Some(Vec2::new(
                sys_pos.x as f32 * view_scale,
                sys_pos.y as f32 * view_scale,
            ))
        }
        ShipState::InFTL {
            origin_system,
            destination_system,
            departed_at,
            arrival_at,
        } => {
            let origin_pos = star_positions.get(*origin_system).ok()?;
            let dest_pos = star_positions.get(*destination_system).ok()?;
            let total = (*arrival_at - *departed_at) as f64;
            let elapsed = (clock.elapsed - *departed_at) as f64;
            let t = if total > 0.0 {
                (elapsed / total).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let cx = (origin_pos.x + (dest_pos.x - origin_pos.x) * t) as f32 * view_scale;
            let cy = (origin_pos.y + (dest_pos.y - origin_pos.y) * t) as f32 * view_scale;
            Some(Vec2::new(cx, cy))
        }
        ShipState::Loitering { position } => Some(Vec2::new(
            position[0] as f32 * view_scale,
            position[1] as f32 * view_scale,
        )),
        // Docked ships selected via outline panel
        _ => None,
    }
}

/// Collect all selectable candidates (ships, stars, deep space structures)
/// within click radius of `world_pos`.
fn collect_candidates(
    world_pos: Vec2,
    ship_q: &Query<(Entity, &Ship, &ShipState)>,
    stars: &Query<(Entity, &StarSystem, &Position)>,
    dss_q: &Query<(Entity, &Position), With<DeepSpaceStructure>>,
    star_positions: &Query<&Position, With<StarSystem>>,
    clock: &GameClock,
    view_scale: f32,
) -> Vec<CycleCandidate> {
    let click_radius = DEPLOY_STAR_SNAP_RADIUS_PX;
    let mut candidates = Vec::new();

    // Ships (in-transit / active, not docked)
    for (entity, _ship, state) in ship_q {
        if let Some(ship_px) = ship_pixel_position(state, star_positions, clock, view_scale) {
            let dist = world_pos.distance(ship_px);
            if dist < click_radius {
                candidates.push(CycleCandidate {
                    entity,
                    kind: CycleKind::Ship,
                    distance: dist,
                });
            }
        }
    }

    // Stars
    for (entity, _star, pos) in stars {
        let star_px = Vec2::new(pos.x as f32 * view_scale, pos.y as f32 * view_scale);
        let dist = world_pos.distance(star_px);
        if dist < click_radius {
            candidates.push(CycleCandidate {
                entity,
                kind: CycleKind::StarSystem,
                distance: dist,
            });
        }
    }

    // Deep space structures
    for (entity, pos) in dss_q {
        let dss_px = Vec2::new(pos.x as f32 * view_scale, pos.y as f32 * view_scale);
        let dist = world_pos.distance(dss_px);
        if dist < click_radius {
            candidates.push(CycleCandidate {
                entity,
                kind: CycleKind::DeepSpaceStructure,
                distance: dist,
            });
        }
    }

    // Sort by: type priority first, then distance within same type, then entity
    // id for determinism.
    candidates.sort_by(|a, b| {
        a.kind
            .priority()
            .cmp(&b.kind.priority())
            .then(
                a.distance
                    .partial_cmp(&b.distance)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.entity.cmp(&b.entity))
    });

    candidates
}

/// Apply selection for a `CycleCandidate`, updating the appropriate resources.
fn apply_candidate_selection(
    candidate: &CycleCandidate,
    sel: &mut SelectionState,
    shift_held: bool,
) {
    match candidate.kind {
        CycleKind::Ship => {
            if shift_held {
                sel.selected_ships.toggle(candidate.entity);
            } else {
                sel.selected_ships.set_single(candidate.entity);
            }
            sel.selected_ship.0 = sel.selected_ships.primary();
        }
        CycleKind::StarSystem => {
            sel.selected.0 = Some(candidate.entity);
            // Keep ship selected — star becomes command target (ship selection
            // persistence rule #5).
        }
        CycleKind::DeepSpaceStructure => {
            // Deep space structures use system selection for now; future may
            // have a dedicated SelectedStructure resource.
            sel.selected.0 = Some(candidate.entity);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn click_select_system(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    stars: Query<(Entity, &StarSystem, &Position)>,
    ship_q: Query<(Entity, &Ship, &ShipState)>,
    star_positions: Query<&Position, With<StarSystem>>,
    dss_q: Query<(Entity, &Position), With<DeepSpaceStructure>>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
    mut sel: SelectionState,
    mut command_queues: Query<&mut CommandQueue>,
    mut egui_contexts: EguiContexts,
) {
    // #347: SELECTION_CANCEL is bound to Escape by default. Fall back to a
    // raw Escape check when no registry is installed (headless tests) so
    // existing behaviour is preserved.
    let cancel_pressed = match keybindings.as_deref() {
        Some(kb) => kb.is_just_pressed(crate::input::actions::SELECTION_CANCEL, &keys),
        None => keys.just_pressed(KeyCode::Escape),
    };
    // Escape handling — cancel deploy mode first if active, otherwise fall
    // through to the existing ship / system deselection logic.
    if cancel_pressed {
        if sel.deploy_mode.0.is_some() {
            sel.deploy_mode.0 = None;
            return;
        }
        if sel.selected_ship.0.is_some() {
            sel.selected_ship.0 = None;
            sel.selected_ships.clear();
            sel.context_menu.open = false;
        } else {
            sel.selected.0 = None;
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
    sel.context_menu.open = false;

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
        sel.selected.0 = None;
        sel.selected_ship.0 = None;
        sel.selected_ships.clear();
        return;
    };

    let cmd_held = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
    let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    // --- Collect all candidates near the click ---
    let candidates = collect_candidates(
        world_pos,
        &ship_q,
        &stars,
        &dss_q,
        &star_positions,
        &clock,
        view.scale,
    );

    // --- Find best star for deploy mode / context menu (unchanged behavior) ---
    let best_star = candidates
        .iter()
        .find(|c| c.kind == CycleKind::StarSystem)
        .map(|c| (c.entity, c.distance));

    // #229 / #240: Deploy mode — if the user has just clicked "Deploy" on a
    // cargo item and then clicks on the map, push a DeployDeliverable command.
    // Clicks close to a star snap to that star's coordinates; clicks on empty
    // space deploy at the cursor's world position (z = 0).
    if let Some(pending) = sel.deploy_mode.0 {
        let snapped_star_world = best_star
            .and_then(|(star_entity, _)| star_positions.get(star_entity).ok())
            .map(|p| p.as_array());
        let target_pos = resolve_deploy_target(snapped_star_world, world_pos, view.scale);
        if let Ok(mut queue) = command_queues.get_mut(pending.ship) {
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
        sel.deploy_mode.0 = None;
        return;
    }

    // When a ship IS selected AND Cmd is held: context menu / default action
    if sel.selected_ship.0.is_some() && cmd_held {
        if let Some((star_entity, _)) = best_star {
            sel.context_menu.open = true;
            sel.context_menu.position = [cursor_pos.x, cursor_pos.y];
            sel.context_menu.target_system = Some(star_entity);
            sel.context_menu.execute_default = shift_held; // Cmd+Shift = default action
            return;
        }
    }

    if candidates.is_empty() {
        // Clicked empty space
        sel.selected.0 = None;
        sel.selected_ship.0 = None;
        sel.selected_ships.clear();
        sel.cycle.candidates.clear();
        return;
    }

    // --- #368: Double-click cycle detection ---
    let now = Instant::now();
    let elapsed_secs = now.duration_since(sel.cycle.last_click_time).as_secs_f32();
    let pos_delta = world_pos.distance(sel.cycle.click_world_pos);
    let is_double_click = elapsed_secs < DOUBLE_CLICK_TIME
        && pos_delta < DOUBLE_CLICK_RADIUS_PX
        && !sel.cycle.candidates.is_empty();

    if is_double_click && candidates.len() > 1 {
        // Advance to next candidate in cycle
        sel.cycle.current_index = (sel.cycle.current_index + 1) % candidates.len();
        sel.cycle.candidates = candidates;
        sel.cycle.last_click_time = now;
        sel.cycle.click_world_pos = world_pos;

        let candidate = sel.cycle.candidates[sel.cycle.current_index];
        apply_candidate_selection(&candidate, &mut sel, shift_held);
    } else {
        // Single click: select first candidate (highest priority / nearest)
        sel.cycle.candidates = candidates;
        sel.cycle.current_index = 0;
        sel.cycle.last_click_time = now;
        sel.cycle.click_world_pos = world_pos;

        let candidate = sel.cycle.candidates[0];
        apply_candidate_selection(&candidate, &mut sel, shift_held);
    }
}

/// #407: Keep `SelectedShip` in sync with `SelectedShips.primary()`. This
/// runs every frame so code that only reads `SelectedShip` (visualization,
/// UI panels) always sees the primary of the multi-select set. Writes to
/// `SelectedShip.0` from UI code (outline, ship panel) are also propagated
/// back into `SelectedShips`.
fn sync_selected_ship_from_ships(
    mut selected_ship: ResMut<SelectedShip>,
    mut selected_ships: ResMut<SelectedShips>,
) {
    let primary = selected_ships.primary();
    if selected_ship.0 != primary {
        // If SelectedShip was changed externally (e.g., by outline click),
        // update SelectedShips to match.
        if let Some(e) = selected_ship.0 {
            if !selected_ships.contains(e) {
                selected_ships.set_single(e);
            }
        } else {
            selected_ships.clear();
        }
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
    stars: Query<(&StarSystem, &Position)>,
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
    for (_star, pos) in &stars {
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

    // -----------------------------------------------------------------------
    // #407: SelectedShips multi-select tests
    // -----------------------------------------------------------------------

    #[test]
    fn shift_click_toggles_selection() {
        let mut selected = SelectedShips::default();
        let mut world = bevy::prelude::World::new();
        let e1 = world.spawn_empty().id();
        let e2 = world.spawn_empty().id();

        // Set single
        selected.set_single(e1);
        assert_eq!(selected.len(), 1);
        assert!(selected.contains(e1));
        assert_eq!(selected.primary(), Some(e1));

        // Toggle adds e2
        selected.toggle(e2);
        assert_eq!(selected.len(), 2);
        assert!(selected.contains(e1));
        assert!(selected.contains(e2));
        assert_eq!(selected.primary(), Some(e1));

        // Toggle removes e1
        selected.toggle(e1);
        assert_eq!(selected.len(), 1);
        assert!(!selected.contains(e1));
        assert!(selected.contains(e2));
        assert_eq!(selected.primary(), Some(e2));

        // Toggle removes e2 — now empty
        selected.toggle(e2);
        assert!(selected.is_empty());
        assert_eq!(selected.primary(), None);

        // Clear
        selected.set_single(e1);
        selected.toggle(e2);
        selected.clear();
        assert!(selected.is_empty());
    }

    // -----------------------------------------------------------------------
    // #368: CycleSelection / double-click cycle tests
    // -----------------------------------------------------------------------

    #[test]
    fn cycle_kind_priority_order() {
        // Ships are selected first (priority 0), then stars, then structures.
        assert!(CycleKind::Ship.priority() < CycleKind::StarSystem.priority());
        assert!(CycleKind::StarSystem.priority() < CycleKind::DeepSpaceStructure.priority());
    }

    #[test]
    fn cycle_selection_default_is_empty() {
        let cycle = CycleSelection::default();
        assert!(cycle.candidates.is_empty());
        assert_eq!(cycle.current_index, 0);
    }

    #[test]
    fn candidates_sorted_by_priority_then_distance() {
        let mut world = bevy::prelude::World::new();
        let e_star = world.spawn_empty().id();
        let e_ship1 = world.spawn_empty().id();
        let e_ship2 = world.spawn_empty().id();
        let e_dss = world.spawn_empty().id();

        let mut candidates = vec![
            CycleCandidate {
                entity: e_star,
                kind: CycleKind::StarSystem,
                distance: 3.0,
            },
            CycleCandidate {
                entity: e_ship1,
                kind: CycleKind::Ship,
                distance: 5.0,
            },
            CycleCandidate {
                entity: e_dss,
                kind: CycleKind::DeepSpaceStructure,
                distance: 1.0,
            },
            CycleCandidate {
                entity: e_ship2,
                kind: CycleKind::Ship,
                distance: 2.0,
            },
        ];

        candidates.sort_by(|a, b| {
            a.kind
                .priority()
                .cmp(&b.kind.priority())
                .then(
                    a.distance
                        .partial_cmp(&b.distance)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.entity.cmp(&b.entity))
        });

        // Ships first (sorted by distance), then star, then DSS
        assert_eq!(candidates[0].entity, e_ship2); // ship, dist 2
        assert_eq!(candidates[1].entity, e_ship1); // ship, dist 5
        assert_eq!(candidates[2].entity, e_star); // star, dist 3
        assert_eq!(candidates[3].entity, e_dss); // dss, dist 1
    }

    #[test]
    fn double_click_detection_within_time_and_radius() {
        let mut cycle = CycleSelection::default();
        let mut world = bevy::prelude::World::new();
        let e1 = world.spawn_empty().id();
        let e2 = world.spawn_empty().id();

        // Simulate first click
        cycle.candidates = vec![
            CycleCandidate {
                entity: e1,
                kind: CycleKind::Ship,
                distance: 3.0,
            },
            CycleCandidate {
                entity: e2,
                kind: CycleKind::StarSystem,
                distance: 5.0,
            },
        ];
        cycle.current_index = 0;
        cycle.click_world_pos = Vec2::new(100.0, 200.0);
        cycle.last_click_time = Instant::now();

        // Simulate second click at nearly same position, immediately after
        let now = Instant::now();
        let pos = Vec2::new(101.0, 200.0); // within 5px
        let elapsed = now.duration_since(cycle.last_click_time).as_secs_f32();
        let pos_delta = pos.distance(cycle.click_world_pos);

        assert!(elapsed < DOUBLE_CLICK_TIME);
        assert!(pos_delta < DOUBLE_CLICK_RADIUS_PX);

        // After double-click, index should advance
        let new_index = (cycle.current_index + 1) % cycle.candidates.len();
        assert_eq!(new_index, 1);
    }

    #[test]
    fn double_click_too_far_resets_cycle() {
        let cycle = CycleSelection {
            candidates: vec![CycleCandidate {
                entity: Entity::PLACEHOLDER,
                kind: CycleKind::Ship,
                distance: 1.0,
            }],
            current_index: 0,
            click_world_pos: Vec2::new(100.0, 200.0),
            last_click_time: Instant::now(),
        };

        // Click far away — should not be detected as double-click
        let pos = Vec2::new(200.0, 300.0);
        let pos_delta = pos.distance(cycle.click_world_pos);
        assert!(pos_delta >= DOUBLE_CLICK_RADIUS_PX);
    }

    #[test]
    fn cycle_wraps_around() {
        // With 3 candidates, cycling from index 2 wraps to 0
        let index = 2;
        let len = 3;
        assert_eq!((index + 1) % len, 0);
    }
}
