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
    draw_dashed_line_with_pattern(gizmos, start, end, color, 4.0, 4.0);
}

/// #489: Dashed-line drawer with caller-controlled dash/gap lengths.
/// `dash_len`/`gap_len` are in the same screen-space units as `start` /
/// `end`. The intended-trajectory overlay varies these via
/// [`intended_layer_dash_pattern`] so dispatch-fresh commands render as
/// short urgent dashes and settled (post-takes-effect) commands render as
/// longer relaxed dashes.
fn draw_dashed_line_with_pattern(
    gizmos: &mut Gizmos,
    start: Vec2,
    end: Vec2,
    color: Color,
    dash_len: f32,
    gap_len: f32,
) {
    let diff = end - start;
    let length = diff.length();
    if length <= 0.0 {
        return;
    }
    let dir = diff / length;
    let dash_len = dash_len.max(0.5);
    let gap_len = gap_len.max(0.5);
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

/// #478: One row of intended-trajectory data the renderer should draw on
/// top of the (already-rendered) projected layer.
///
/// Produced by [`compute_intended_render_inputs`] when the dispatcher's
/// projection has a divergent intended target (= a command is in flight to
/// the ship, or the ship is in transit to the intended target).
///
/// `alpha` is precomputed by [`intended_layer_alpha`] so the same number is
/// testable independently of gizmos. The dashed line is drawn from the
/// projected position to the intended position.
#[derive(Clone, Debug, PartialEq)]
pub struct IntendedRenderItem {
    pub entity: Entity,
    pub design_id: String,
    pub projected_system: Option<Entity>,
    pub intended_system: Option<Entity>,
    pub alpha: f32,
    /// #489: (dash_length, gap_length) in screen-space pixels. Varies
    /// with the clock to give the player a second perceptible channel
    /// (urgency-vs-settled) on top of the alpha curve.
    pub dash_pattern: (f32, f32),
}

/// Alpha floor for the intended-trajectory overlay once the command has
/// reached the ship (`now >= takes_effect_at`). Kept low so a "settled"
/// dashed trail is visible but recedes to background. Widened from the
/// #478 value of 0.4 to 0.3 (#489) to expand the perceptible delta
/// against the dark Galaxy Map background.
const INTENDED_ALPHA_FLOOR: f32 = 0.3;
/// Alpha ceiling for the intended-trajectory overlay at the dispatch
/// tick — fresh commands render at near-full opacity so the player can
/// instantly tell "I just sent that, it hasn't arrived yet". Widened
/// from #478's 0.8 to 1.0 (#489).
const INTENDED_ALPHA_CEIL: f32 = 1.0;

/// Dash/gap pattern at the dispatch tick — short urgent dashes signal
/// "command in flight". (#489)
const INTENDED_DASH_AT_DISPATCH: (f32, f32) = (4.0, 2.0);
/// Dash/gap pattern once the command has taken effect — long relaxed
/// dashes signal "settled / in transit / waiting for arrival
/// confirmation". (#489)
const INTENDED_DASH_AFTER_TAKES_EFFECT: (f32, f32) = (8.0, 4.0);

/// #478 / #489: Compute the alpha for the intended-trajectory dashed
/// overlay.
///
/// Curve (#489 widened from #478's 0.4→0.8 to 0.3→1.0):
/// * Right at dispatch → 1.0 (full opacity; "command just sent").
/// * Linearly fades toward 0.3 across `[dispatched_at, takes_effect_at]`.
/// * After the command has reached the ship (`now >= takes_effect_at`),
///   the layer holds at 0.3 — the dashed line still shows the ship is
///   *not yet at* the intended target, but is no longer "in flight to ship".
/// * If the projection has no `intended_takes_effect_at`, falls back to
///   the steady floor value.
pub fn intended_layer_alpha(projection: &ShipProjection, now: i64) -> f32 {
    let Some(takes_effect_at) = projection.intended_takes_effect_at else {
        return INTENDED_ALPHA_FLOOR;
    };
    if now >= takes_effect_at {
        return INTENDED_ALPHA_FLOOR;
    }
    let span = takes_effect_at - projection.dispatched_at;
    if span <= 0 {
        return INTENDED_ALPHA_CEIL;
    }
    let remaining = (takes_effect_at - now) as f32;
    let total = span as f32;
    // fraction in [0, 1]: 1.0 at dispatch_tick, 0.0 at takes_effect_at
    let fraction = (remaining / total).clamp(0.0, 1.0);
    INTENDED_ALPHA_FLOOR + fraction * (INTENDED_ALPHA_CEIL - INTENDED_ALPHA_FLOOR)
}

/// #489: Compute the (dash_length, gap_length) pattern for the
/// intended-trajectory dashed overlay at the given clock tick.
///
/// Pattern interpolation mirrors [`intended_layer_alpha`]:
/// * At dispatch tick → short urgent dashes (`(4.0, 2.0)`) — "fresh
///   order".
/// * At / after `takes_effect_at` → long settled dashes (`(8.0, 4.0)`).
/// * Linearly interpolated in between.
/// * Falls back to the post-takes-effect pattern when
///   `intended_takes_effect_at` is missing — that codepath is the
///   "steady-state divergence" case (no in-flight command tracking) and
///   the relaxed pattern is the right visual.
///
/// The returned values are in screen-space pixels (same units the
/// dashed-line drawer expects). Combined with the widened alpha curve
/// this gives the player two independent visual channels that
/// distinguish dispatch-fresh from settled commands against a dark
/// background.
pub fn intended_layer_dash_pattern(projection: &ShipProjection, now: i64) -> (f32, f32) {
    let Some(takes_effect_at) = projection.intended_takes_effect_at else {
        return INTENDED_DASH_AFTER_TAKES_EFFECT;
    };
    if now >= takes_effect_at {
        return INTENDED_DASH_AFTER_TAKES_EFFECT;
    }
    let span = takes_effect_at - projection.dispatched_at;
    if span <= 0 {
        return INTENDED_DASH_AT_DISPATCH;
    }
    let remaining = (takes_effect_at - now) as f32;
    let total = span as f32;
    // fraction in [0, 1]: 1.0 at dispatch_tick, 0.0 at takes_effect_at
    let fraction = (remaining / total).clamp(0.0, 1.0);
    let (d0, g0) = INTENDED_DASH_AT_DISPATCH;
    let (d1, g1) = INTENDED_DASH_AFTER_TAKES_EFFECT;
    // fraction=1 → urgent (d0,g0); fraction=0 → settled (d1,g1).
    let dash = d1 + fraction * (d0 - d1);
    let gap = g1 + fraction * (g0 - g1);
    (dash, gap)
}

/// #478: Pure helper — given a viewing empire's [`KnowledgeStore`], a
/// per-entity metadata lookup, and the current clock tick, compute the
/// intended-trajectory rows the renderer should overlay on top of the
/// projected layer.
///
/// Filtering rules (matches the [`compute_own_ship_render_inputs`] gates,
/// plus the divergence requirement so we don't draw a zero-length line):
/// * Skip ships with no realtime metadata (entity despawned).
/// * Skip ships not owned by the viewing empire.
/// * Skip stations.
/// * Skip projections in terminal `Destroyed` / `Missing` states (the
///   reconciler clears `intended_*` on these, but pin it explicitly).
/// * Skip projections with `intended_system == None` (no in-flight intent).
/// * Skip projections where `projected_system == intended_system` (already
///   reconciled / converged — no divergence to draw).
pub fn compute_intended_render_inputs(
    store: &KnowledgeStore,
    metadata: &HashMap<Entity, OwnShipMetadata>,
    now: i64,
) -> Vec<IntendedRenderItem> {
    let mut out = Vec::new();
    for (ship_entity, projection) in store.iter_projections() {
        let Some(meta) = metadata.get(ship_entity) else {
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
                continue;
            }
            _ => {}
        }
        if projection.intended_system.is_none() {
            continue;
        }
        // Divergence: only draw when projected != intended. When equal the
        // ship is at the intended target (reconciled) — the projected layer
        // already covers it.
        if projection.projected_system == projection.intended_system {
            continue;
        }
        out.push(IntendedRenderItem {
            entity: *ship_entity,
            design_id: meta.design_id.clone(),
            projected_system: projection.projected_system,
            intended_system: projection.intended_system,
            alpha: intended_layer_alpha(projection, now),
            dash_pattern: intended_layer_dash_pattern(projection, now),
        });
    }
    out
}

