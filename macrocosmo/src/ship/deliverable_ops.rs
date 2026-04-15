//! #223: Deliverable-side command processing.
//!
//! Separate from `process_command_queue` (which handles FTL/sublight routing)
//! because deliverable operations are synchronous, in-place, and don't involve
//! route planning. Runs BEFORE `process_command_queue` so that any `MoveTo`
//! auto-queued by this module is dispatched on the same tick.
//!
//! Commands handled:
//!   - `LoadDeliverable { system, stockpile_index }`
//!   - `DeployDeliverable { position, item_index }`
//!   - `TransferToStructure { structure, minerals, energy }`
//!   - `LoadFromScrapyard { structure }`
//!
//! See `src/ship/mod.rs` for the variant docs.

use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::DeliverableStockpile;
use crate::components::Position;
use crate::deep_space::{
    spawn_deliverable_entity, ConstructionPlatform, DeepSpaceStructure, LifetimeCost, ResourceCost,
    Scrapyard, StructureRegistry,
};
use crate::knowledge::{
 FactSysParam, KnowledgeFact, PlayerVantage,
};
use crate::player::{AboardShip, Player, StationedAt};
use crate::ship::{
    Cargo, CargoItem, CommandQueue, QueuedCommand, Ship, ShipModifiers, ShipState,
    core_deliverable::{CoreDeployTicket, PendingCoreDeploys},
};

/// Maximum position delta (in light-years) for a ship to be considered
/// "co-located" with a deep-space structure or deploy coordinate.
pub const DEPLOY_POSITION_EPSILON: f64 = 0.01;

