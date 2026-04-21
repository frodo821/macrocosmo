use bevy::prelude::*;

use crate::amount::{Amt, SignedAmt};
use crate::galaxy::{Planet, StarSystem};
use crate::modifier::{ModifiedValue, Modifier};
use crate::scripting::building_api::BuildingRegistry;
use crate::ship::{Ship, ShipState};
use crate::time_system::GameClock;

use super::{Buildings, Colony, LastProductionTick, ResourceStockpile};

/// Colony-level maintenance cost as a ModifiedValue (energy/hexady).
/// The sync_maintenance_modifiers system pushes building and ship maintenance
/// as base_add modifiers; tick_maintenance reads final_value().
#[derive(Component)]
pub struct MaintenanceCost {
    pub energy_per_hexadies: ModifiedValue,
}

impl Default for MaintenanceCost {
    fn default() -> Self {
        Self {
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
        }
    }
}

/// Synchronise maintenance cost modifiers on the MaintenanceCost component.
/// Pushes a `base_add` modifier for each occupied building slot and for each
/// ship whose home_port matches the colony's system.
/// Runs BEFORE tick_maintenance so that `.final_value()` is up-to-date.
pub fn sync_maintenance_modifiers(
    registry: Res<BuildingRegistry>,
    design_registry: Res<crate::ship_design::ShipDesignRegistry>,
    mut colonies: Query<(&Colony, &mut MaintenanceCost, Option<&Buildings>)>,
    ships: Query<(Entity, &Ship)>,
    stars: Query<&StarSystem>,
    planets: Query<&Planet>,
) {
    // Find capital system for fallback
    let capital_entity: Option<Entity> = {
        let mut found = None;
        for (colony, _, _) in colonies.iter() {
            if let Some(sys) = colony.system(&planets) {
                if let Ok(star) = stars.get(sys) {
                    if star.is_capital {
                        found = Some(sys);
                        break;
                    }
                }
            }
        }
        found
    };

    // Collect colony system entities for home_port validation
    let colony_systems: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter_map(|(c, _, _)| c.system(&planets))
        .collect();

    // Collect ship maintenance costs grouped by effective home_port
    let mut ship_costs_by_system: std::collections::HashMap<Entity, Vec<(String, Amt)>> =
        std::collections::HashMap::new();
    for (entity, ship) in &ships {
        let effective_port = if colony_systems.contains(&ship.home_port) {
            ship.home_port
        } else {
            capital_entity.unwrap_or(ship.home_port)
        };
        ship_costs_by_system
            .entry(effective_port)
            .or_default()
            .push((
                format!("ship_maint_{:?}", entity),
                design_registry.maintenance(&ship.design_id),
            ));
    }

    for (colony, mut maint, buildings) in &mut colonies {
        // Track which modifier IDs we set this frame
        let mut active_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Building maintenance modifiers
        if let Some(buildings) = buildings {
            for (slot_idx, slot) in buildings.slots.iter().enumerate() {
                let id = format!("building_maint_{}", slot_idx);
                if let Some(bid) = slot {
                    let cost = registry
                        .get(bid.as_str())
                        .map(|d| d.maintenance)
                        .unwrap_or(Amt::ZERO);
                    let name = registry
                        .get(bid.as_str())
                        .map(|d| d.name.as_str())
                        .unwrap_or(bid.as_str());
                    if cost != Amt::ZERO {
                        maint.energy_per_hexadies.push_modifier(Modifier {
                            id: id.clone(),
                            label: format!("{} (slot {})", name, slot_idx),
                            base_add: SignedAmt::from_amt(cost),
                            multiplier: SignedAmt::ZERO,
                            add: SignedAmt::ZERO,
                            expires_at: None,
                            on_expire_event: None,
                        });
                        active_ids.insert(id);
                    } else {
                        maint.energy_per_hexadies.pop_modifier(&id);
                    }
                } else {
                    maint.energy_per_hexadies.pop_modifier(&id);
                }
            }
        }

        // Ship maintenance modifiers
        let colony_sys = colony.system(&planets);
        if let Some(ref sys) = colony_sys {
            if let Some(ship_list) = ship_costs_by_system.get(sys) {
                for (ship_id, cost) in ship_list {
                    maint.energy_per_hexadies.push_modifier(Modifier {
                        id: ship_id.clone(),
                        label: format!("Ship {}", ship_id),
                        base_add: SignedAmt::from_amt(*cost),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                    active_ids.insert(ship_id.clone());
                }
            }
        }

        // Remove stale ship modifiers (ships that moved away or were destroyed)
        let stale: Vec<String> = maint
            .energy_per_hexadies
            .modifiers()
            .iter()
            .filter(|m| m.id.starts_with("ship_maint_") && !active_ids.contains(&m.id))
            .map(|m| m.id.clone())
            .collect();
        for id in stale {
            maint.energy_per_hexadies.pop_modifier(&id);
        }
    }
}

/// #51/#64: Deduct energy maintenance costs for buildings and ships.
/// Uses MaintenanceCost component (populated by sync_maintenance_modifiers) when present,
/// falling back to manual summing for colonies without the component.
/// Ship home_port reassignment to capital is now handled in sync_maintenance_modifiers.
/// Runs after production so that newly generated energy is available.
pub fn tick_maintenance(
    registry: Res<BuildingRegistry>,
    design_registry: Res<crate::ship_design::ShipDesignRegistry>,
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    colonies: Query<(&Colony, Option<&MaintenanceCost>, Option<&Buildings>)>,
    mut stockpiles: Query<&mut ResourceStockpile, With<StarSystem>>,
    ships: Query<(&Ship, &ShipState)>,
    stars: Query<&StarSystem>,
    planets: Query<&Planet>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;

    // For colonies WITH MaintenanceCost, just read final_value().
    // For colonies WITHOUT it (backward compat), fall back to manual sum.
    let capital_entity: Option<Entity> = {
        let mut found = None;
        for (colony, _, _) in colonies.iter() {
            if let Some(sys) = colony.system(&planets) {
                if let Ok(star) = stars.get(sys) {
                    if star.is_capital {
                        found = Some(sys);
                        break;
                    }
                }
            }
        }
        found
    };

    let colony_systems: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter_map(|(c, _, _)| c.system(&planets))
        .collect();

    let mut ship_maintenance_by_system: std::collections::HashMap<Entity, Amt> =
        std::collections::HashMap::new();

    for (ship, _state) in &ships {
        let effective_port = if colony_systems.contains(&ship.home_port) {
            ship.home_port
        } else {
            capital_entity.unwrap_or(ship.home_port)
        };
        let entry = ship_maintenance_by_system
            .entry(effective_port)
            .or_insert(Amt::ZERO);
        *entry = entry.add(design_registry.maintenance(&ship.design_id));
    }

    // Collect maintenance costs per system
    let mut system_maintenance: std::collections::HashMap<Entity, Amt> =
        std::collections::HashMap::new();
    for (colony, maint, buildings) in &colonies {
        let Some(sys) = colony.system(&planets) else {
            continue;
        };

        let total_maintenance = if let Some(maint) = maint {
            maint.energy_per_hexadies.final_value()
        } else {
            let mut total = Amt::ZERO;
            if let Some(buildings) = buildings {
                for slot in &buildings.slots {
                    if let Some(bid) = slot {
                        total = total.add(
                            registry
                                .get(bid.as_str())
                                .map(|d| d.maintenance)
                                .unwrap_or(Amt::ZERO),
                        );
                    }
                }
            }
            if let Some(&ship_cost) = ship_maintenance_by_system.get(&sys) {
                total = total.add(ship_cost);
            }
            total
        };

        let entry = system_maintenance.entry(sys).or_insert(Amt::ZERO);
        *entry = entry.add(total_maintenance);
    }

    // Deduct energy from system stockpiles
    for (sys, total_maintenance) in system_maintenance {
        if let Ok(mut stockpile) = stockpiles.get_mut(sys) {
            stockpile.energy = stockpile.energy.sub(total_maintenance.mul_u64(d));
        }
    }
}