/// #478: Resolve the on-screen position of a system entity for the
/// intended-trajectory overlay. Returns `None` if the system can't be
/// resolved to a [`Position`].
fn system_screen_pos(
    system: Entity,
    stars: &Query<&Position, With<StarSystem>>,
    view_scale: f32,
) -> Option<Vec2> {
    let pos = stars.get(system).ok()?;
    Some(Vec2::new(
        pos.x as f32 * view_scale,
        pos.y as f32 * view_scale,
    ))
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

    // #478: Intended-trajectory overlay. Drawn AFTER docked ships so the
    // dashed line connects the ship marker to the player-commanded
    // destination. Only emitted when the intended layer diverges from the
    // projected layer (= a command is in flight to the ship, or the ship
    // is locally believed to be moving toward the intended target but the
    // reconciler hasn't confirmed arrival yet).
    let intended_items = compute_intended_render_inputs(viewing_store, &metadata, clock.elapsed);
    for item in &intended_items {
        // Resolve start (projected) position. For ships in deep-space
        // (Loitering / pre-arrival InTransit), `projected_system` is None
        // — fall back to the projection's screen pos helper which handles
        // the Loitering case via inline coordinates.
        let start = match item.projected_system {
            Some(sys) => system_screen_pos(sys, &stars, view.scale),
            None => viewing_store
                .get_projection(item.entity)
                .and_then(|p| projection_screen_pos(p, &stars, view.scale)),
        };
        let Some(start) = start else {
            continue;
        };
        let Some(intended_sys) = item.intended_system else {
            continue;
        };
        let Some(end) = system_screen_pos(intended_sys, &stars, view.scale) else {
            continue;
        };
        let (r, g, b) = ship_color_rgb(&item.design_id);
        let (dash_len, gap_len) = item.dash_pattern;
        draw_dashed_line_with_pattern(
            &mut gizmos,
            start,
            end,
            Color::srgba(r, g, b, item.alpha),
            dash_len,
            gap_len,
        );
        // Subtle ring at the intended target so the player can see where
        // the command will land even at low alpha.
        gizmos.circle_2d(end, 4.5, Color::srgba(r, g, b, item.alpha * 0.7));
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
