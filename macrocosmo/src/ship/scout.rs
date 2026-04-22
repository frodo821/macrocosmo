//! #217: Scout command — reconnaissance dispatch + report mechanics.
//!
//! Flow overview:
//!
//! 1. [`QueuedCommand::Scout`](super::QueuedCommand::Scout) is queued on a
//!    ship that has a Scout module equipped and FTL capability.
//! 2. `handlers::handle_scout_requested` routes the ship to the target
//!    (auto-inserting `MoveTo`) and, once docked at the target, transitions
//!    the ship into [`ShipState::Scouting`](super::ShipState::Scouting).
//! 3. [`tick_scout_observation`] polls Scouting ships each tick. When
//!    `completes_at` is reached, it collects a sensor-range snapshot of the
//!    surrounding hostile ships and deep-space structures, attaches a
//!    [`ScoutReport`] to the ship, and parks the ship back in
//!    `ShipState::InSystem` at the target system.
//! 4. [`process_scout_report`] delivers the report:
//!    - `ReportMode::FtlComm` — if the scout position and the player empire
//!      are both covered by a paired FTL Comm Relay, write the snapshot into
//!      the empire's [`KnowledgeStore`] with `source = ObservationSource::Scout`
//!      immediately. Otherwise falls back to `Return` behavior.
//!    - `ReportMode::Return` — auto-queues a `MoveTo` to the ship's origin
//!      system and defers delivery until it docks there. `observed_at` is
//!      preserved as the time of observation (so the report is "old" by the
//!      time the ship arrives, which is the whole point of the mechanic).

use bevy::prelude::*;
use std::collections::HashMap;

use crate::components::Position;
use crate::galaxy::{AtSystem, Hostile, HostileStats, StarSystem};
use crate::knowledge::{
    KnowledgeStore, ObservationSource, ShipSnapshot, ShipSnapshotState, SystemKnowledge,
    SystemSnapshot,
};
use crate::physics::distance_ly_arr;
use crate::player::{Player, StationedAt};
use crate::time_system::GameClock;

use super::{CommandQueue, QueuedCommand, ReportMode, Ship, ShipHitpoints, ShipState};

/// Fallback sensor range for scout operations (light-years). In the MVP
/// release the value is a constant; future iterations will derive the range
/// from the equipped scout module's modifier (#219+).
pub const SCOUT_SENSOR_RANGE_LY: f64 = 3.0;

/// Canonical module id a ship must carry to accept a `Scout` command.
/// Defined in `scripts/ships/modules.lua` (`scout_module`). Rust-side code
/// references this id directly so the gating logic stays deterministic even
/// if the Lua-side `define_module` call is absent (e.g. the fallback test
/// registry).
pub const SCOUT_MODULE_ID: &str = "scout_module";

/// #217: Result of a scout ship's observation window.
///
/// Attached to the ship as a component when [`tick_scout_observation`]
/// finishes its timer. Consumed by [`process_scout_report`] once delivery
/// conditions are met.
#[derive(Component, Clone, Debug)]
pub struct ScoutReport {
    /// The system that was observed.
    pub target_system: Entity,
    /// The ship's cached origin (where it should return if FtlComm fails
    /// and for `Return` mode). Usually the system the scout was docked at
    /// when the Scout command started.
    pub origin_system: Entity,
    /// When the observation window ended (hexadies). Becomes the snapshot
    /// `observed_at` on delivery.
    pub observed_at: i64,
    /// Delivery channel selected by the player at dispatch time.
    pub report_mode: ReportMode,
    /// Snapshot of the target system at observation time.
    pub system_snapshot: SystemSnapshot,
    /// Ship snapshots within sensor range at observation time.
    pub ship_snapshots: Vec<ShipSnapshot>,
    /// Whether the ship has already been auto-queued home (Return fallback).
    /// Prevents `process_scout_report` from stacking duplicate `MoveTo`s
    /// across frames while the ship is in FTL / sublight.
    pub return_queued: bool,
}

