use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{Colony, SlotAssignment};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::{KnowledgeStore, ShipSnapshotState};
use crate::physics;
use crate::player::{AboardShip, Player, StationedAt};
use crate::ship::{
    Cargo, CommandQueue, Owner, QueuedCommand, Ship, ShipHitpoints, ShipState, SurveyData,
};
use crate::ship_design::ShipDesignRegistry;
use crate::technology::GlobalParams;
use crate::time_system::GameClock;
use crate::ui::UiElementRegistry;
use crate::ui::ship_view::ship_view;
use crate::visualization::{SelectedShip, SelectedShips};

/// #482: Map a [`QueuedCommand`] to the equivalent [`crate::ship::ShipCommand`]
/// for the player-dispatch projection writer (`write_player_dispatch_projection`
/// in `ui/mod.rs`). Only spatial commands are mapped — spatial-less variants
/// return `None` and their dispatch is intentionally skipped by the
/// projection path (`SetROE`, etc.).
pub fn queued_command_to_ship_command(qc: &QueuedCommand) -> Option<crate::ship::ShipCommand> {
    match qc {
        QueuedCommand::MoveTo { system } => Some(crate::ship::ShipCommand::MoveTo {
            destination: *system,
        }),
        QueuedCommand::Survey { system } => {
            Some(crate::ship::ShipCommand::Survey { target: *system })
        }
        QueuedCommand::Colonize { .. } => Some(crate::ship::ShipCommand::Colonize),
        // Other variants (Fortify, Blockade, etc.) — not spatial-less, but
        // not currently emitted by the context menu path. If a future menu
        // entry adds one, extend this match.
        _ => None,
    }
}

/// #389: Pre-computed harbour info for context menu display.
pub struct HarbourInfo {
    pub entity: Entity,
    pub name: String,
    pub can_dock: bool,
}

/// #491 (PR-3): Light-coherent context-menu inputs derived from a ship's
/// `ShipView` instead of the realtime ECS `ShipState`.
///
/// Returned by [`compute_context_menu_ship_data`]. All fields are derived
/// from the viewing empire's projection (own ship) / snapshot (foreign
/// ship) / realtime fallback (no-store) — never the ship's realtime
/// `ShipState` directly. This collapses the FTL leak surface in the
/// context-menu path to a single helper.
///
/// `remaining_travel` is the *projection-mediated* time-to-arrival —
/// `expected_arrival_at - clock.elapsed`, clamped at zero. For non-transit
/// projection states (e.g. steady-state `InSystem`, or projections with
/// `expected_arrival_at = None`) this is `0`.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuShipData {
    /// `Some(system)` when the ship's *projected* state is `InSystem`.
    pub docked_system: Option<Entity>,
    /// The ship's projected target / current system, when the ship is
    /// in transit / surveying / settling / refitting. `None` for
    /// `InSystem` / `Loitering` / terminal states.
    pub current_destination_system: Option<Entity>,
    /// Deep-space coordinate when the projection reports `Loitering`.
    /// `None` for all other states.
    pub loitering_pos: Option<[f64; 3]>,
    /// Projected time-to-arrival in hexadies. `0` when the ship is
    /// `is_docked` or has no in-flight expected arrival.
    pub remaining_travel: i64,
    /// `true` iff the projection reports `InSystem`.
    pub is_docked: bool,
}

