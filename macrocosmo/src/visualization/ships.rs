use std::collections::HashMap;

use bevy::prelude::*;

use super::{GalaxyView, SelectedShip};
use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::knowledge::{KnowledgeStore, ShipProjection, ShipSnapshotState};
use crate::player::{Empire, PlayerEmpire};
use crate::ship::{CommandQueue, Owner, QueuedCommand, Ship, ShipState, ShipStats};
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
    is_harbour: bool,
}

/// #477: Light-coherent metadata about an own-empire ship as the viewing
/// empire perceives it through its [`KnowledgeStore::projections`].
///
/// `name` / `design_id` / `is_harbour` / `is_station` are read from the
/// realtime [`Ship`] / [`ShipStats`] components — own-empire metadata
/// (build cost, hull, harbour capacity) is locally known and not bound by
/// light-speed. The *position-affecting state* (`projected_state`,
/// `projected_system`) comes purely from the projection store.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnShipRenderItem {
    pub entity: Entity,
    pub design_id: String,
    pub is_station: bool,
    pub is_harbour: bool,
    pub projected_state: ShipSnapshotState,
    pub projected_system: Option<Entity>,
}

/// Per-entity ship metadata pulled from realtime ECS Components.
///
/// Only describes Components that are *not* light-delayed for the viewing
/// empire (own-empire ship build data, role flags). The realtime
/// [`ShipState`] is intentionally NOT part of this metadata — that's the
/// FTL leak #477 fixes.
#[derive(Clone, Debug)]
pub struct OwnShipMetadata {
    pub design_id: String,
    pub is_station: bool,
    pub is_harbour: bool,
    pub owned_by_viewing_empire: bool,
}

/// #477: Pure helper — given a viewing empire's [`KnowledgeStore`] and a
/// per-entity metadata lookup, compute what the renderer should draw for
/// each own-empire ship. Returns an empty `Vec` if the store has no
/// projections.
///
/// Skips:
/// * ships with no realtime metadata (the entity has been despawned —
///   `Destroyed`/`Missing` snapshots are rendered by the foreign-ghost
///   branch in [`draw_ships`] which handles both own and foreign empires
///   for despawned ships);
/// * stations (rendered as overlay icons, not ship markers);
/// * `Destroyed` / `Missing` projected states (also rendered via the
///   snapshot ghost branch for visual consistency with foreign ships);
/// * ships whose [`Ship::owner`] is not the viewing empire — projections
///   are dispatcher-keyed but defense-in-depth is cheap here.
pub fn compute_own_ship_render_inputs(
    store: &KnowledgeStore,
    metadata: &HashMap<Entity, OwnShipMetadata>,
) -> Vec<OwnShipRenderItem> {
    let mut out = Vec::new();
    for (ship_entity, projection) in store.iter_projections() {
        let Some(meta) = metadata.get(ship_entity) else {
            // Entity gone (Destroyed/Missing reconciled) — let the
            // snapshot ghost branch render it.
            continue;
        };
        if !meta.owned_by_viewing_empire {
            continue;
        }
        if meta.is_station {
            continue;
        }
        match &projection.projected_state {
            ShipSnapshotState::Destroyed | ShipSnapshotState::Missing => {
                // Terminal states render via the existing ghost branch
                // for parity with foreign ship rendering.
                continue;
            }
            _ => {}
        }
        out.push(OwnShipRenderItem {
            entity: *ship_entity,
            design_id: meta.design_id.clone(),
            is_station: meta.is_station,
            is_harbour: meta.is_harbour,
            projected_state: projection.projected_state.clone(),
            projected_system: projection.projected_system,
        });
    }
    out
}