/// Returns `true` if the ship has any module whose `module_id` matches
/// [`SCOUT_MODULE_ID`].
pub fn ship_has_scout_module(ship: &Ship) -> bool {
    ship.modules.iter().any(|m| m.module_id == SCOUT_MODULE_ID)
}

/// Returns `true` when the `scout_pos` is within a paired FTL Comm Relay
/// source range AND the player's current position is within the matching
/// partner relay's range. This mirrors the gating logic used by
/// `deep_space::relay_knowledge_propagate_system` but without writing any
/// knowledge — callers decide whether to treat the pair as "in range".
///
/// `relay_range_for(structure_definition_id)` returns the `ftl_comm_relay`
/// capability range in light-years, or `None` if the structure is not a
/// relay (e.g. partner was destroyed / unpaired). A range of `0.0` is
/// treated as "infinite" to match the relay module convention.
///
/// #217: Scout FTL comm coverage check. Does NOT modify relay state —
/// relay propagation itself is owned by #216.
#[allow(clippy::too_many_arguments)]
pub fn ftl_comm_covers(
    scout_pos: [f64; 3],
    player_pos: [f64; 3],
    relays: &[RelayCoverageSnapshot],
) -> bool {
    for relay in relays {
        let scout_to_source = distance_ly_arr(scout_pos, relay.source_pos);
        if relay.source_range > 0.0 && scout_to_source > relay.source_range {
            continue;
        }
        let player_to_partner = distance_ly_arr(player_pos, relay.partner_pos);
        if relay.partner_range > 0.0 && player_to_partner > relay.partner_range {
            continue;
        }
        return true;
    }
    false
}

/// Minimal snapshot of a single FTL Comm Relay pair for coverage checks.
/// Collected once per tick by [`collect_relay_coverage`].
#[derive(Clone, Copy, Debug)]
pub struct RelayCoverageSnapshot {
    pub source_pos: [f64; 3],
    pub partner_pos: [f64; 3],
    /// Source-side `ftl_comm_relay.range` in light-years. `0.0` means "infinite".
    pub source_range: f64,
    /// Partner-side `ftl_comm_relay.range` in light-years. `0.0` means "infinite".
    pub partner_range: f64,
}

/// #217: Walk live FTL Comm Relay pairs and return a flat list of coverage
/// snapshots. Ignores relays whose partner is dead (dangling pairs are
/// cleaned up by `verify_relay_pairings_system`).
///
/// This helper is intentionally lightweight and allocates only a small Vec;
/// the relay set is single-digit at MVP scope.
pub fn collect_relay_coverage(
    registry: &crate::deep_space::StructureRegistry,
    relays: &Query<
        (
            Entity,
            &crate::deep_space::DeepSpaceStructure,
            &Position,
            &crate::deep_space::FTLCommRelay,
        ),
        (
            Without<crate::deep_space::ConstructionPlatform>,
            Without<crate::deep_space::Scrapyard>,
        ),
    >,
    relay_positions: &Query<&Position, With<crate::deep_space::DeepSpaceStructure>>,
    partner_structures: &Query<&crate::deep_space::DeepSpaceStructure>,
) -> Vec<RelayCoverageSnapshot> {
    let mut out = Vec::new();

    let relay_range_for = |structure: &crate::deep_space::DeepSpaceStructure| -> Option<f64> {
        let def = registry.get(&structure.definition_id)?;
        let cap = def.capabilities.get("ftl_comm_relay")?;
        Some(cap.range)
    };

    for (_source_entity, source_structure, source_pos, relay) in relays.iter() {
        let Some(source_range) = relay_range_for(source_structure) else {
            continue;
        };
        let partner_entity = relay.paired_with;
        let Ok(partner_structure) = partner_structures.get(partner_entity) else {
            continue;
        };
        let Ok(partner_pos) = relay_positions.get(partner_entity) else {
            continue;
        };
        let Some(partner_range) = relay_range_for(partner_structure) else {
            continue;
        };

        out.push(RelayCoverageSnapshot {
            source_pos: source_pos.as_array(),
            partner_pos: partner_pos.as_array(),
            source_range,
            partner_range,
        });
    }

    out
}