/// #491 (PR-3): Compute the projection-mediated context-menu data for a
/// ship. Routes own-empire ships through `ShipProjection` and foreign
/// ships through `ShipSnapshot` via the [`ship_view`] helper, eliminating
/// the realtime `ShipState` reads that previously leaked FTL/transit
/// state into the context menu (#491).
///
/// # Semantics
///
/// * `docked_system` / `is_docked` — derived from `ShipView::state` via
///   `ShipSnapshotState::InSystem`. The ship's *projected* docking
///   state, not its realtime ECS state.
/// * `current_destination_system` — derived from `ShipView::system` for
///   transit / survey / settle / refit projection states.
/// * `loitering_pos` — extracted via `ShipView::position()`.
/// * `remaining_travel` — `ShipProjection.expected_arrival_at -
///   clock.elapsed`, clamped at zero. Light-coherent with the dispatcher's
///   timeline (= what the player believes about the ship's arrival).
///
/// # Fallback
///
/// When `ship_view` returns `None` (= the viewing empire has no
/// projection / snapshot for the ship — e.g. before a seed projection
/// lands), this returns `None`. The caller treats that as "close menu".
pub fn compute_context_menu_ship_data(
    ship_entity: Entity,
    ship: &Ship,
    realtime_state: &ShipState,
    clock: &GameClock,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) -> Option<ContextMenuShipData> {
    let view = ship_view(
        ship_entity,
        ship,
        realtime_state,
        viewing_knowledge,
        viewing_empire,
    )?;
    // #491 Stage-2 follow-up: terminal projections (Destroyed / Missing)
    // must not surface a context menu — the player should not be able to
    // dispatch MoveTo / Survey / Colonize against a ship the empire
    // already believes is gone. Return `None` so the caller closes the
    // menu, the same outcome as a missing projection (= "menu has no
    // valid target, bail").
    if !view.is_actionable() {
        return None;
    }
    let docked_system = match view.state {
        ShipSnapshotState::InSystem => view.system,
        _ => None,
    };
    // For non-`InSystem` states, the `view.system` is the destination /
    // target / planet-bearing system. `Loitering` / `Destroyed` /
    // `Missing` carry `view.system = None`, so this naturally yields
    // `None` for those. `InSystem` is handled via `docked_system` above
    // (current_destination_system stays `None`).
    let current_destination_system = match view.state {
        ShipSnapshotState::InSystem | ShipSnapshotState::Destroyed | ShipSnapshotState::Missing => {
            None
        }
        ShipSnapshotState::Loitering { .. } => None,
        _ => view.system,
    };
    let loitering_pos = view.position();
    let is_docked = matches!(view.state, ShipSnapshotState::InSystem);
    // #491 (PR-3): `remaining_travel` is the dispatcher's projected
    // time-to-arrival. For docked ships it's irrelevant (forced to 0).
    // For projections with no `expected_arrival_at` (e.g. steady-state
    // `InSystem`, or spatial-less commands) it's also 0. This replaces
    // the previous realtime-`ShipState`-driven `arrival_at` /
    // `completes_at` reads.
    let remaining_travel = if is_docked {
        0
    } else {
        viewing_knowledge
            .and_then(|k| k.get_projection(ship_entity))
            .and_then(|p| p.expected_arrival_at)
            .map(|eta| (eta - clock.elapsed).max(0))
            .unwrap_or(0)
    };
    Some(ContextMenuShipData {
        docked_system,
        current_destination_system,
        loitering_pos,
        remaining_travel,
        is_docked,
    })
}

/// Apply a command directly to a ship's [`ShipState`].
///
/// **Pre-condition**: caller has verified the command target ship is at
/// zero light-delay distance from the issuer (= same system, docked, or
/// otherwise locally addressable). This is the local-write path used by
/// `draw_context_menu` after the delay calculation has resolved to 0.
///
/// **NEVER** call this for remote commands — the [`PendingShipCommand`]
/// path is mandatory for nonzero delay, otherwise the empire's command
/// would bypass light-speed transport.
///
/// The `expected_delay` argument exists purely to encode that
/// pre-condition in the type system: it is checked via
/// [`debug_assert_eq!`] so dev/test builds catch any regression where a
/// nonzero-delay command is routed through the local path.
///
/// Visibility is `pub` (rather than `pub(crate)`) so integration tests
/// can exercise the assertion directly without going through the full
/// egui pipeline. Treat it as an internal helper — production callers
/// outside this module should not exist.
///
/// Returns `true` iff the ship entity was found and its state was
/// written. Callers use this to gate dependent side effects (e.g.
/// clearing `SelectedShip`) so a despawn mid-frame leaves selection
/// untouched, matching pre-#462 behavior.
///
/// [`PendingShipCommand`]: crate::ship::PendingShipCommand
#[doc(hidden)]
pub fn apply_local_ship_command(
    ship: Entity,
    new_state: ShipState,
    expected_delay: i64,
    ships_query: &mut Query<
        (
            Entity,
            &mut Ship,
            &mut ShipState,
            Option<&mut Cargo>,
            &ShipHitpoints,
            Option<&SurveyData>,
        ),
        Without<SlotAssignment>,
    >,
) -> bool {
    debug_assert_eq!(
        expected_delay, 0,
        "apply_local_ship_command called with non-zero delay {} \
         — must use PendingShipCommand path instead",
        expected_delay
    );
    if let Ok((_, _, mut state, _, _, _)) = ships_query.get_mut(ship) {
        *state = new_state;
        true
    } else {
        false
    }
}

