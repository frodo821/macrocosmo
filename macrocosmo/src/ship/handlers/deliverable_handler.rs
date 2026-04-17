//! #334 Phase 2: deliverable command handlers.
//!
//! Extracted verbatim from `deliverable_ops::process_deliverable_commands`
//! — same validation, same mutation, same `FactSysParam` dual-writes. The
//! only change is that these systems are **message-driven**: the dispatcher
//! emits per-variant `*Requested` messages instead of the legacy loop
//! pulling the queue head.
//!
//! Commit 1: `handle_load_deliverable_requested`, `handle_deploy_deliverable_requested`.
//! Commit 2: Core-branch of Deploy emits a [`CoreDeployRequested`] message
//! consumed by `core_deliverable::handle_core_deploy_requested`.
//! Commit 3: `handle_transfer_to_structure_requested`, `handle_load_from_scrapyard_requested`.

use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::DeliverableStockpile;
use crate::components::Position;
use crate::deep_space::{
    ConstructionPlatform, DeepSpaceStructure, Scrapyard, StructureRegistry,
    spawn_deliverable_entity,
};
use crate::knowledge::{FactSysParam, KnowledgeFact, PlayerVantage};
use crate::player::{AboardShip, Player, StationedAt};
use crate::ship::command_events::{
    CommandExecuted, CommandKind, CommandResult, CoreDeployRequested, DeployDeliverableRequested,
    LoadDeliverableRequested, LoadFromScrapyardRequested, TransferToStructureRequested,
};
use crate::ship::deliverable_ops::DEPLOY_POSITION_EPSILON;
use crate::ship::{Cargo, CommandQueue, QueuedCommand, Ship, ShipModifiers, ShipState};
use crate::time_system::GameClock;

// ---------------------------------------------------------------------------
// LoadDeliverable
// ---------------------------------------------------------------------------