/// #477: Resolve the on-screen position implied by a [`ShipProjection`].
///
/// `view_scale` is `GalaxyView.scale`. Returns `None` if the projection's
/// `projected_system` cannot be resolved to a [`Position`]. For
/// `Loitering`, the position comes directly from the projection's inline
/// coordinates and never consults `stars`.
///
/// `InTransit`: the projection does not carry from/to/depart/eta
/// interpolation fields, so we draw at `projected_system` (= the
/// destination, per #475 / #476). This is coarser than the pre-#477
/// realtime renderer (which interpolated along the leg), but is the
/// light-coherent answer until #478 expands the projection schema.
pub fn projection_screen_pos(
    projection: &ShipProjection,
    stars: &Query<&Position, With<StarSystem>>,
    view_scale: f32,
) -> Option<Vec2> {
    if let ShipSnapshotState::Loitering { position } = &projection.projected_state {
        return Some(Vec2::new(
            position[0] as f32 * view_scale,
            position[1] as f32 * view_scale,
        ));
    }
    let system = projection.projected_system?;
    let pos = stars.get(system).ok()?;
    Some(Vec2::new(
        pos.x as f32 * view_scale,
        pos.y as f32 * view_scale,
    ))
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
    empire_q: Query<(Entity, &KnowledgeStore), With<PlayerEmpire>>,
    all_empire_stores: Query<&KnowledgeStore, With<Empire>>,
    _player_q: Query<&crate::player::StationedAt, With<crate::player::Player>>,
    observer_mode: Res<crate::observer::ObserverMode>,
    observer_view: Res<crate::observer::ObserverView>,
    all_empire_q: Query<Entity, With<Empire>>,
) {
    // #434 / #477: Resolve the viewing empire (PlayerEmpire in normal play,
    // ObserverView.viewing in observer mode). The ship marker rendering
    // pipeline reads from this empire's `KnowledgeStore.projections` so the
    // galaxy map is light-coherent: no realtime ECS `ShipState` is consulted
    // for own-empire ship rendering (epic #473).
    let empire_entity = if observer_mode.enabled {
        observer_view.viewing.and_then(|e| all_empire_q.get(e).ok())
    } else {
        empire_q.single().ok().map(|(e, _)| e)
    };
    let Some(empire_entity) = empire_entity else {
        return;
    };

    // Look up the viewing empire's KnowledgeStore. Both `empire_q` and
    // `all_empire_stores` borrow `&KnowledgeStore` (read-only), so they
    // do not conflict per Bevy B0001.
    let Ok(viewing_store) = all_empire_stores.get(empire_entity) else {
        return;
    };

    // Build the metadata table from the realtime ships query. Only own-empire
    // ships' `Ship` / `ShipStats` Components are read here — the realtime
    // `ShipState` is intentionally NOT consulted (the FTL leak fix).
    let mut metadata: HashMap<Entity, OwnShipMetadata> = HashMap::new();
    for (entity, ship, _state, _queue, stats) in &ships {
        let owned_by_viewing_empire = matches!(ship.owner, Owner::Empire(e) if e == empire_entity);
        metadata.insert(
            entity,
            OwnShipMetadata {
                design_id: ship.design_id.clone(),
                is_station: is_station(ship),
                is_harbour: is_harbour(stats),
                owned_by_viewing_empire,
            },
        );
    }

    // #477: Compute the projection-driven render items. This is the only
    // source of own-ship marker positions on the galaxy map.
    let render_items = compute_own_ship_render_inputs(viewing_store, &metadata);

    // Group docked ships by system so we can offset them.
    // #395: Immobile ships (stations / infrastructure) are excluded entirely
    // (filtered out by `compute_own_ship_render_inputs`) — they are
    // represented by icons in the galaxy overlay instead.
    let mut docked_counts: HashMap<Entity, Vec<DockedShipInfo>> = HashMap::new();
    let mut system_ship_counts: HashMap<Entity, u32> = HashMap::new();

    for item in &render_items {
        match &item.projected_state {
            // Docked-style states render as a circle around the system.
            ShipSnapshotState::InSystem | ShipSnapshotState::Refitting => {
                let Some(system) = item.projected_system else {
                    continue;
                };
                docked_counts
                    .entry(system)
                    .or_default()
                    .push(DockedShipInfo {
                        design_id: item.design_id.clone(),
                        is_harbour: item.is_harbour,
                    });
                *system_ship_counts.entry(system).or_insert(0) += 1;
            }
            // #477: `InTransit` falls back to drawing at `projected_system`
            // (the destination, per #475). The pre-#477 realtime renderer
            // interpolated from origin → destination based on departed_at /
            // arrival_at, but `ShipProjection` doesn't carry those fields;
            // adding them is deferred to a later schema bump (epic #473
            // sub-issue E or follow-up). The coarser draw is the
            // light-coherent answer.
            ShipSnapshotState::InTransit => {
                let Some(system) = item.projected_system else {
                    continue;
                };
                let Ok(sys_pos) = stars.get(system) else {
                    continue;
                };
                let cx = sys_pos.x as f32 * view.scale;
                let cy = sys_pos.y as f32 * view.scale;
                let (r, g, b) = ship_color_rgb(&item.design_id);
                // Same semi-transparent marker the FTL ghost path used,
                // marking the projected destination as the ship's
                // light-coherent location.
                gizmos.circle_2d(Vec2::new(cx, cy), 3.0, Color::srgba(r, g, b, 0.4));
            }
            ShipSnapshotState::Settling => {
                let Some(system) = item.projected_system else {
                    continue;
                };
                if let Ok(sys_pos) = stars.get(system) {
                    let sx = sys_pos.x as f32 * view.scale;
                    let sy = sys_pos.y as f32 * view.scale;
                    let (r, g, b) = ship_color_rgb(&item.design_id);
                    let pulse = (clock.as_years_f64() as f32 * 3.0).sin() * 0.3 + 0.7;
                    gizmos.circle_2d(Vec2::new(sx, sy), 6.0, Color::srgba(r, g, b, pulse));
                    gizmos.circle_2d(Vec2::new(sx, sy), 3.5, Color::srgb(r, g, b));
                }
            }
            ShipSnapshotState::Surveying => {
                let Some(system) = item.projected_system else {
                    continue;
                };
                if let Ok(sys_pos) = stars.get(system) {
                    let sx = sys_pos.x as f32 * view.scale;
                    let sy = sys_pos.y as f32 * view.scale;
                    let (r, g, b) = ship_color_rgb(&item.design_id);
                    let pulse = (clock.as_years_f64() as f32 * 5.0).sin() * 0.3 + 0.7;
                    gizmos.circle_2d(Vec2::new(sx, sy), 6.0, Color::srgba(r, g, b, pulse));
                    gizmos.circle_2d(Vec2::new(sx, sy), 3.5, Color::srgb(r, g, b));
                }
            }
            // #185: Loitering ships are drawn at their inline deep-space coord.
            ShipSnapshotState::Loitering { position } => {
                let cx = position[0] as f32 * view.scale;
                let cy = position[1] as f32 * view.scale;
                let (r, g, b) = ship_color_rgb(&item.design_id);
                gizmos.circle_2d(Vec2::new(cx, cy), 3.0, Color::srgb(r, g, b));
                gizmos.circle_2d(Vec2::new(cx, cy), 5.5, Color::srgba(r, g, b, 0.25));
            }
            // Destroyed / Missing are filtered by `compute_own_ship_render_inputs`
            // and rendered by the foreign-ghost branch below.
            ShipSnapshotState::Destroyed | ShipSnapshotState::Missing => {}
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
            let offset_radius = 8.0;
            let ox = sx + angle.cos() * offset_radius;
            let oy = sy + angle.sin() * offset_radius;

            if info.is_harbour {
                // Harbour ships: gold diamond
                let gold = Color::srgb(1.0, 0.85, 0.2);
                let radius = 5.5;
                let center = Vec2::new(ox, oy);
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

    // #104 / #477: Command queue overlay for selected ship.
    // Starting position is read from the viewing empire's `ShipProjection`
    // so the dashed queue path begins at the same point the ship marker is
    // drawn. Falls back to `None` (no overlay) if no projection exists for
    // the ship — that's normal for foreign-empire / freshly-spawned ships.
    if let Some(selected_entity) = selected_ship.0 {
        if let Ok((_entity, ship, _state, Some(queue), _stats)) = ships.get(selected_entity) {
            if !queue.commands.is_empty() {
                let current_pos = viewing_store
                    .get_projection(selected_entity)
                    .and_then(|p| projection_screen_pos(p, &stars, view.scale));

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

    // #409: Ghost rendering for destroyed ships whose destruction hasn't
    // reached the player yet via light-speed. These ships are despawned
    // (no live entity) but their KnowledgeStore snapshot still shows them
    // alive at their last known position.
    if let Ok((_, store)) = empire_q.single() {
        let live_entities: std::collections::HashSet<Entity> =
            ships.iter().map(|(e, ..)| e).collect();

        for (_, snapshot) in store.iter_ships() {
            if live_entities.contains(&snapshot.entity) {
                continue;
            }
            if snapshot.last_known_state == ShipSnapshotState::Destroyed {
                continue;
            }

            let pos = match &snapshot.last_known_state {
                ShipSnapshotState::Loitering { position } => Some(Vec2::new(
                    position[0] as f32 * view.scale,
                    position[1] as f32 * view.scale,
                )),
                _ => snapshot.last_known_system.and_then(|sys| {
                    stars
                        .get(sys)
                        .ok()
                        .map(|p| Vec2::new(p.x as f32 * view.scale, p.y as f32 * view.scale))
                }),
            };

            if let Some(pos) = pos {
                if snapshot.last_known_state == ShipSnapshotState::Missing {
                    // Amber "?" pulsing marker for missing ships
                    let pulse = (clock.as_years_f64() as f32 * 3.0).sin() * 0.2 + 0.6;
                    gizmos.circle_2d(pos, 4.0, Color::srgba(1.0, 0.7, 0.1, pulse));
                    gizmos.circle_2d(pos, 6.5, Color::srgba(1.0, 0.7, 0.1, pulse * 0.4));
                } else {
                    let (r, g, b) = ship_color_rgb(&snapshot.design_id);
                    // Semi-transparent ghost marker
                    gizmos.circle_2d(pos, 3.0, Color::srgba(r, g, b, 0.3));
                    // Pulsing outer ring to indicate "last known"
                    let pulse = (clock.as_years_f64() as f32 * 2.0).sin() * 0.15 + 0.2;
                    gizmos.circle_2d(pos, 5.0, Color::srgba(r, g, b, pulse));
                }
            }
        }
    }
}