/// #217: Observation tick — completes the Scouting timer and writes a
/// `ScoutReport` component onto the ship. Runs after game time has
/// advanced.
///
/// Implementation note: we can't have both `Query<..&mut ShipState..>` and
/// a second `Query<..&ShipState..>` in the same system (Bevy B0001). So we
/// first pass reads every ship's position / state / identity into an owned
/// snapshot Vec and then uses the mutable query to apply transitions.
#[allow(clippy::too_many_arguments)]
pub fn tick_scout_observation(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut ships: Query<(Entity, &Ship, &mut ShipState, &Position, &ShipHitpoints)>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
    hostiles: Query<
        (
            &AtSystem,
            &HostileStats,
            Option<&crate::faction::FactionOwner>,
        ),
        With<Hostile>,
    >,
    faction_relations: Res<crate::faction::FactionRelations>,
) {
    let now = clock.elapsed;

    // #293: Hostile systems lookup keyed by (viewer_empire, star_system) with
    // summed strength. Filter by FactionRelations so hostiles the viewing
    // empire considers non-aggressive do not register as "hostile" in
    // scout reports. Build lazily per viewer empire.
    let mut hostile_map_cache: HashMap<Option<Entity>, HashMap<Entity, f64>> = HashMap::new();
    let build_hostile_map =
        |viewer: Option<Entity>,
         hostiles: &Query<(&AtSystem, &HostileStats, Option<&crate::faction::FactionOwner>), With<Hostile>>,
         relations: &crate::faction::FactionRelations|
         -> HashMap<Entity, f64> {
            let mut map: HashMap<Entity, f64> = HashMap::new();
            for (at_system, stats, owner) in hostiles.iter() {
                let include = match (viewer, owner) {
                    (Some(v), Some(o)) => relations
                        .get_or_default(v, o.0)
                        .can_attack_aggressive(),
                    _ => true,
                };
                if include {
                    *map.entry(at_system.0).or_insert(0.0) += stats.strength;
                }
            }
            map
        };

    // First pass: collect an owned snapshot of every ship so we can sensor-
    // scan without borrowing the mutable query twice. Also identify which
    // ships are completing their Scouting window this tick.
    #[derive(Clone)]
    struct ShipObservation {
        entity: Entity,
        name: String,
        design_id: String,
        pos: [f64; 3],
        hull: f64,
        hull_max: f64,
        snapshot_state: ShipSnapshotState,
        last_system: Option<Entity>,
    }
    let mut all_ships: Vec<ShipObservation> = Vec::new();
    let mut completions: Vec<(
        Entity,
        Entity,           // target_system
        Entity,           // origin_system
        ReportMode,
        [f64; 3],
        Option<Entity>,   // owner empire entity
    )> = Vec::new();

    for (ship_entity, ship, state, ship_pos, hp) in ships.iter() {
        let (snap_state, last_system) = match state {
            ShipState::InSystem { system } => (ShipSnapshotState::InSystem, Some(*system)),
            ShipState::SubLight { target_system, .. } => {
                (ShipSnapshotState::InTransit, *target_system)
            }
            ShipState::InFTL {
                destination_system, ..
            } => (ShipSnapshotState::InTransit, Some(*destination_system)),
            ShipState::Surveying { target_system, .. } => {
                (ShipSnapshotState::Surveying, Some(*target_system))
            }
            ShipState::Settling { system, .. } => (ShipSnapshotState::Settling, Some(*system)),
            ShipState::Refitting { system, .. } => (ShipSnapshotState::Refitting, Some(*system)),
            ShipState::Loitering { position } => (
                ShipSnapshotState::Loitering {
                    position: *position,
                },
                None,
            ),
            ShipState::Scouting { target_system, .. } => {
                (ShipSnapshotState::Surveying, Some(*target_system))
            }
        };
        all_ships.push(ShipObservation {
            entity: ship_entity,
            name: ship.name.clone(),
            design_id: ship.design_id.clone(),
            pos: ship_pos.as_array(),
            hull: hp.hull,
            hull_max: hp.hull_max,
            snapshot_state: snap_state,
            last_system,
        });

        if let ShipState::Scouting {
            target_system,
            origin_system,
            completes_at,
            report_mode,
            ..
        } = *state
        {
            if now >= completes_at {
                let owner_empire = match ship.owner {
                    super::Owner::Empire(e) => Some(e),
                    super::Owner::Neutral => None,
                };
                completions.push((
                    ship_entity,
                    target_system,
                    origin_system,
                    report_mode,
                    ship_pos.as_array(),
                    owner_empire,
                ));
            }
        }
    }

    for (ship_entity, target_system, origin_system, report_mode, scout_pos, owner_empire) in completions {
        // Build per-empire hostile map (cached).
        let hostile_map = hostile_map_cache
            .entry(owner_empire)
            .or_insert_with(|| build_hostile_map(owner_empire, &hostiles, &faction_relations));

        // Build system snapshot — minimal survey-compatible payload.
        let (system_name, system_pos, surveyed) = match systems.get(target_system) {
            Ok((s, p)) => (s.name.clone(), p.as_array(), s.surveyed),
            Err(_) => {
                warn!("Scout {ship_entity:?}: target system despawned mid-observation");
                if let Ok((_, _, mut state, _, _)) = ships.get_mut(ship_entity) {
                    *state = ShipState::InSystem {
                        system: target_system,
                    };
                }
                continue;
            }
        };

        let has_hostile_here = hostile_map.contains_key(&target_system);
        let hostile_strength = hostile_map.get(&target_system).copied().unwrap_or(0.0);

        let system_snapshot = SystemSnapshot {
            name: system_name,
            position: system_pos,
            surveyed,
            has_hostile: has_hostile_here,
            hostile_strength,
            ..default()
        };

        // Ship snapshots within sensor range, derived from the owned first-pass.
        let mut ship_snapshots = Vec::new();
        for obs in &all_ships {
            if obs.entity == ship_entity {
                continue;
            }
            let dist = distance_ly_arr(scout_pos, obs.pos);
            if dist > SCOUT_SENSOR_RANGE_LY {
                continue;
            }
            ship_snapshots.push(ShipSnapshot {
                entity: obs.entity,
                name: obs.name.clone(),
                design_id: obs.design_id.clone(),
                last_known_state: obs.snapshot_state.clone(),
                last_known_system: obs.last_system,
                observed_at: now,
                hp: obs.hull,
                hp_max: obs.hull_max,
                source: ObservationSource::Scout,
            });
        }

        let contacts = ship_snapshots.len();
        let report = ScoutReport {
            target_system,
            origin_system,
            observed_at: now,
            report_mode,
            system_snapshot,
            ship_snapshots,
            return_queued: false,
        };

        // Park the ship at the target system and attach the report.
        if let Ok((_, _, mut state, _, _)) = ships.get_mut(ship_entity) {
            *state = ShipState::InSystem {
                system: target_system,
            };
        }
        commands.entity(ship_entity).try_insert(report);
        info!(
            "Scout observation complete: ship {:?} observed system {:?} ({} ship contacts, hostile={})",
            ship_entity, target_system, contacts, has_hostile_here,
        );
    }
}