/// Handles `LoadDeliverableRequested`. Mirrors the `LoadDeliverable` arm of
/// the legacy `process_deliverable_commands`: validates that the ship is
/// docked at the target system (auto-queues a `MoveTo` otherwise), pulls the
/// stockpile item if cargo capacity permits, and dual-writes a
/// `StructureBuilt` knowledge fact.
#[allow(clippy::too_many_arguments)]
pub fn handle_load_deliverable_requested(
    clock: Res<GameClock>,
    balance: Res<crate::technology::GameBalance>,
    registry: Res<StructureRegistry>,
    mut reqs: MessageReader<LoadDeliverableRequested>,
    mut events: MessageWriter<crate::events::GameEvent>,
    mut executed: MessageWriter<CommandExecuted>,
    mut ships: Query<(
        &Ship,
        &ShipState,
        &Position,
        &mut CommandQueue,
        &mut Cargo,
        &ShipModifiers,
    )>,
    mut stockpiles: Query<&mut DeliverableStockpile>,
    star_systems: Query<(Entity, &Position), (Without<Ship>, With<crate::galaxy::StarSystem>)>,
    player_q: Query<&StationedAt, Without<Ship>>,
    player_aboard_q: Query<&AboardShip, With<Player>>,
    mut fact_sys: FactSysParam,
) {
    let mass_per_slot_raw = balance.mass_per_item_slot().0;
    let player_system = player_q.iter().next().map(|s| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| star_systems.get(s).ok())
        .map(|(_, p)| p.as_array());
    let player_aboard = player_aboard_q.iter().next().is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });

    for req in reqs.read() {
        let Ok((ship, state, _ship_pos, mut queue, mut cargo, ship_mods)) = ships.get_mut(req.ship)
        else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadDeliverable,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "ship unavailable".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        // Ship must be docked at the target system.
        let docked_system = match state {
            ShipState::InSystem { system } => Some(*system),
            _ => None,
        };
        if docked_system != Some(req.system) {
            // Auto-insert a MoveTo. The command queue ordering is preserved
            // because the caller (dispatcher) already popped the original
            // LoadDeliverable; we re-inject it here so the legacy queue
            // sees it again after the ship arrives.
            queue.commands.insert(
                0,
                QueuedCommand::LoadDeliverable {
                    system: req.system,
                    stockpile_index: req.stockpile_index,
                },
            );
            queue
                .commands
                .insert(0, QueuedCommand::MoveTo { system: req.system });
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadDeliverable,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        let Ok(mut stockpile) = stockpiles.get_mut(req.system) else {
            warn!("LoadDeliverable: system has no DeliverableStockpile");
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadDeliverable,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "no stockpile".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };
        let Some(item) = stockpile.items.get(req.stockpile_index).cloned() else {
            warn!(
                "LoadDeliverable: index {} out of range (len={})",
                req.stockpile_index,
                stockpile.items.len()
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadDeliverable,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "stockpile index out of range".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        let size = registry
            .get(item.definition_id())
            .and_then(|d| d.deliverable.as_ref().map(|m| m.cargo_size))
            .unwrap_or(1);
        let cap = ship_mods.cargo_capacity.final_value();
        let lookup = |id: &str| -> Option<u32> {
            registry
                .get(id)
                .and_then(|d| d.deliverable.as_ref().map(|m| m.cargo_size))
        };
        if !cargo.can_fit(size, cap, &lookup, mass_per_slot_raw) {
            warn!(
                "LoadDeliverable: ship {} has insufficient cargo capacity for {}",
                ship.name,
                item.definition_id()
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadDeliverable,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "insufficient cargo".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        }

        stockpile.items.remove(req.stockpile_index);
        cargo.items.push(item.clone());
        info!(
            "Ship {} loaded {} from system stockpile",
            ship.name,
            item.definition_id()
        );

        // Dual-write knowledge fact.
        let event_id = fact_sys.allocate_event_id();
        let desc = format!("{} loaded {}", ship.name, item.definition_id());
        events.write(crate::events::GameEvent {
            id: event_id,
            timestamp: clock.elapsed,
            kind: crate::events::GameEventKind::ShipBuilt,
            description: desc.clone(),
            related_system: Some(req.system),
        });
        let origin_pos: Option<[f64; 3]> =
            star_systems.get(req.system).ok().map(|(_, p)| p.as_array());
        if let (Some(v), Some(op)) = (vantage, origin_pos) {
            let fact = KnowledgeFact::StructureBuilt {
                event_id: Some(event_id),
                system: Some(req.system),
                kind: "cargo_load".into(),
                name: item.definition_id().to_string(),
                destroyed: false,
                detail: desc,
            };
            fact_sys.record(fact, op, clock.elapsed, &v);
        }

        executed.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::LoadDeliverable,
            ship: req.ship,
            result: CommandResult::Ok,
            completed_at: clock.elapsed,
        });
    }
}

// ---------------------------------------------------------------------------
// DeployDeliverable (structure-spawn + Core-branch)
// ---------------------------------------------------------------------------

/// Handles `DeployDeliverableRequested`. Preserves both branches:
/// - Non-Core: spawns the deep-space structure via
///   [`spawn_deliverable_entity`] and dual-writes the Deploy event.
/// - Core (`spawns_as_ship = Some(_)`): emits a [`CoreDeployRequested`]
///   message consumed by `handle_core_deploy_requested` (tie-break + spawn).
///   The Deploy handler emits a `Deferred` `CommandExecuted` so the
///   two-phase CommandLog status lands; the Core handler emits the terminal
///   `Ok` / `Rejected` keyed by the original `command_id`.
#[allow(clippy::too_many_arguments)]
pub fn handle_deploy_deliverable_requested(
    mut commands: Commands,
    clock: Res<GameClock>,
    registry: Res<StructureRegistry>,
    mut reqs: MessageReader<DeployDeliverableRequested>,
    mut events: MessageWriter<crate::events::GameEvent>,
    mut executed: MessageWriter<CommandExecuted>,
    mut core_out: MessageWriter<CoreDeployRequested>,
    mut ships: Query<(
        Entity,
        &Ship,
        &ShipState,
        &Position,
        &mut CommandQueue,
        &mut Cargo,
    )>,
    existing_cores: Query<&crate::galaxy::AtSystem, With<crate::ship::CoreShip>>,
    star_systems: Query<(Entity, &Position), (Without<Ship>, With<crate::galaxy::StarSystem>)>,
    player_q: Query<&StationedAt, Without<Ship>>,
    player_aboard_q: Query<&AboardShip, With<Player>>,
    mut fact_sys: FactSysParam,
) {
    let player_system = player_q.iter().next().map(|s| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| star_systems.get(s).ok())
        .map(|(_, p)| p.as_array());
    let player_aboard = player_aboard_q.iter().next().is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });

    for req in reqs.read() {
        let Ok((ship_entity, ship, state, ship_pos, mut queue, mut cargo)) =
            ships.get_mut(req.ship)
        else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::DeployDeliverable,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "ship unavailable".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        // Ship must not be in FTL or surveying. Loitering/Docked OK.
        let allowed = matches!(
            state,
            ShipState::InSystem { .. } | ShipState::Loitering { .. }
        );
        if !allowed {
            // Wait until movement completes — re-inject the original command
            // into the queue head so it retries next tick when the ship is
            // idle.
            queue.commands.insert(
                0,
                QueuedCommand::DeployDeliverable {
                    position: req.position,
                    item_index: req.item_index,
                },
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::DeployDeliverable,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        // Check that ship is at position.
        let here = ship_pos.as_array();
        let d = (here[0] - req.position[0]).powi(2)
            + (here[1] - req.position[1]).powi(2)
            + (here[2] - req.position[2]).powi(2);
        if d.sqrt() > DEPLOY_POSITION_EPSILON {
            queue.commands.insert(
                0,
                QueuedCommand::DeployDeliverable {
                    position: req.position,
                    item_index: req.item_index,
                },
            );
            queue.commands.insert(
                0,
                QueuedCommand::MoveToCoordinates {
                    target: req.position,
                },
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::DeployDeliverable,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        // Execute deployment.
        let Some(item) = cargo.items.get(req.item_index).cloned() else {
            warn!("DeployDeliverable: item_index out of range");
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::DeployDeliverable,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "cargo index out of range".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };
        let def_id = item.definition_id().to_string();
        let Some(def) = registry.get(&def_id) else {
            warn!("DeployDeliverable: unknown definition {}", def_id);
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::DeployDeliverable,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "unknown definition".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        // Core branch: deliverables with `spawns_as_ship = Some(_)` route
        // through the Core deploy pipeline.
        if let Some(design_id) = def
            .deliverable
            .as_ref()
            .and_then(|m| m.spawns_as_ship.as_ref())
        {
            let mut target: Option<(Entity, Position)> = None;
            let mut best_d: f64 = f64::INFINITY;
            for (sys_entity, sys_pos) in star_systems.iter() {
                let dx = req.position[0] - sys_pos.x;
                let dy = req.position[1] - sys_pos.y;
                let dz = req.position[2] - sys_pos.z;
                let d = (dx * dx + dy * dy + dz * dz).sqrt();
                if d <= crate::galaxy::SYSTEM_RADIUS_LY && d < best_d {
                    best_d = d;
                    target = Some((sys_entity, *sys_pos));
                }
            }
            let Some((target_system, sys_pos)) = target else {
                warn!(
                    "Core deploy self-destruct: ship {} deployed {} in deep space",
                    ship.name, def_id
                );
                cargo.items.remove(req.item_index);
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::DeployDeliverable,
                    ship: req.ship,
                    result: CommandResult::Rejected {
                        reason: "deep-space Core self-destruct".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
                continue;
            };
            // Early validation: already owned → self-destruct.
            let has_core = existing_cores.iter().any(|at| at.0 == target_system);
            if has_core {
                info!(
                    "Core deploy self-destruct: system {:?} already has a Core (ship {})",
                    target_system, ship.name
                );
                cargo.items.remove(req.item_index);
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::DeployDeliverable,
                    ship: req.ship,
                    result: CommandResult::Rejected {
                        reason: "system already has Core".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
                continue;
            }
            let faction_owner = match ship.owner {
                crate::ship::Owner::Empire(f) => Some(f),
                crate::ship::Owner::Neutral => None,
            };
            let deploy_pos = [
                sys_pos.x + crate::galaxy::INNER_ORBIT_OFFSET_LY,
                sys_pos.y,
                sys_pos.z,
            ];
            core_out.write(CoreDeployRequested {
                command_id: req.command_id,
                deployer: ship_entity,
                target_system,
                deploy_pos,
                faction_owner,
                owner: ship.owner,
                design_id: design_id.clone(),
                submitted_at: clock.elapsed,
            });
            cargo.items.remove(req.item_index);
            info!(
                "Ship {} emitted CoreDeployRequested for system {:?} (definition={}, cmd {})",
                ship.name, target_system, def_id, req.command_id.0
            );
            // `handle_core_deploy_requested` emits the terminal
            // `CommandExecuted` (Ok / Rejected) this same tick via the
            // `.after(handle_deploy_deliverable_requested)` edge. We emit
            // `Deferred` here so CommandLog acquires an intermediate state
            // — the bridge then overwrites it when the core handler fires.
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::DeployDeliverable,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        // Structure-spawn branch.
        let spawned =
            spawn_deliverable_entity(&mut commands, &def_id, req.position, ship.owner, &registry);
        if spawned.is_none() {
            warn!("DeployDeliverable: spawn failed for {}", def_id);
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::DeployDeliverable,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "spawn failed".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        }
        cargo.items.remove(req.item_index);
        info!(
            "Ship {} deployed {} at {:?}",
            ship.name, def_id, req.position
        );

        // Dual-write Deploy event.
        let event_id = fact_sys.allocate_event_id();
        let desc = format!("{} deployed {}", ship.name, def_id);
        events.write(crate::events::GameEvent {
            id: event_id,
            timestamp: clock.elapsed,
            kind: crate::events::GameEventKind::ShipBuilt,
            description: desc.clone(),
            related_system: None,
        });
        let origin_pos = req.position;
        if let Some(v) = vantage {
            let fact = KnowledgeFact::StructureBuilt {
                event_id: Some(event_id),
                system: None,
                kind: "deployed_deliverable".into(),
                name: def_id.clone(),
                destroyed: false,
                detail: desc,
            };
            fact_sys.record(fact, origin_pos, clock.elapsed, &v);
        }

        executed.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::DeployDeliverable,
            ship: req.ship,
            result: CommandResult::Ok,
            completed_at: clock.elapsed,
        });

        // Silence unused imports warnings in case `DeepSpaceStructure`
        // isn't referenced by a specific build path — it's retained to
        // match the legacy query graph for future reads. (ConstructionPlatform
        // and Scrapyard are used by the transfer / scrapyard handlers below.)
        let _ = std::marker::PhantomData::<DeepSpaceStructure>;
    }
}

// ---------------------------------------------------------------------------
// TransferToStructure
// ---------------------------------------------------------------------------

/// Handles `TransferToStructureRequested`. Auto-injects a MoveToCoordinates
/// when the ship isn't at the platform's position (plan §3.3), then drains
/// the ship's minerals/energy into the platform's accumulated pool clamped
/// by what the ship actually carries.
#[allow(clippy::too_many_arguments)]
pub fn handle_transfer_to_structure_requested(
    clock: Res<GameClock>,
    mut reqs: MessageReader<TransferToStructureRequested>,
    mut executed: MessageWriter<CommandExecuted>,
    mut ships: Query<(&Ship, &Position, &mut CommandQueue, &mut Cargo)>,
    mut platforms: Query<(&Position, &mut ConstructionPlatform), Without<Ship>>,
) {
    for req in reqs.read() {
        let Ok((ship, ship_pos, mut queue, mut cargo)) = ships.get_mut(req.ship) else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::TransferToStructure,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "ship unavailable".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        let Ok((struct_pos, mut platform)) = platforms.get_mut(req.structure) else {
            warn!(
                "TransferToStructure: target {:?} is not a ConstructionPlatform",
                req.structure
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::TransferToStructure,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "target not a platform".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        if ship_pos.distance_to(struct_pos) > DEPLOY_POSITION_EPSILON {
            queue.commands.insert(
                0,
                QueuedCommand::TransferToStructure {
                    structure: req.structure,
                    minerals: req.minerals,
                    energy: req.energy,
                },
            );
            queue.commands.insert(
                0,
                QueuedCommand::MoveToCoordinates {
                    target: struct_pos.as_array(),
                },
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::TransferToStructure,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        let m = cargo.minerals.min(req.minerals);
        let e = cargo.energy.min(req.energy);
        if m == Amt::ZERO && e == Amt::ZERO {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::TransferToStructure,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "nothing to transfer".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        }
        cargo.minerals = cargo.minerals.sub(m);
        cargo.energy = cargo.energy.sub(e);
        platform.accumulated.minerals = platform.accumulated.minerals.add(m);
        platform.accumulated.energy = platform.accumulated.energy.add(e);
        info!(
            "Ship {} transferred {}m/{}e to platform {:?}",
            ship.name,
            m.to_f64(),
            e.to_f64(),
            req.structure
        );
        executed.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::TransferToStructure,
            ship: req.ship,
            result: CommandResult::Ok,
            completed_at: clock.elapsed,
        });
    }
}

// ---------------------------------------------------------------------------
// LoadFromScrapyard
// ---------------------------------------------------------------------------

/// Handles `LoadFromScrapyardRequested`. Auto-injects MoveToCoordinates
/// when not co-located. Drains as much as the ship can hold; minerals are
/// taken first, then energy fills remaining headroom (same algorithm as
/// the legacy code).
#[allow(clippy::too_many_arguments)]
pub fn handle_load_from_scrapyard_requested(
    clock: Res<GameClock>,
    balance: Res<crate::technology::GameBalance>,
    registry: Res<StructureRegistry>,
    mut reqs: MessageReader<LoadFromScrapyardRequested>,
    mut executed: MessageWriter<CommandExecuted>,
    mut ships: Query<(
        &Ship,
        &Position,
        &mut CommandQueue,
        &mut Cargo,
        &ShipModifiers,
    )>,
    mut scrapyards: Query<(&Position, &mut Scrapyard), Without<Ship>>,
) {
    let mass_per_slot_raw = balance.mass_per_item_slot().0;

    for req in reqs.read() {
        let Ok((ship, ship_pos, mut queue, mut cargo, ship_mods)) = ships.get_mut(req.ship) else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadFromScrapyard,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "ship unavailable".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        let Ok((scrap_pos, mut scrap)) = scrapyards.get_mut(req.structure) else {
            warn!(
                "LoadFromScrapyard: target {:?} has no Scrapyard",
                req.structure
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadFromScrapyard,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "target not a scrapyard".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        if ship_pos.distance_to(scrap_pos) > DEPLOY_POSITION_EPSILON {
            queue.commands.insert(
                0,
                QueuedCommand::LoadFromScrapyard {
                    structure: req.structure,
                },
            );
            queue.commands.insert(
                0,
                QueuedCommand::MoveToCoordinates {
                    target: scrap_pos.as_array(),
                },
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadFromScrapyard,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        let cap = ship_mods.cargo_capacity.final_value();
        let lookup = |id: &str| -> Option<u32> {
            registry
                .get(id)
                .and_then(|d| d.deliverable.as_ref().map(|m| m.cargo_size))
        };
        let current_mass = cargo.total_mass_with(&lookup, mass_per_slot_raw);
        let headroom = if current_mass >= cap {
            Amt::ZERO
        } else {
            Amt(cap.0 - current_mass.0)
        };
        let to_take_m = scrap.remaining.minerals.min(headroom);
        let headroom_after_m = Amt(headroom.0.saturating_sub(to_take_m.0));
        let to_take_e = scrap.remaining.energy.min(headroom_after_m);

        if to_take_m == Amt::ZERO && to_take_e == Amt::ZERO {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::LoadFromScrapyard,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "no cargo space".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        }

        cargo.minerals = cargo.minerals.add(to_take_m);
        cargo.energy = cargo.energy.add(to_take_e);
        scrap.remaining.minerals = scrap.remaining.minerals.sub(to_take_m);
        scrap.remaining.energy = scrap.remaining.energy.sub(to_take_e);
        info!(
            "Ship {} salvaged {}m/{}e from scrapyard {:?}",
            ship.name,
            to_take_m.to_f64(),
            to_take_e.to_f64(),
            req.structure
        );
        executed.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::LoadFromScrapyard,
            ship: req.ship,
            result: CommandResult::Ok,
            completed_at: clock.elapsed,
        });
    }
}