/// #389: Actions that come out of the context menu requiring Commands access.
pub struct ContextMenuActions {
    /// Dock ship at harbour. Payload: (ship, harbour).
    pub dock_at: Option<(Entity, Entity)>,
    /// #482: Zero-delay player dispatches that need a `ShipProjection`
    /// write at the dispatch tick. Filled by [`draw_context_menu`] for
    /// each branch that pushes a `QueuedCommand` directly to the ship's
    /// `CommandQueue` (or applies the state in place) without going
    /// through the `PendingShipCommand` pipeline. The caller drains
    /// these and runs the same `write_player_dispatch_projection`
    /// closure that `pending_ship_commands` uses, preserving the
    /// "dispatch-time projection write" invariant from epic #473 / #475.
    ///
    /// Each entry is `(ship, command)` — `ship` is the dispatch target,
    /// `command` is the equivalent `ShipCommand` for projection-mapping
    /// purposes (`MoveTo`, `Survey`, `Colonize`). Spatial-less commands
    /// are not pushed here.
    pub zero_delay_dispatches: Vec<(Entity, crate::ship::ShipCommand)>,
}

/// Draws the RTS-style context menu when a ship is selected and a star is clicked.
/// #76: Commands are delayed by light-speed distance from player to ship.
#[allow(clippy::too_many_arguments)]
pub fn draw_context_menu(
    ctx: &egui::Context,
    context_menu: &mut crate::visualization::ContextMenu,
    selected_ship: &mut SelectedShip,
    selected_ships: &mut SelectedShips,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    ships_query: &mut Query<
        (
            Entity,
            &mut Ship,
            &mut ShipState,
            Option<&mut Cargo>,
            &ShipHitpoints,
            Option<&SurveyData>,
        ),
        Without<SlotAssignment>,
    >,
    command_queues: &mut Query<&mut CommandQueue>,
    positions: &Query<&Position>,
    clock: &GameClock,
    global_params: &GlobalParams,
    player_q: &Query<(Entity, &StationedAt, Option<&AboardShip>), With<Player>>,
    pending_commands_out: &mut Vec<crate::ship::PendingShipCommand>,
    colonies: &[Colony],
    planets: &Query<&Planet>,
    planet_entities: &Query<(Entity, &Planet, Option<&SystemAttributes>)>,
    hostile_systems: &std::collections::HashSet<Entity>,
    design_registry: &ShipDesignRegistry,
    // #299 (S-5): (system_entity, faction_entity) pairs for all Core ships.
    core_by_system: &[(Entity, Entity)],
    mut ui_registry: Option<&mut UiElementRegistry>,
    // #389: Harbours in the target system that the selected ship could dock at.
    target_harbours: &[HarbourInfo],
    // #389: Whether the selected ship is already docked at a harbour.
    ship_is_docked_at_harbour: bool,
    // #491 (PR-3): Viewing empire's KnowledgeStore for projection-mediated
    // ship-state reads. `None` falls back to realtime ECS (= early Startup
    // before empires are wired, observer god-view future work). The
    // existing #432 caller-side guard suppresses the menu for non-owned
    // ships, so the realistic `Some(...)` case is `viewing_empire ==
    // ship.owner` (= projection-driven path).
    viewing_knowledge: Option<&KnowledgeStore>,
    // #491 (PR-3): Empire whose perspective the player is currently
    // viewing — `PlayerEmpire` in normal play, observed empire in
    // observer mode. Resolved at the call site (`ui/mod.rs`).
    viewing_empire: Option<Entity>,
) -> ContextMenuActions {
    let mut ctx_actions = ContextMenuActions {
        dock_at: None,
        zero_delay_dispatches: Vec::new(),
    };
    if !context_menu.open {
        return ctx_actions;
    }

    let Some(ship_entity) = selected_ship.0 else {
        context_menu.open = false;
        return ctx_actions;
    };

    let Some(target_entity) = context_menu.target_system else {
        context_menu.open = false;
        return ctx_actions;
    };

    // Collect ship data — projection-mediated per #491 (PR-3): own-empire
    // ships read `ShipProjection`, foreign ships read `ShipSnapshot`,
    // no-store falls back to realtime ECS state. The previous realtime
    // `ShipState` reads (`InFTL { arrival_at }` / `SubLight {
    // target_system }` / etc.) are gone — they were the FTL-leak surface
    // this PR closes. See [`compute_context_menu_ship_data`] for the
    // helper contract.
    let (
        ship_name,
        design_id,
        ftl_range,
        sublight_speed,
        docked_system,
        current_destination_system,
        loitering_pos,
        ship_immobile,
        ship_owner,
        remaining_travel,
        is_docked,
    ) = {
        let Ok((_, ship, state, _, _, _)) = ships_query.get(ship_entity) else {
            context_menu.open = false;
            return ctx_actions;
        };
        let Some(view_data) = compute_context_menu_ship_data(
            ship_entity,
            &ship,
            &state,
            clock,
            viewing_knowledge,
            viewing_empire,
        ) else {
            // No projection / snapshot resolvable for this ship — close
            // the menu rather than fall through with stale realtime data.
            context_menu.open = false;
            return ctx_actions;
        };
        (
            ship.name.clone(),
            ship.design_id.clone(),
            ship.ftl_range,
            ship.sublight_speed,
            view_data.docked_system,
            view_data.current_destination_system,
            view_data.loitering_pos,
            // #296: cache immobility so the MoveTo guard below stays a
            // simple boolean.
            ship.is_immobile(),
            // #299 (S-5): ship faction for Core-presence check.
            ship.owner,
            view_data.remaining_travel,
            view_data.is_docked,
        )
    };

    // For docked ships, the origin is the docked system.
    // For non-docked ships, the origin is either the current destination
    // (in-transit / parked at target) or a loitering deep-space position
    // (#266).
    let origin_system = if let Some(ds) = docked_system {
        Some(ds)
    } else {
        current_destination_system
    };

    // Resolve the ship's current Position — either from a system entity (the
    // common case) or a deep-space loitering coordinate.
    let ship_pos: Option<Position> = if let Some(sys) = origin_system {
        positions.get(sys).ok().copied()
    } else {
        loitering_pos.map(Position::from)
    };

    let Some(ship_pos) = ship_pos else {
        // No origin determinable; close menu.
        context_menu.open = false;
        return ctx_actions;
    };

    let same_system = is_docked && origin_system == Some(target_entity);

    // #76: Calculate light-speed delay from player to ship's location.
    // For in-transit ships, the command must also wait for the ship to
    // arrive at its destination (it can't receive commands mid-FTL).
    //
    // #491 (PR-3): `remaining_travel` is now the dispatcher's projected
    // ETA (= `ShipProjection.expected_arrival_at - clock.elapsed`) instead
    // of the realtime `arrival_at` / `completes_at`. The semantic shift
    // is intentional: the player's command travel time should align with
    // *what the player believes* about the ship's arrival, not the
    // ground-truth ECS timeline (which the player can't observe yet).
    // The `light_delay.max(remaining_travel)` envelope is preserved.
    let command_delay: i64 = {
        let light_delay: i64 = player_q
            .single()
            .ok()
            .and_then(|(_, stationed, _)| {
                let player_pos = positions.get(stationed.system).ok()?;
                let dist = physics::distance_ly(player_pos, &ship_pos);
                Some(physics::light_delay_hexadies(dist))
            })
            .unwrap_or(0);

        light_delay.max(remaining_travel)
    };

    // Collect target star data
    let Ok((_, target_star, target_pos, target_attrs)) = stars.get(target_entity) else {
        context_menu.open = false;
        return ctx_actions;
    };

    let dist = physics::distance_ly(&ship_pos, target_pos);
    let target_name = target_star.name.clone();
    let target_surveyed = target_star.surveyed;

    // #114: Check for colonizable planets (habitable + uncolonized) in the target system
    let colonized_planets: std::collections::HashSet<Entity> =
        colonies.iter().map(|c| c.planet).collect();
    let has_colonizable_planet = planet_entities.iter().any(|(pe, p, attrs)| {
        p.system == target_entity
            && attrs
                .map(|a| crate::galaxy::is_colonizable(a.habitability))
                .unwrap_or(false)
            && !colonized_planets.contains(&pe)
    });

    // #108: Unified move — auto-route picks FTL vs sublight.
    // #296 (S-3): Immobile ships (Infrastructure Cores) cannot be commanded
    // to move, so suppress the MoveTo button entirely.
    let can_move = !same_system && !ship_immobile;
    // Survey: can survey unsurveyed system (docked: immediate/delayed, non-docked: queued)
    let can_survey = design_registry.can_survey(&design_id) && !target_surveyed;
    // #52/#56: Check for hostile presence at target system
    let target_has_hostile = hostile_systems.contains(&target_entity);
    // #299 (S-5): Check Core presence — colonization requires a Core owned
    // by the ship's faction in the target system.
    let ship_faction_entity = match ship_owner {
        Owner::Empire(e) => Some(e),
        Owner::Neutral => None,
    };
    let target_has_own_core = ship_faction_entity.is_some_and(|faction| {
        core_by_system
            .iter()
            .any(|&(sys, fo)| sys == target_entity && fo == faction)
    });
    // Colonize: can colonize surveyed system with at least one habitable uncolonized planet, no hostiles, and a Core
    let can_colonize = design_registry.can_colonize(&design_id)
        && has_colonizable_planet
        && target_surveyed
        && !target_has_hostile
        && target_has_own_core;

    let mut command: Option<ShipState> = None;
    let mut queued_command: Option<QueuedCommand> = None;
    // #76: Delayed command for remote ships (light-speed delay > 0)
    let mut delayed_command: Option<crate::ship::ShipCommand> = None;
    let mut close_menu = false;

    // #389: Can dock at a harbour in the target system?
    let can_dock = same_system && !ship_is_docked_at_harbour && !target_harbours.is_empty();

    // No actions available at all? Close and bail
    if !can_move && !can_survey && !can_colonize && !can_dock {
        context_menu.open = false;
        return ctx_actions;
    }

    // Shift+click: execute default action immediately without showing menu
    if context_menu.execute_default {
        if is_docked && same_system {
            // Same system: default is survey or colonize
            if can_survey {
                if command_delay == 0 {
                    command = Some(ShipState::Surveying {
                        target_system: target_entity,
                        started_at: clock.elapsed,
                        completes_at: clock.elapsed + crate::ship::SURVEY_DURATION_HEXADIES,
                    });
                } else {
                    delayed_command = Some(crate::ship::ShipCommand::Survey {
                        target: target_entity,
                    });
                }
            } else if can_colonize {
                if command_delay == 0 {
                    command = Some(ShipState::Settling {
                        system: target_entity,
                        planet: None,
                        started_at: clock.elapsed,
                        completes_at: clock.elapsed + crate::ship::SETTLING_DURATION_HEXADIES,
                    });
                } else {
                    delayed_command = Some(crate::ship::ShipCommand::Colonize);
                }
            }
            context_menu.open = false;
            context_menu.target_system = None;
            context_menu.execute_default = false;
            if let Some(new_state) = command {
                // #482: zero-delay dispatch — record an equivalent
                // `ShipCommand` for the projection-write path so the
                // own-ship Galaxy Map render branch (#477) can locate
                // this ship even though it's bypassing the
                // `PendingShipCommand` pipeline that #475 hooked.
                let equiv = match &new_state {
                    ShipState::Surveying { target_system, .. } => {
                        Some(crate::ship::ShipCommand::Survey {
                            target: *target_system,
                        })
                    }
                    ShipState::Settling { .. } => Some(crate::ship::ShipCommand::Colonize),
                    _ => None,
                };
                if let Some(eq) = equiv {
                    ctx_actions.zero_delay_dispatches.push((ship_entity, eq));
                }
                apply_local_ship_command(ship_entity, new_state, command_delay, ships_query);
            }
            if let Some(ship_cmd) = delayed_command {
                info!(
                    "Command sent to {} (arrives in {} hd)",
                    ship_name, command_delay
                );
                pending_commands_out.push(crate::ship::PendingShipCommand {
                    ship: ship_entity,
                    command: ship_cmd,
                    arrives_at: clock.elapsed + command_delay,
                });
            }
            return ctx_actions;
        } else if is_docked {
            // #108: Unified move — command queue or pending command handles FTL vs sublight
            if command_delay == 0 {
                // Queue the move; the dispatcher + move handler will auto-route
                queued_command = Some(QueuedCommand::MoveTo {
                    system: target_entity,
                });
            } else {
                delayed_command = Some(crate::ship::ShipCommand::MoveTo {
                    destination: target_entity,
                });
            }
        } else {
            // Non-docked: queue the default action (with delay if remote)
            let qc = QueuedCommand::MoveTo {
                system: target_entity,
            };
            if command_delay > 0 {
                delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(qc));
            } else {
                queued_command = Some(qc);
            }
        }
        context_menu.open = false;
        context_menu.target_system = None;
        context_menu.execute_default = false;

        if let Some(new_state) = command {
            if apply_local_ship_command(ship_entity, new_state, command_delay, ships_query) {
                selected_ship.0 = None;
            }
        }
        if let Some(ship_cmd) = delayed_command {
            info!(
                "Command sent to {} (arrives in {} hd)",
                ship_name, command_delay
            );
            pending_commands_out.push(crate::ship::PendingShipCommand {
                ship: ship_entity,
                command: ship_cmd,
                arrives_at: clock.elapsed + command_delay,
            });
            selected_ship.0 = None;
        }
        if let Some(qc) = queued_command {
            // #482: zero-delay queued command (= shift+click in the
            // cross-system / non-docked branches with `command_delay
            // == 0`). Record an equivalent `ShipCommand` for the
            // projection write before pushing to the queue.
            if let Some(eq) = queued_command_to_ship_command(&qc) {
                ctx_actions.zero_delay_dispatches.push((ship_entity, eq));
            }
            if let Ok(mut queue) = command_queues.get_mut(ship_entity) {
                queue.commands.push(qc);
                selected_ship.0 = None;
            }
        }
        return ctx_actions;
    }

    let menu_pos = egui::pos2(context_menu.position[0], context_menu.position[1]);
    let queue_prefix = if is_docked { "" } else { "Queue: " };

    egui::Window::new("Ship Commands")
        .fixed_pos(menu_pos)
        .resizable(false)
        .collapsible(false)
        .title_bar(false)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(format!("{} -> {}", ship_name, target_name)).strong());
            ui.label(format!("Distance: {:.1} ly", dist));
            // #76: Show command delay if player is remote
            if command_delay > 0 {
                ui.label(
                    egui::RichText::new(format!("Command delay: {} hd", command_delay))
                        .color(egui::Color32::from_rgb(255, 200, 100)),
                );
            }
            if !is_docked {
                ui.label(
                    egui::RichText::new("(commands will be queued)")
                        .weak()
                        .italics(),
                );
            }
            ui.separator();

            // #108: Unified Move — auto-route picks FTL chain > FTL direct > SubLight
            if can_move {
                let move_label = format!("{}Move to {}", queue_prefix, target_name);
                let move_resp = ui.button(&move_label);
                #[cfg(feature = "remote")]
                if let Some(ref mut reg) = ui_registry {
                    crate::ui::register_ui_element(
                        reg,
                        "context_menu.move",
                        "Move",
                        move_resp.rect,
                    );
                }
                if move_resp.clicked() {
                    let qc = QueuedCommand::MoveTo {
                        system: target_entity,
                    };
                    if is_docked {
                        if command_delay == 0 {
                            queued_command = Some(qc);
                        } else {
                            delayed_command = Some(crate::ship::ShipCommand::MoveTo {
                                destination: target_entity,
                            });
                        }
                    } else if command_delay > 0 {
                        // In-transit + remote: delay until command reaches the ship
                        delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(qc));
                    } else {
                        queued_command = Some(qc);
                    }
                    close_menu = true;
                }
            }

            // Survey -- if Explorer + target unsurveyed
            if can_survey {
                let survey_label = if !is_docked || !same_system {
                    format!("{}Survey", queue_prefix)
                } else {
                    "Survey".to_string()
                };
                let survey_resp = ui.button(&survey_label);
                #[cfg(feature = "remote")]
                if let Some(ref mut reg) = ui_registry {
                    crate::ui::register_ui_element(
                        reg,
                        "context_menu.survey",
                        "Survey",
                        survey_resp.rect,
                    );
                }
                if survey_resp.clicked() {
                    let qc = QueuedCommand::Survey {
                        system: target_entity,
                    };
                    if !is_docked {
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(qc));
                        } else {
                            queued_command = Some(qc);
                        }
                    } else if same_system {
                        if command_delay == 0 {
                            command = Some(ShipState::Surveying {
                                target_system: target_entity,
                                started_at: clock.elapsed,
                                completes_at: clock.elapsed + crate::ship::SURVEY_DURATION_HEXADIES,
                            });
                        } else {
                            delayed_command = Some(crate::ship::ShipCommand::Survey {
                                target: target_entity,
                            });
                        }
                    } else {
                        // Docked at different system: queue survey (auto-inserts move)
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(
                                QueuedCommand::Survey {
                                    system: target_entity,
                                },
                            ));
                        } else {
                            queued_command = Some(QueuedCommand::Survey {
                                system: target_entity,
                            });
                        }
                    }
                    close_menu = true;
                }
            }

            // Colonize -- if ColonyShip + target has colonizable planet
            if can_colonize {
                let colonize_label = if !is_docked || !same_system {
                    format!("{}Colonize", queue_prefix)
                } else {
                    "Colonize".to_string()
                };
                let colonize_resp = ui.button(&colonize_label);
                #[cfg(feature = "remote")]
                if let Some(ref mut reg) = ui_registry {
                    crate::ui::register_ui_element(
                        reg,
                        "context_menu.colonize",
                        "Colonize",
                        colonize_resp.rect,
                    );
                }
                if colonize_resp.clicked() {
                    let qc = QueuedCommand::Colonize {
                        system: target_entity,
                        planet: None,
                    };
                    if !is_docked {
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(qc));
                        } else {
                            queued_command = Some(qc);
                        }
                    } else if same_system {
                        if command_delay == 0 {
                            command = Some(ShipState::Settling {
                                system: target_entity,
                                planet: None,
                                started_at: clock.elapsed,
                                completes_at: clock.elapsed
                                    + crate::ship::SETTLING_DURATION_HEXADIES,
                            });
                        } else {
                            delayed_command = Some(crate::ship::ShipCommand::Colonize);
                        }
                    } else {
                        // Docked at different system: queue colonize (auto-inserts move)
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(
                                QueuedCommand::Colonize {
                                    system: target_entity,
                                    planet: None,
                                },
                            ));
                        } else {
                            queued_command = Some(QueuedCommand::Colonize {
                                system: target_entity,
                                planet: None,
                            });
                        }
                    }
                    close_menu = true;
                }
            }

            // #389: Dock at harbour buttons
            if can_dock {
                for harbour in target_harbours {
                    let label = format!("Dock at {}", harbour.name);
                    let dock_btn = ui.add_enabled(harbour.can_dock, egui::Button::new(&label));
                    if harbour.can_dock {
                        if dock_btn.clicked() {
                            ctx_actions.dock_at = Some((ship_entity, harbour.entity));
                            close_menu = true;
                        }
                    } else {
                        dock_btn.on_disabled_hover_text("Harbour full — insufficient capacity");
                    }
                }
            }

            ui.separator();
            let cancel_resp = ui.button("Cancel");
            #[cfg(feature = "remote")]
            if let Some(ref mut reg) = ui_registry {
                crate::ui::register_ui_element(
                    reg,
                    "context_menu.cancel",
                    "Cancel",
                    cancel_resp.rect,
                );
            }
            if cancel_resp.clicked() {
                close_menu = true;
            }
        });

    if close_menu {
        context_menu.open = false;
        context_menu.target_system = None;
    }

    // Apply immediate command (docked ships, no delay).
    // The same-system / docked precondition is enforced by
    // `apply_local_ship_command` via `debug_assert_eq!(command_delay, 0)`
    // — see #462.
    if let Some(new_state) = command {
        // #482: zero-delay dispatch (same-system survey / colonize) —
        // record an equivalent `ShipCommand` for the projection-write
        // path before applying. Mirrors the early-return shift+click
        // branch above.
        let equiv = match &new_state {
            ShipState::Surveying { target_system, .. } => Some(crate::ship::ShipCommand::Survey {
                target: *target_system,
            }),
            ShipState::Settling { .. } => Some(crate::ship::ShipCommand::Colonize),
            _ => None,
        };
        if let Some(eq) = equiv {
            ctx_actions.zero_delay_dispatches.push((ship_entity, eq));
        }
        if apply_local_ship_command(ship_entity, new_state, command_delay, ships_query) {
            selected_ship.0 = None;
        }
    }

    // #76: Apply delayed command (docked ships, light-speed delay > 0)
    if let Some(ship_cmd) = delayed_command {
        info!(
            "Command sent to {} (arrives in {} hd)",
            ship_name, command_delay
        );
        pending_commands_out.push(crate::ship::PendingShipCommand {
            ship: ship_entity,
            command: ship_cmd,
            arrives_at: clock.elapsed + command_delay,
        });
        selected_ship.0 = None;
    }

    // Apply queued command (non-docked ships)
    // #407: Apply MoveTo to all selected ships when multi-selected.
    if let Some(ref qc) = queued_command {
        // #482: queued commands enter the ship's local `CommandQueue`
        // immediately (no light-delay), so the projection write must
        // also fire at the dispatch tick. Map back to a `ShipCommand`
        // for the existing player-dispatch projection helper.
        let equiv = queued_command_to_ship_command(qc);
        if matches!(qc, QueuedCommand::MoveTo { .. }) && selected_ships.len() > 1 {
            for &other_ship in selected_ships.iter() {
                if let Ok(mut queue) = command_queues.get_mut(other_ship) {
                    queue.commands.push(qc.clone());
                    if let Some(ref eq) = equiv {
                        ctx_actions
                            .zero_delay_dispatches
                            .push((other_ship, eq.clone()));
                    }
                }
            }
            selected_ship.0 = None;
            selected_ships.clear();
        } else if let Ok(mut queue) = command_queues.get_mut(ship_entity) {
            queue.commands.push(qc.clone());
            if let Some(eq) = equiv {
                ctx_actions.zero_delay_dispatches.push((ship_entity, eq));
            }
            selected_ship.0 = None;
        }
    }

    ctx_actions
}
