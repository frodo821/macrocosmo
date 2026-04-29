//! #334: command-queue dispatcher.
//!
//! Iterates every ship's `CommandQueue`, peeks the head command, performs
//! **lightweight validation only** (ship is Docked/Loitering, target exists,
//! ship not immobile), emits the corresponding
//! [`CommandRequested`](super::command_events) message, and pops the
//! queue. Phase 3 completes the migration — every `QueuedCommand`
//! variant now has a matching request arm here and a handler under
//! `super::handlers`. There are no legacy fallthroughs.
//!
//! **No state mutation beyond `CommandQueue::commands.remove(0)` and
//! message emit** — all semantic effects (starting FTL travel, spawning
//! route tasks, flipping `ShipState`) happen in the downstream handler
//! systems that read the message. This keeps the dispatcher's query set
//! tiny (well below Bevy's 16-param cap) and frees each handler to hold
//! only the queries *it* needs (plan §2.2, §2.3).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::command_events::{
    ColonizeRequested, CommandId, DeployDeliverableRequested, LoadDeliverableRequested,
    LoadFromScrapyardRequested, MoveRequested, MoveToCoordinatesRequested, NextCommandId,
    ScoutRequested, SurveyRequested, TransferToStructureRequested,
};
use super::routing::PendingRoute;
use super::{CommandQueue, Owner, QueuedCommand, Ship, ShipState};
use crate::communication::{CommandLog, CommandLogEntry};
use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::knowledge::{KnowledgeStore, ShipSnapshotState, compute_ship_projection};
use crate::player::{AboardShip, Empire, EmpireRuler, PlayerEmpire, Ruler, StationedAt};
use crate::time_system::GameClock;