/// Process deliverable-side commands at the head of each ship's queue.
///
/// Runs in the `Update` schedule, ordered `.after(advance_game_time)` and
/// `.before(process_command_queue)` so any auto-queued movement reaches the
/// FTL planner on the same tick.
#[allow(clippy::too_many_arguments)]
pub fn process_deliverable_commands(
    mut commands: Commands,
    clock: Res<crate::time_system::GameClock>,
    balance: Res<crate::technology::GameBalance>,
    registry: Res<StructureRegistry>,
    mut events: MessageWriter<crate::events::GameEvent>,
    mut ships: Query<(
        Entity,
        &Ship,
        &ShipState,
        &Position,
        &mut CommandQueue,
        &mut Cargo,
        &ShipModifiers,
    )>,
    mut stockpiles: Query<&mut DeliverableStockpile>,
    mut platforms: Query<(&Position, &mut ConstructionPlatform), Without<Ship>>,
    mut scrapyards: Query<(&Position, &mut Scrapyard), Without<Ship>>,
    structures: Query<(&DeepSpaceStructure, &Position), Without<Ship>>,
    player_q: Query<&StationedAt, Without<Ship>>,
    player_aboard_q: Query<&AboardShip, With<Player>>,
    // #296 (S-3): (entity, position) for all star systems — used both by the
    // existing player-vantage lookup and by the new Core-deploy proximity
    // match. A single tuple-style Query keeps the total param count under
    // Bevy's 16-arg limit.
    star_systems: Query<(Entity, &Position), (Without<Ship>, With<crate::galaxy::StarSystem>)>,
    // #296 (S-3): Existing Core ships keyed by AtSystem for the
    // "already-has-core" validation branch. Query is `With<CoreShip>` so it
    // stays disjoint from any writable Ship query.
    existing_cores: Query<&crate::galaxy::AtSystem, With<crate::ship::CoreShip>>,
    // #296 (S-3): Pending ticket queue — resolved downstream by
    // `resolve_core_deploys` with tie-break + Core-ship spawn.
    mut pending_cores: ResMut<PendingCoreDeploys>,
    mut fact_sys: FactSysParam,
) {
    let mass_per_slot_raw = balance.mass_per_item_slot().0;
    // #249: Snapshot player vantage once per tick.
    let player_system = player_q.iter().next().map(|s| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| star_systems.get(s).ok())
        .map(|(_, p)| p.as_array());
    let player_aboard = player_aboard_q.iter().next().is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });

    for (_ship_entity, ship, state, ship_pos, mut queue, mut cargo, ship_mods) in
        ships.iter_mut()
    {
        if queue.commands.is_empty() {
            continue;
        }
        // Peek at head.
        let head = queue.commands[0].clone();
        match head {
            QueuedCommand::LoadDeliverable { system, stockpile_index } => {
                // Ship must be docked at the system.
                let docked_system = match state {
                    ShipState::Docked { system: s } => Some(*s),
                    _ => None,
                };
                if docked_system != Some(system) {
                    // Inject a MoveTo to that system before this command.
                    // The existing command is kept; insert the move at position 0.
                    queue.commands.insert(0, QueuedCommand::MoveTo { system });
                    continue;
                }
                // Find the stockpile.
                let Ok(mut stockpile) = stockpiles.get_mut(system) else {
                    warn!("LoadDeliverable: system has no DeliverableStockpile");
                    queue.commands.remove(0);
                    continue;
                };
                let Some(item) = stockpile.items.get(stockpile_index).cloned() else {
                    warn!(
                        "LoadDeliverable: index {} out of range (len={})",
                        stockpile_index,
                        stockpile.items.len()
                    );
                    queue.commands.remove(0);
                    continue;
                };
                // Check cargo capacity.
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
                    queue.commands.remove(0);
                    continue;
                }
                // Load it.
                stockpile.items.remove(stockpile_index);
                cargo.items.push(item.clone());
                queue.commands.remove(0);
                info!(
                    "Ship {} loaded {} from system stockpile",
                    ship.name,
                    item.definition_id()
                );
                // #249: Dual-write Load event as a StructureBuilt fact.
                let event_id = fact_sys.allocate_event_id();
                let desc = format!("{} loaded {}", ship.name, item.definition_id());
                events.write(crate::events::GameEvent {
                    id: event_id,
                    timestamp: clock.elapsed,
                    kind: crate::events::GameEventKind::ShipBuilt,
                    description: desc.clone(),
                    related_system: Some(system),
                });
                let origin_pos: Option<[f64; 3]> =
                    star_systems.get(system).ok().map(|(_, p)| p.as_array());
                if let (Some(v), Some(op)) = (vantage, origin_pos) {
                    let fact = KnowledgeFact::StructureBuilt {
                        event_id: Some(event_id),
                        system: Some(system),
                        kind: "cargo_load".into(),
                        name: item.definition_id().to_string(),
                        destroyed: false,
                        detail: desc,
                    };
                    fact_sys.record(fact, op, clock.elapsed, &v);
                }
            }
            QueuedCommand::DeployDeliverable { position, item_index } => {
                // Ship must not be in FTL or surveying. Loitering/Docked OK.
                let allowed = matches!(
                    state,
                    ShipState::Docked { .. } | ShipState::Loitering { .. }
                );
                if !allowed {
                    // Wait until movement completes.
                    continue;
                }
                // Check that ship is at position.
                let here = ship_pos.as_array();
                let d = (here[0] - position[0]).powi(2)
                    + (here[1] - position[1]).powi(2)
                    + (here[2] - position[2]).powi(2);
                if d.sqrt() > DEPLOY_POSITION_EPSILON {
                    queue
                        .commands
                        .insert(0, QueuedCommand::MoveToCoordinates { target: position });
                    continue;
                }
                // Execute deployment.
                let Some(item) = cargo.items.get(item_index).cloned() else {
                    warn!("DeployDeliverable: item_index out of range");
                    queue.commands.remove(0);
                    continue;
                };
                let def_id = item.definition_id().to_string();
                let Some(def) = registry.get(&def_id) else {
                    warn!("DeployDeliverable: unknown definition {}", def_id);
                    queue.commands.remove(0);
                    continue;
                };
                // #296 (S-3): Deliverables whose metadata carries
                // `spawns_as_ship = Some(_)` are Core ships. Route them
                // through PendingCoreDeploys instead of the deep-space
                // structure spawn path. The ticket queue enforces validation
                // (existing Core, tie-break) in a dedicated resolver.
                if let Some(design_id) = def
                    .deliverable
                    .as_ref()
                    .and_then(|m| m.spawns_as_ship.as_ref())
                {
                    // Identify target system by proximity. Deploy must be
                    // inside a known system; a deep-space deploy makes the
                    // ticket self-destruct (consume cargo + remove command).
                    let mut target: Option<(Entity, Position)> = None;
                    let mut best_d: f64 = f64::INFINITY;
                    for (sys_entity, sys_pos) in star_systems.iter() {
                        let dx = position[0] - sys_pos.x;
                        let dy = position[1] - sys_pos.y;
                        let dz = position[2] - sys_pos.z;
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
                        cargo.items.remove(item_index);
                        queue.commands.remove(0);
                        continue;
                    };
                    // Early validation: already owned → self-destruct.
                    let has_core = existing_cores
                        .iter()
                        .any(|at| at.0 == target_system);
                    if has_core {
                        info!(
                            "Core deploy self-destruct: system {:?} already has a Core (ship {})",
                            target_system, ship.name
                        );
                        cargo.items.remove(item_index);
                        queue.commands.remove(0);
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
                    pending_cores.tickets.push(CoreDeployTicket {
                        deployer: _ship_entity,
                        target_system,
                        deploy_pos,
                        faction_owner,
                        owner: ship.owner,
                        design_id: design_id.clone(),
                        cargo_item_index: item_index,
                        submitted_at: clock.elapsed,
                    });
                    cargo.items.remove(item_index);
                    queue.commands.remove(0);
                    info!(
                        "Ship {} enqueued Core deploy in system {:?} (definition={})",
                        ship.name, target_system, def_id
                    );
                    continue;
                }
                let spawned = spawn_deliverable_entity(
                    &mut commands,
                    &def_id,
                    position,
                    ship.owner,
                    &registry,
                );
                if spawned.is_none() {
                    warn!("DeployDeliverable: spawn failed for {}", def_id);
                    queue.commands.remove(0);
                    continue;
                }
                cargo.items.remove(item_index);
                queue.commands.remove(0);
                info!("Ship {} deployed {} at {:?}", ship.name, def_id, position);
                // #249: Dual-write Deploy event.
                let event_id = fact_sys.allocate_event_id();
                let desc = format!("{} deployed {}", ship.name, def_id);
                events.write(crate::events::GameEvent {
                    id: event_id,
                    timestamp: clock.elapsed,
                    kind: crate::events::GameEventKind::ShipBuilt,
                    description: desc.clone(),
                    related_system: None,
                });
                let origin_pos = position;
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
            }
            QueuedCommand::TransferToStructure {
                structure,
                minerals,
                energy,
            } => {
                // Ship must be at the structure's position.
                let Ok((struct_pos, mut platform)) = platforms.get_mut(structure) else {
                    warn!(
                        "TransferToStructure: target {:?} is not a ConstructionPlatform",
                        structure
                    );
                    queue.commands.remove(0);
                    continue;
                };
                if ship_pos.distance_to(struct_pos) > DEPLOY_POSITION_EPSILON {
                    queue.commands.insert(
                        0,
                        QueuedCommand::MoveToCoordinates {
                            target: struct_pos.as_array(),
                        },
                    );
                    continue;
                }
                // Clamp transfers by what the ship actually carries.
                let m = cargo.minerals.min(minerals);
                let e = cargo.energy.min(energy);
                if m == Amt::ZERO && e == Amt::ZERO {
                    queue.commands.remove(0);
                    continue;
                }
                cargo.minerals = cargo.minerals.sub(m);
                cargo.energy = cargo.energy.sub(e);
                platform.accumulated.minerals = platform.accumulated.minerals.add(m);
                platform.accumulated.energy = platform.accumulated.energy.add(e);
                queue.commands.remove(0);
                info!(
                    "Ship {} transferred {}m/{}e to platform {:?}",
                    ship.name,
                    m.to_f64(),
                    e.to_f64(),
                    structure
                );
            }
            QueuedCommand::LoadFromScrapyard { structure } => {
                let Ok((scrap_pos, mut scrap)) = scrapyards.get_mut(structure) else {
                    warn!(
                        "LoadFromScrapyard: target {:?} has no Scrapyard",
                        structure
                    );
                    queue.commands.remove(0);
                    continue;
                };
                if ship_pos.distance_to(scrap_pos) > DEPLOY_POSITION_EPSILON {
                    queue.commands.insert(
                        0,
                        QueuedCommand::MoveToCoordinates {
                            target: scrap_pos.as_array(),
                        },
                    );
                    continue;
                }
                // Drain as much as the ship can hold. Resources are weightless
                // relative to the item-mass model (cargo_capacity bounds the
                // TOTAL mass including resources), so use the same accounting.
                let cap = ship_mods.cargo_capacity.final_value();
                let lookup = |id: &str| -> Option<u32> {
                    registry
                        .get(id)
                        .and_then(|d| d.deliverable.as_ref().map(|m| m.cargo_size))
                };
                let current_mass =
                    cargo.total_mass_with(&lookup, mass_per_slot_raw);
                let headroom = if current_mass >= cap {
                    Amt::ZERO
                } else {
                    Amt(cap.0 - current_mass.0)
                };
                // Split headroom between minerals and energy proportionally.
                let to_take_m = scrap.remaining.minerals.min(headroom);
                let headroom_after_m = Amt(headroom.0.saturating_sub(to_take_m.0));
                let to_take_e = scrap.remaining.energy.min(headroom_after_m);

                if to_take_m == Amt::ZERO && to_take_e == Amt::ZERO {
                    // No space. Drop the command so the user can retry.
                    queue.commands.remove(0);
                    continue;
                }

                cargo.minerals = cargo.minerals.add(to_take_m);
                cargo.energy = cargo.energy.add(to_take_e);
                scrap.remaining.minerals = scrap.remaining.minerals.sub(to_take_m);
                scrap.remaining.energy = scrap.remaining.energy.sub(to_take_e);
                queue.commands.remove(0);
                info!(
                    "Ship {} salvaged {}m/{}e from scrapyard {:?}",
                    ship.name,
                    to_take_m.to_f64(),
                    to_take_e.to_f64(),
                    structure
                );
            }
            // Other commands are handled by process_command_queue.
            _ => {}
        }
    }

    // Suppress unused warning for this query — it's kept for future use.
    let _ = structures;
}

