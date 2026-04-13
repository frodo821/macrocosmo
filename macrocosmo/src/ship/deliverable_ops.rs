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
use crate::ship::{Cargo, CargoItem, CommandQueue, QueuedCommand, Ship, ShipState, ShipStats};

/// Maximum position delta (in light-years) for a ship to be considered
/// "co-located" with a deep-space structure or deploy coordinate.
pub const DEPLOY_POSITION_EPSILON: f64 = 0.01;

/// Process deliverable-side commands at the head of each ship's queue.
///
/// Runs in the `Update` schedule, ordered `.after(advance_game_time)` and
/// `.before(process_command_queue)` so any auto-queued movement reaches the
/// FTL planner on the same tick.
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
        &ShipStats,
    )>,
    mut stockpiles: Query<&mut DeliverableStockpile>,
    mut platforms: Query<(&Position, &mut ConstructionPlatform), Without<Ship>>,
    mut scrapyards: Query<(&Position, &mut Scrapyard), Without<Ship>>,
    structures: Query<(&DeepSpaceStructure, &Position), Without<Ship>>,
) {
    let mass_per_slot_raw = balance.mass_per_item_slot().0;

    for (_ship_entity, ship, state, ship_pos, mut queue, mut cargo, stats) in ships.iter_mut() {
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
                let cap = stats.cargo_capacity.cached();
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
                events.write(crate::events::GameEvent {
                    timestamp: clock.elapsed,
                    kind: crate::events::GameEventKind::ShipBuilt,
                    description: format!("{} loaded {}", ship.name, item.definition_id()),
                    related_system: Some(system),
                });
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
                if registry.get(&def_id).is_none() {
                    warn!("DeployDeliverable: unknown definition {}", def_id);
                    queue.commands.remove(0);
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
                events.write(crate::events::GameEvent {
                    timestamp: clock.elapsed,
                    kind: crate::events::GameEventKind::ShipBuilt,
                    description: format!("{} deployed {}", ship.name, def_id),
                    related_system: None,
                });
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
                let cap = stats.cargo_capacity.cached();
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