/// #217: Scout report delivery. Runs after `tick_scout_observation` and the
/// command queue systems; either writes to the empire's `KnowledgeStore`
/// (FtlComm mode with live coverage) or auto-queues a return move to
/// `origin_system` (Return mode, or FtlComm fallback).
#[allow(clippy::too_many_arguments)]
pub fn process_scout_report(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut reports: Query<(
        Entity,
        &Ship,
        &ShipState,
        &Position,
        &mut CommandQueue,
        &mut ScoutReport,
    )>,
    mut empire_q: Query<&mut KnowledgeStore, With<crate::player::Empire>>,
    player_q: Query<&StationedAt, With<Player>>,
    viewer_q: Query<&crate::player::EmpireViewerSystem, With<crate::player::Empire>>,
    positions: Query<&Position>,
    // FTL comm coverage inputs.
    structure_registry: Res<crate::deep_space::StructureRegistry>,
    relays: Query<
        (
            Entity,
            &crate::deep_space::DeepSpaceStructure,
            &Position,
            &crate::deep_space::FTLCommRelay,
        ),
        (
            Without<crate::deep_space::ConstructionPlatform>,
            Without<crate::deep_space::Scrapyard>,
        ),
    >,
    relay_positions: Query<&Position, With<crate::deep_space::DeepSpaceStructure>>,
    partner_structures: Query<&crate::deep_space::DeepSpaceStructure>,
    system_positions: Query<&Position, With<StarSystem>>,
) {
    // Player position used for FTL comm coverage check (player empire only).
    let player_pos: Option<[f64; 3]> = player_q
        .iter()
        .next()
        .and_then(|s| positions.get(s.system).ok().map(|p| p.as_array()));

    let coverage = collect_relay_coverage(
        &structure_registry,
        &relays,
        &relay_positions,
        &partner_structures,
    );

    for (ship_entity, ship, state, ship_pos, mut queue, mut report) in reports.iter_mut() {
        // Look up the ship's owner empire's KnowledgeStore.
        let owner_entity = match ship.owner {
            super::Owner::Empire(e) => Some(e),
            super::Owner::Neutral => None,
        };

        // Determine the empire viewer position for FTL comm coverage.
        // Use the EmpireViewerSystem position for the ship's owner empire.
        // Fall back to the player's StationedAt position for backward compat.
        let empire_viewer_pos: Option<[f64; 3]> = owner_entity.and_then(|e| {
            viewer_q
                .get(e)
                .ok()
                .and_then(|v| positions.get(v.0).ok().map(|p| p.as_array()))
                .or(player_pos)
        });

        let deliver =
            |store: &mut KnowledgeStore, report: &ScoutReport, source: ObservationSource| {
                // System knowledge entry.
                store.update(SystemKnowledge {
                    system: report.target_system,
                    observed_at: report.observed_at,
                    received_at: clock.elapsed,
                    data: report.system_snapshot.clone(),
                    source,
                });
                // Ship snapshots.
                for snap in &report.ship_snapshots {
                    let mut snap = snap.clone();
                    // Keep the original observation time; callers set the source
                    // already, but overwrite for safety when the report was
                    // carried home (still Scout).
                    snap.source = source;
                    store.update_ship(snap);
                }
            };

        match report.report_mode {
            ReportMode::FtlComm => {
                // Try instant delivery IF:
                //  1. The ship is still in the target system / at the
                //     observation position (i.e., it hasn't left), AND
                //  2. FTL comm coverage includes both scout pos and empire viewer.
                let at_observation_post = matches!(state, ShipState::InSystem { .. });
                let covered = match empire_viewer_pos {
                    Some(pp) => ftl_comm_covers(ship_pos.as_array(), pp, &coverage),
                    None => false,
                };

                if at_observation_post && covered {
                    if let Some(e) = owner_entity {
                        if let Ok(mut store) = empire_q.get_mut(e) {
                            deliver(&mut store, &report, ObservationSource::Scout);
                        }
                    }
                    commands.entity(ship_entity).remove::<ScoutReport>();
                    info!(
                        "Scout report delivered via FTL Comm: {} -> system {:?}",
                        ship.name, report.target_system
                    );
                    continue;
                }

                // Fallback path: behave like ReportMode::Return. If the ship
                // has already made it home, deliver; otherwise auto-queue
                // move home.
                if let ShipState::InSystem { system } = state {
                    if *system == report.origin_system {
                        if let Some(e) = owner_entity {
                            if let Ok(mut store) = empire_q.get_mut(e) {
                                deliver(&mut store, &report, ObservationSource::Scout);
                            }
                        }
                        commands.entity(ship_entity).remove::<ScoutReport>();
                        info!(
                            "Scout report (FtlComm fallback -> Return) delivered on dock at origin: {} -> system {:?}",
                            ship.name, report.target_system
                        );
                        continue;
                    }
                }
                if !report.return_queued && queue.commands.is_empty() {
                    queue.commands.push(QueuedCommand::MoveTo {
                        system: report.origin_system,
                    });
                    queue.sync_prediction(
                        system_positions
                            .get(report.origin_system)
                            .map(|p| p.as_array())
                            .unwrap_or(ship_pos.as_array()),
                        Some(report.origin_system),
                    );
                    report.return_queued = true;
                    info!(
                        "Scout report: FTL Comm out of range; auto-queuing return for {}",
                        ship.name
                    );
                }
            }
            ReportMode::Return => {
                if let ShipState::InSystem { system } = state {
                    if *system == report.origin_system {
                        if let Some(e) = owner_entity {
                            if let Ok(mut store) = empire_q.get_mut(e) {
                                deliver(&mut store, &report, ObservationSource::Scout);
                            }
                        }
                        commands.entity(ship_entity).remove::<ScoutReport>();
                        info!(
                            "Scout report delivered on dock at origin: {} -> system {:?}",
                            ship.name, report.target_system
                        );
                        continue;
                    }
                }
                if !report.return_queued && queue.commands.is_empty() {
                    queue.commands.push(QueuedCommand::MoveTo {
                        system: report.origin_system,
                    });
                    queue.sync_prediction(
                        system_positions
                            .get(report.origin_system)
                            .map(|p| p.as_array())
                            .unwrap_or(ship_pos.as_array()),
                        Some(report.origin_system),
                    );
                    report.return_queued = true;
                    info!(
                        "Scout report: {} heading home to deliver observation",
                        ship.name
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ftl_comm_covers_requires_both_ends() {
        let relays = vec![RelayCoverageSnapshot {
            source_pos: [0.0, 0.0, 0.0],
            partner_pos: [100.0, 0.0, 0.0],
            source_range: 5.0,
            partner_range: 5.0,
        }];
        // Scout at source, player near partner → covered.
        assert!(ftl_comm_covers([1.0, 0.0, 0.0], [99.0, 0.0, 0.0], &relays));
        // Scout out of source range → not covered.
        assert!(!ftl_comm_covers(
            [10.0, 0.0, 0.0],
            [99.0, 0.0, 0.0],
            &relays
        ));
        // Player out of partner range → not covered.
        assert!(!ftl_comm_covers([1.0, 0.0, 0.0], [50.0, 0.0, 0.0], &relays));
    }

    #[test]
    fn ftl_comm_covers_zero_range_is_infinite() {
        let relays = vec![RelayCoverageSnapshot {
            source_pos: [0.0, 0.0, 0.0],
            partner_pos: [100.0, 0.0, 0.0],
            source_range: 0.0,
            partner_range: 0.0,
        }];
        assert!(ftl_comm_covers(
            [9999.0, 0.0, 0.0],
            [-9999.0, 0.0, 0.0],
            &relays
        ));
    }

    #[test]
    fn ship_has_scout_module_detects_scout() {
        use super::super::EquippedModule;
        use super::super::Owner;
        let ship_no = Ship {
            name: "n".into(),
            design_id: "d".into(),
            hull_id: "h".into(),
            modules: vec![EquippedModule {
                slot_type: "utility".into(),
                module_id: "cargo_bay".into(),
            }],
            owner: Owner::Neutral,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            ruler_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        };
        assert!(!ship_has_scout_module(&ship_no));

        let ship_yes = Ship {
            modules: vec![EquippedModule {
                slot_type: "utility".into(),
                module_id: SCOUT_MODULE_ID.into(),
            }],
            ..ship_no
        };
        assert!(ship_has_scout_module(&ship_yes));
    }
}