/// Lightweight dispatcher: validates + emits `CommandRequested` messages.
///
/// As of #334 Phase 3 every `QueuedCommand` variant is handled here and
/// consumed by a focused handler under `super::handlers`.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_queued_commands(
    clock: Res<GameClock>,
    mut next_id: ResMut<NextCommandId>,
    // Ships not already mid-route. `PendingRoute` means a MoveTo is already
    // being resolved asynchronously; skip those to preserve the 1-in-flight
    // invariant from the legacy code.
    mut ships: Query<
        (Entity, &Ship, &ShipState, &Position, &mut CommandQueue),
        Without<PendingRoute>,
    >,
    // Read-only target lookup. Name not used here but the filter ensures we
    // only match star-system entities.
    systems: Query<Entity, With<StarSystem>>,
    // Typed message writers — one per Phase-1/2 request variant.
    mut move_req: MessageWriter<MoveRequested>,
    mut move_xy_req: MessageWriter<MoveToCoordinatesRequested>,
    mut load_req: MessageWriter<LoadDeliverableRequested>,
    mut deploy_req: MessageWriter<DeployDeliverableRequested>,
    mut transfer_req: MessageWriter<TransferToStructureRequested>,
    mut scrap_req: MessageWriter<LoadFromScrapyardRequested>,
    mut survey_req: MessageWriter<SurveyRequested>,
    mut colonize_req: MessageWriter<ColonizeRequested>,
    mut scout_req: MessageWriter<ScoutRequested>,
    // #334 Phase 1: append a `Dispatched` entry to the player empire's
    // CommandLog on each successful validation. The bridge system
    // `bridge_command_executed_to_log` finalizes via `CommandId` match.
    // Optional — observer-mode apps without a `PlayerEmpire` skip logging.
    mut command_log_q: Query<&mut CommandLog, With<PlayerEmpire>>,
    // #488: extra ECS access for the defensive dispatcher-side
    // `ShipProjection` write. Bundled into a `SystemParam` to stay
    // under the 16-arg limit.
    mut projection_params: ProjectionWriteParams,
) {
    let mut command_log = command_log_q.single_mut().ok();
    for (ship_entity, ship, state, ship_pos, mut queue) in ships.iter_mut() {
        // Only ships in a state that can accept a new command get dispatched.
        // The legacy code consumed queue items for ships that were Docked or
        // Loitering; mid-travel / mid-survey / mid-settling ships have to
        // finish the current action first. Preserve that exactly.
        let (is_docked, docked_system): (bool, Option<Entity>) = match *state {
            ShipState::InSystem { system } => (true, Some(system)),
            ShipState::Loitering { .. } => (false, None),
            _ => continue,
        };

        if queue.commands.is_empty() {
            continue;
        }

        // #488 / #493: defensive `ShipProjection` write for the **direct
        // CommandQueue append** path (BRP `world.mutate_components`,
        // future plugins, tests, etc.). The 3 civilised dispatch sites
        // (AI outbox / Lua `request_command` / player UI) write their
        // own projection at the *dispatch instant* — that path stays
        // intact and produces light-cone-correct semantics. This
        // dispatcher-side write covers everything that bypassed those
        // sites, so the renderer (#477) never sees an empty
        // `KnowledgeStore.projections` for a queued ship.
        //
        // The guard `get_projection().is_none()` makes the write
        // idempotent: if a caller-side projection exists (= the
        // canonical case), we leave it alone — the caller's
        // dispatch-time light-delay numbers are strictly better than
        // anything we can recompute here.
        //
        // #493: the projection write is moved **inside** each per-variant
        // validation block so it only fires after validation passes (=
        // just before `queue.commands.remove(0)` for the dispatched
        // command). Pre-#493 the write fired before validation, leaking
        // a stale `intended_*` projection whenever the head command was
        // dropped (target gone, immobile ship, no-op same-system MoveTo,
        // ...) — the renderer would keep dashing toward a phantom target
        // until the reconciler eventually cleared it.

        // Peek the head command. We only mutate the queue if this command
        // is a Phase-1 migrated variant AND passes dispatcher validation.
        match &queue.commands[0] {
            QueuedCommand::MoveTo { system: target } => {
                let target = *target;

                // Target system must still exist.
                if systems.get(target).is_err() {
                    warn!(
                        "dispatch: MoveTo target {:?} no longer exists (ship {})",
                        target, ship.name
                    );
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                // Already at target — drop the no-op.
                if is_docked && docked_system == Some(target) {
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                // Immobile ships (Cores, etc.) can never satisfy a MoveTo.
                // Drop with info-level log; UI guard should already prevent
                // this but belt-and-braces per plan §3.1.
                if ship.is_immobile() {
                    info!(
                        "dispatch: dropping MoveTo on immobile ship {} (no propulsion)",
                        ship.name
                    );
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                // Validation passed → write the defensive projection
                // (#493) and emit the request.
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id: CommandId = next_id.allocate();
                queue.commands.remove(0);
                move_req.write(MoveRequested {
                    command_id,
                    ship: ship_entity,
                    target,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → MoveTo {:?}", ship.name, target),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} MoveRequested -> {:?} (cmd {})",
                    ship.name, target, command_id.0
                );
            }
            QueuedCommand::MoveToCoordinates { target } => {
                let target_arr = *target;
                // Immobile ships cannot MoveToCoordinates either.
                if ship.is_immobile() {
                    info!(
                        "dispatch: dropping MoveToCoordinates on immobile ship {}",
                        ship.name
                    );
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                // Validation passed → defensive projection write (#493).
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                move_xy_req.write(MoveToCoordinatesRequested {
                    command_id,
                    ship: ship_entity,
                    target: target_arr,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!(
                            "{} → MoveToCoordinates ({:.2},{:.2},{:.2})",
                            ship.name, target_arr[0], target_arr[1], target_arr[2]
                        ),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} MoveToCoordinatesRequested -> ({:.2},{:.2},{:.2}) (cmd {})",
                    ship.name, target_arr[0], target_arr[1], target_arr[2], command_id.0
                );
            }
            QueuedCommand::LoadDeliverable {
                system,
                stockpile_index,
            } => {
                let system = *system;
                let stockpile_index = *stockpile_index;
                // No per-variant validation drops; defensive projection
                // write (#493) is a no-op for spatial-less commands but
                // the call is uniform across variants.
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                load_req.write(LoadDeliverableRequested {
                    command_id,
                    ship: ship_entity,
                    system,
                    stockpile_index,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → LoadDeliverable [{}]", ship.name, stockpile_index),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} LoadDeliverableRequested system={:?} idx={} (cmd {})",
                    ship.name, system, stockpile_index, command_id.0
                );
            }
            QueuedCommand::DeployDeliverable {
                position,
                item_index,
            } => {
                let position = *position;
                let item_index = *item_index;
                // Spatial-less, projection write is a no-op (#493).
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                deploy_req.write(DeployDeliverableRequested {
                    command_id,
                    ship: ship_entity,
                    position,
                    item_index,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!(
                            "{} → DeployDeliverable [{}] at ({:.2},{:.2},{:.2})",
                            ship.name, item_index, position[0], position[1], position[2]
                        ),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} DeployDeliverableRequested idx={} (cmd {})",
                    ship.name, item_index, command_id.0
                );
            }
            QueuedCommand::TransferToStructure {
                structure,
                minerals,
                energy,
            } => {
                let structure = *structure;
                let minerals = *minerals;
                let energy = *energy;
                // Spatial-less, projection write is a no-op (#493).
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                transfer_req.write(TransferToStructureRequested {
                    command_id,
                    ship: ship_entity,
                    structure,
                    minerals,
                    energy,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!(
                            "{} → TransferToStructure {:?} ({}m/{}e)",
                            ship.name,
                            structure,
                            minerals.to_f64(),
                            energy.to_f64()
                        ),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} TransferToStructureRequested -> {:?} (cmd {})",
                    ship.name, structure, command_id.0
                );
            }
            QueuedCommand::LoadFromScrapyard { structure } => {
                let structure = *structure;
                // Spatial-less, projection write is a no-op (#493).
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                scrap_req.write(LoadFromScrapyardRequested {
                    command_id,
                    ship: ship_entity,
                    structure,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → LoadFromScrapyard {:?}", ship.name, structure),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} LoadFromScrapyardRequested -> {:?} (cmd {})",
                    ship.name, structure, command_id.0
                );
            }
            QueuedCommand::Survey { system: target } => {
                let target = *target;
                // No per-variant validation; defensive projection write (#493).
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                survey_req.write(SurveyRequested {
                    command_id,
                    ship: ship_entity,
                    target_system: target,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → Survey {:?}", ship.name, target),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} SurveyRequested -> {:?} (cmd {})",
                    ship.name, target, command_id.0
                );
            }
            QueuedCommand::Colonize {
                system: target,
                planet,
            } => {
                let target = *target;
                let planet = *planet;
                // No per-variant validation; defensive projection write (#493).
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                colonize_req.write(ColonizeRequested {
                    command_id,
                    ship: ship_entity,
                    target_system: target,
                    planet,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → Colonize {:?}", ship.name, target),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} ColonizeRequested -> {:?} (cmd {})",
                    ship.name, target, command_id.0
                );
            }
            QueuedCommand::Scout {
                target_system,
                observation_duration,
                report_mode,
            } => {
                let target_system = *target_system;
                let observation_duration = *observation_duration;
                let report_mode = *report_mode;
                // No per-variant validation; defensive projection write (#493).
                maybe_write_dispatcher_projection(
                    ship_entity,
                    ship,
                    &queue.commands[0],
                    ship_pos.as_array(),
                    docked_system,
                    clock.elapsed,
                    &mut projection_params,
                );
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                scout_req.write(ScoutRequested {
                    command_id,
                    ship: ship_entity,
                    target_system,
                    observation_duration,
                    report_mode,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → Scout {:?}", ship.name, target_system),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} ScoutRequested -> {:?} (cmd {})",
                    ship.name, target_system, command_id.0
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// #488: defensive dispatcher-side `ShipProjection` write.
//
// Pre-#488, the 3 civilised dispatch sites (AI outbox / Lua / player UI)
// each wrote a projection at the **dispatch instant** (= correct light-cone
// semantics from the dispatcher's POV). But anything that appended to
// `CommandQueue` outside those 3 paths — BRP `world.mutate_components`,
// new plugins, tests, etc. — bypassed the projection write. Without a
// projection, the Galaxy Map renderer (#477) shows nothing → ship
// vanishes.
//
// The defensive write below runs at queue-processing time and only
// fires when the issuing empire's `KnowledgeStore` does not already
// have a projection for the ship. Caller-side writes always take
// precedence — they have access to the caller's exact dispatch tick
// and Ruler position, while the dispatcher only knows "now" and the
// empire's ruler.
// ---------------------------------------------------------------------------

/// SystemParam bundle holding the extra ECS access the dispatcher needs
/// to perform the #488 defensive `ShipProjection` write. Bundled because
/// `dispatch_queued_commands` is already at 14 params (1 over Bevy's 16
/// soft limit if we expanded inline).
#[derive(SystemParam)]
pub struct ProjectionWriteParams<'w, 's> {
    /// `Empire → Ruler` chain — used to locate the ruler entity, which
    /// stands in for "the dispatcher" when `dispatch_queued_commands`
    /// processes a direct-CommandQueue-append.
    pub empire_rulers: Query<'w, 's, &'static EmpireRuler, With<Empire>>,
    /// Ruler's world location — `StationedAt` for system-bound,
    /// `AboardShip` for ship-bound. Position is read off the system /
    /// ship entity via [`positions`].
    pub rulers:
        Query<'w, 's, (Option<&'static StationedAt>, Option<&'static AboardShip>), With<Ruler>>,
    /// Per-system positions used for `target_system_pos`, the fallback
    /// for `ship_pos` (when no `ShipSnapshot` exists), AND the Ruler's
    /// `StationedAt` lookup in [`resolve_dispatcher_pos`]. The
    /// `Without<Ship>` guard avoids overlap with the dispatcher's
    /// mutable ship query. In practice the Ruler's location is a
    /// `StarSystem` entity (StationedAt) or a Ship entity (AboardShip);
    /// for `AboardShip` we fall through to `dispatcher_pos = ship_pos`
    /// since reading a ship's `Position` here would conflict with the
    /// dispatcher's mutable query. (#497: previously a separate
    /// `ruler_system_positions` query of identical type — collapsed
    /// for hygiene.)
    pub star_positions: Query<'w, 's, &'static Position, (With<StarSystem>, Without<Ship>)>,
    /// Per-empire `KnowledgeStore` — read for snapshot lookup +
    /// projection idempotency check, and mutated for the actual
    /// projection write.
    pub knowledge_stores: Query<'w, 's, &'static mut KnowledgeStore, With<Empire>>,
}

/// Map a queued command to the [`ShipSnapshotState`] it implies once the
/// command takes effect. Mirrors
/// [`crate::knowledge::command_kind_to_intended_state`] but works
/// directly on the typed `QueuedCommand` enum so the dispatcher does
/// not have to round-trip through the AI command-kind string namespace.
///
/// Returns `None` for cargo / structure variants — those produce no
/// `ShipState` change once executed, so the projection write skips them
/// (matches the spatial-less skip in
/// `scripting::gamestate_scope::write_lua_dispatch_projection`).
pub fn queued_command_intended_state(cmd: &QueuedCommand) -> Option<ShipSnapshotState> {
    match cmd {
        // #491 (D-H-4): The dispatcher cannot know FTL vs SubLight at
        // command-issue time — route planning is async (see
        // `crate::ship::routing::PendingRoute` /
        // `poll_pending_routes`). The intended-state layer is seeded
        // here with `InTransitSubLight` as the conservative
        // placeholder; once the A* route plan completes,
        // `poll_pending_routes` upgrades the projection's
        // `intended_state` to `InTransitFTL` when `segments[0]` is an
        // FTL hop. No `KnowledgeFact::ShipDeparted` is required — the
        // dispatching empire's belief is self-updated, not observed
        // (the player UI must surface the FTL distinction the moment
        // the route plan resolves, well before any light-coherent
        // observation could arrive).
        QueuedCommand::MoveTo { .. } | QueuedCommand::MoveToCoordinates { .. } => {
            Some(ShipSnapshotState::InTransitSubLight)
        }
        QueuedCommand::Survey { .. } | QueuedCommand::Scout { .. } => {
            Some(ShipSnapshotState::Surveying)
        }
        QueuedCommand::Colonize { .. } => Some(ShipSnapshotState::Settling),
        QueuedCommand::LoadDeliverable { .. }
        | QueuedCommand::DeployDeliverable { .. }
        | QueuedCommand::TransferToStructure { .. }
        | QueuedCommand::LoadFromScrapyard { .. } => None,
    }
}

/// Extract the target [`StarSystem`] entity from a queued command, if any.
/// `MoveToCoordinates` and `DeployDeliverable` carry deep-space coords
/// (no star system) — the projection still writes with `intended_system =
/// None` and `target_system_pos = None`.
pub fn queued_command_target_system(cmd: &QueuedCommand) -> Option<Entity> {
    match cmd {
        QueuedCommand::MoveTo { system } => Some(*system),
        QueuedCommand::Survey { system } => Some(*system),
        QueuedCommand::Colonize { system, .. } => Some(*system),
        QueuedCommand::Scout { target_system, .. } => Some(*target_system),
        QueuedCommand::LoadDeliverable { system, .. } => Some(*system),
        QueuedCommand::MoveToCoordinates { .. }
        | QueuedCommand::DeployDeliverable { .. }
        | QueuedCommand::TransferToStructure { .. }
        | QueuedCommand::LoadFromScrapyard { .. } => None,
    }
}

/// Whether the command implies the ship returns to its origin after
/// completion. Mirrors
/// [`crate::knowledge::command_kind_has_return_leg`] — only
/// survey / scout missions have a return leg.
pub fn queued_command_has_return_leg(cmd: &QueuedCommand) -> bool {
    matches!(
        cmd,
        QueuedCommand::Survey { .. } | QueuedCommand::Scout { .. }
    )
}

/// #488: defensive projection write invoked once per ship per tick
/// inside `dispatch_queued_commands`.
///
/// Skips when:
/// * Ship has `Owner::Neutral` (mirrors `seed_own_ship_projections`'s
///   neutral skip — hostile / pirate factions never seed any empire's
///   projection store).
/// * The command is spatial-less (cargo / structure ops with no
///   `ShipState` implication) — `queued_command_intended_state`
///   returns `None` — AND the existing projection's `intended_*` is
///   already `None` (= seed / post-reconcile steady state matches
///   "no mission"; nothing to write).
/// * The owning empire's existing projection's `intended_state` /
///   `intended_system` already match what this head queued command
///   implies (= a caller-side dispatch site wrote an accurate
///   projection at the dispatch instant; their numbers are strictly
///   better than what we can recompute here, so leave it alone).
///
/// Per #492: the previous `is_some()` guard was dead-on-arrival —
/// `seed_own_ship_projections` (#481) installs a seed projection
/// (`intended_state: None`) at every own-empire ship's spawn, which
/// caused the guard to always fire and the defensive write to never
/// run. The head-command-aware match below correctly overwrites
/// (a) the seed, (b) post-reconcile cleared-intended state, and
/// (c) stale caller writes whose mission diverges from the current
/// queue head, while still preserving fresh caller writes.
fn maybe_write_dispatcher_projection(
    ship_entity: Entity,
    ship: &Ship,
    cmd: &QueuedCommand,
    ship_pos_arr: [f64; 3],
    docked_system: Option<Entity>,
    now: i64,
    params: &mut ProjectionWriteParams,
) {
    // Hostile / Neutral ships — never seed an empire's projection.
    let owner_empire = match ship.owner {
        Owner::Empire(e) => e,
        Owner::Neutral => return,
    };

    let intended_state = queued_command_intended_state(cmd);
    let intended_system = queued_command_target_system(cmd);

    // Head-command-aware idempotency guard (#492). Compare the
    // existing projection's `intended_state` / `intended_system`
    // against what the current head command implies — only skip when
    // they match. This correctly:
    // * overwrites the seed projection (`intended_state: None`) when
    //   the head command has a non-None intended,
    // * overwrites post-reconcile state (intended_* cleared) when a
    //   new mission has been queued,
    // * overwrites stale caller writes whose mission no longer
    //   matches the current queue head,
    // * preserves fresh caller writes for the same mission.
    let existing_matches = match params.knowledge_stores.get(owner_empire) {
        Ok(store) => store
            .get_projection(ship_entity)
            .map(|p| p.intended_state == intended_state && p.intended_system == intended_system)
            .unwrap_or(false),
        Err(_) => {
            // Owning empire missing or has no `KnowledgeStore` —
            // nothing to write into.
            return;
        }
    };
    if existing_matches {
        return;
    }

    // Skip spatial-less commands (cargo ops). They have no ShipState
    // implication once executed. We only get here when the existing
    // projection (if any) does NOT already match `intended_state =
    // None / intended_system = None` — but for a spatial-less head
    // command, writing a new projection with `intended_state = None`
    // would still leak (e.g.) a stale caller's intended fields into
    // an unrelated mission. The safest behaviour matches the
    // pre-#492 contract: skip the dispatcher write entirely for
    // spatial-less commands. The renderer treats `intended_*: None`
    // as "no in-flight mission", which is correct for cargo ops.
    if intended_state.is_none() {
        return;
    }

    let target_system_pos =
        intended_system.and_then(|sys| params.star_positions.get(sys).ok().map(|p| p.as_array()));
    let has_return_leg = queued_command_has_return_leg(cmd);

    // Resolve the dispatcher's reference position via the empire's
    // Ruler. We read the Ruler's `StationedAt` system position; if
    // the Ruler is `AboardShip`, the ruler-aboard ship's `Position`
    // would conflict with the dispatcher's mutable `Ship` query, so
    // we fall back to the queued ship's known position (the next
    // most-local proxy for "where the dispatch instruction
    // originated"). This is a best-effort approximation — the 3
    // civilised dispatch sites (which always run *first* on the
    // canonical path) supply the exact Ruler position via their
    // own writes.
    let dispatcher_pos = resolve_dispatcher_pos(owner_empire, ship_pos_arr, params);

    // Use the dispatcher's last-known snapshot of the ship if it
    // exists. The fallback projected_system is the ship's home_port —
    // matches `seed_own_ship_projections`'s conservative default.
    let snapshot = params
        .knowledge_stores
        .get(owner_empire)
        .ok()
        .and_then(|store| store.get_ship(ship_entity).cloned());

    let fallback_system = Some(ship.home_port);

    // Ship position the dispatcher *believes*. Drawn from snapshot
    // (last_known_system position or Loitering coord) when present;
    // from `fallback_system` (home_port) position as last resort.
    // For the no-snapshot, no-home_port-position case we fall back to
    // the ship's currently-docked system position (`docked_system`)
    // since at this point the dispatcher has already verified the
    // ship is in `InSystem`/`Loitering` state via the docked-state
    // pattern match.
    let ship_pos = match snapshot.as_ref().map(|s| &s.last_known_state) {
        Some(ShipSnapshotState::Loitering { position }) => *position,
        _ => snapshot
            .as_ref()
            .and_then(|s| s.last_known_system)
            .or(fallback_system)
            .or(docked_system)
            .and_then(|sys| params.star_positions.get(sys).ok().map(|p| p.as_array()))
            .unwrap_or(ship_pos_arr),
    };

    let projection = compute_ship_projection(
        ship_entity,
        snapshot.as_ref(),
        dispatcher_pos,
        ship_pos,
        target_system_pos,
        intended_state,
        intended_system,
        has_return_leg,
        fallback_system,
        now,
    );

    if let Ok(mut store) = params.knowledge_stores.get_mut(owner_empire) {
        store.update_projection(projection);
    }
}

/// Resolve the empire Ruler's world-space position. Used as the
/// `dispatcher_pos` input to [`compute_ship_projection`] for the #488
/// fallback path.
///
/// `AboardShip` rulers cannot be resolved here — reading the
/// ruler-ship's `Position` would conflict with the dispatcher's
/// mutable `Ship` query — so we fall back to `ship_pos_arr` (the
/// queued ship's own believed position) for that case. The 3 civilised
/// dispatch sites supply the exact AboardShip Ruler position via their
/// own pre-dispatch writes, so this fallback is only exercised when the
/// canonical path was bypassed.
fn resolve_dispatcher_pos(
    empire: Entity,
    ship_pos_arr: [f64; 3],
    params: &ProjectionWriteParams,
) -> [f64; 3] {
    let Ok(empire_ruler) = params.empire_rulers.get(empire) else {
        return ship_pos_arr;
    };
    let Ok((stationed, _aboard)) = params.rulers.get(empire_ruler.0) else {
        return ship_pos_arr;
    };
    if let Some(stationed) = stationed {
        if let Ok(pos) = params.star_positions.get(stationed.system) {
            return pos.as_array();
        }
    }
    // AboardShip path: handled implicitly via fallback (cannot read
    // ship Position without query conflict). Return the queued ship's
    // own position as the most-local proxy.
    ship_pos_arr
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::ship::command_events::CommandEventsPlugin;
    use crate::ship::{ShipHitpoints, ShipModifiers, ShipStats};
    use bevy::MinimalPlugins;
    use bevy::ecs::message::Messages;

    fn dummy_home_port(world: &mut World) -> Entity {
        // Spawn a harmless placeholder entity just so `Ship.home_port`
        // references a valid Entity id. The dispatcher never resolves it.
        world.spawn_empty().id()
    }

    fn spawn_test_ship(
        world: &mut World,
        pos: [f64; 3],
        docked_system: Option<Entity>,
        sublight_speed: f64,
        ftl_range: f64,
    ) -> Entity {
        let home_port = dummy_home_port(world);
        let state = match docked_system {
            Some(system) => ShipState::InSystem { system },
            None => ShipState::Loitering { position: pos },
        };
        world
            .spawn((
                Ship {
                    name: "T".into(),
                    design_id: "test".into(),
                    hull_id: "hull".into(),
                    modules: vec![],
                    owner: Owner::Neutral,
                    sublight_speed,
                    ftl_range,
                    ruler_aboard: false,
                    home_port,
                    design_revision: 0,
                    fleet: None,
                },
                state,
                Position::from(pos),
                CommandQueue::default(),
                crate::ship::Cargo::default(),
                ShipHitpoints {
                    hull: 10.0,
                    hull_max: 10.0,
                    armor: 0.0,
                    armor_max: 0.0,
                    shield: 0.0,
                    shield_max: 0.0,
                    shield_regen: 0.0,
                },
                ShipModifiers::default(),
                ShipStats::default(),
            ))
            .id()
    }

    fn spawn_test_system(world: &mut World, pos: [f64; 3]) -> Entity {
        world
            .spawn((
                StarSystem {
                    name: "S".into(),
                    surveyed: true,
                    is_capital: false,
                    star_type: "g2v".into(),
                },
                Position::from(pos),
            ))
            .id()
    }

    fn make_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(0));
        app.add_plugins(CommandEventsPlugin);
        app.add_systems(Update, dispatch_queued_commands);
        app
    }

    #[test]
    fn dispatches_move_to_emits_request_and_pops_queue() {
        let mut app = make_app();
        let target = spawn_test_system(app.world_mut(), [5.0, 0.0, 0.0]);
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: target });
        }
        app.update();

        // Message emitted with matching ship + target
        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        let all: Vec<&MoveRequested> = cursor.read(messages).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].ship, ship);
        assert_eq!(all[0].target, target);
        assert_ne!(all[0].command_id, CommandId::ZERO);

        // Queue is now empty — command popped.
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatcher_rejects_move_to_for_immobile_ship() {
        let mut app = make_app();
        let target = spawn_test_system(app.world_mut(), [5.0, 0.0, 0.0]);
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        // Immobile: 0 sublight, 0 ftl_range.
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.0, 0.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: target });
        }
        app.update();

        // No message, queue cleared.
        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0);
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatcher_drops_already_at_target() {
        let mut app = make_app();
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: origin });
        }
        app.update();

        // Already at target → queue cleared, no message.
        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0);
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatcher_drops_nonexistent_target() {
        let mut app = make_app();
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let phantom = Entity::from_raw_u32(9999).unwrap();
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: phantom });
        }
        app.update();

        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0);
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatches_move_to_coordinates_and_pops() {
        let mut app = make_app();
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveToCoordinates {
                target: [3.0, 4.0, 0.0],
            });
        }
        app.update();

        let messages = app
            .world()
            .resource::<Messages<MoveToCoordinatesRequested>>();
        let mut cursor = messages.get_cursor();
        let all: Vec<&MoveToCoordinatesRequested> = cursor.read(messages).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].target, [3.0, 4.0, 0.0]);
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatches_scout_emits_request_and_pops_queue() {
        // #334 Phase 3 (Commit 1): Scout migrated to the handler pipeline.
        // The dispatcher now emits `ScoutRequested` and pops the queue —
        // there are no remaining non-migrated variants.
        use crate::ship::ReportMode;
        let mut app = make_app();
        let target = spawn_test_system(app.world_mut(), [5.0, 0.0, 0.0]);
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::Scout {
                target_system: target,
                observation_duration: 10,
                report_mode: ReportMode::Return,
            });
        }
        app.update();

        let messages = app.world().resource::<Messages<ScoutRequested>>();
        let mut cursor = messages.get_cursor();
        let all: Vec<&ScoutRequested> = cursor.read(messages).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].ship, ship);
        assert_eq!(all[0].target_system, target);
        assert_eq!(all[0].observation_duration, 10);
        assert!(matches!(all[0].report_mode, ReportMode::Return));
        assert_ne!(all[0].command_id, CommandId::ZERO);

        // Queue is now empty — command popped.
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatcher_fifo_across_multiple_ships() {
        // Plan §6: verify per-ship FIFO + cross-ship emit order.
        let mut app = make_app();
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let t1 = spawn_test_system(app.world_mut(), [5.0, 0.0, 0.0]);
        let t2 = spawn_test_system(app.world_mut(), [6.0, 0.0, 0.0]);
        let ship_a = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        let ship_b = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship_a).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: t1 });
        }
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship_b).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: t2 });
        }
        app.update();

        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        let all: Vec<&MoveRequested> = cursor.read(messages).collect();
        assert_eq!(all.len(), 2);
        // Command ids must be strictly monotonic.
        assert!(all[0].command_id < all[1].command_id);
    }
}