/// #223: Dismantle a deep-space structure. Removes any existing
/// `ConstructionPlatform` (lost investment) and installs a `Scrapyard` whose
/// `remaining = lifetime_cost * scrap_refund`.
pub fn dismantle_structure(
    world: &mut World,
    structure: Entity,
) -> Result<(), &'static str> {
    // Gather what we need without the registry mutably borrowed.
    let (def_id, lifetime) = {
        let Some(ds) = world.get::<DeepSpaceStructure>(structure) else {
            return Err("entity is not a DeepSpaceStructure");
        };
        let lifetime = world
            .get::<LifetimeCost>(structure)
            .map(|lc| lc.0.clone())
            .unwrap_or_default();
        (ds.definition_id.clone(), lifetime)
    };
    let refund = {
        let registry = world.resource::<StructureRegistry>();
        registry
            .get(&def_id)
            .and_then(|d| d.deliverable.as_ref().map(|m| m.scrap_refund))
            .unwrap_or(0.0)
    };
    let remaining = lifetime.scale(refund);
    // Remove markers and install Scrapyard.
    world.entity_mut(structure).remove::<ConstructionPlatform>();
    world.entity_mut(structure).insert(Scrapyard {
        remaining,
        original_definition_id: def_id,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_cost_helpers() {
        let a = ResourceCost {
            minerals: Amt::units(100),
            energy: Amt::units(50),
        };
        assert!(!a.is_zero());
        let half = a.scale(0.5);
        assert_eq!(half.minerals, Amt::units(50));
        assert_eq!(half.energy, Amt::units(25));

        let zero = a.scale(0.0);
        assert!(zero.is_zero());

        assert!(a.covers(&half));
        assert!(!half.covers(&a));
    }
}
