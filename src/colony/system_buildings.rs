use bevy::prelude::*;

use crate::amount::{Amt, SignedAmt};
use crate::galaxy::Planet;
use crate::modifier::{ModifiedValue, Modifier};
use crate::scripting::building_api::{BuildingId, BuildingRegistry};
use crate::time_system::GameClock;

use super::{
    BuildingOrder, Colony, DemolitionOrder, LastProductionTick, MaintenanceCost,
    Production, ResourceStockpile, UpgradeOrder,
};

/// Default number of system building slots for any star system.
pub const DEFAULT_SYSTEM_BUILDING_SLOTS: usize = 6;

/// System-level buildings (Shipyard, ResearchLab, Port) placed on StarSystem entities.
#[derive(Component)]
pub struct SystemBuildings {
    pub slots: Vec<Option<BuildingId>>,
}

impl SystemBuildings {
    /// Check if any slot contains a building with the given id.
    pub fn has_building(&self, id: &str) -> bool {
        self.slots.iter().any(|s| s.as_ref().is_some_and(|b| b.0 == id))
    }

    /// Check if any slot contains a Shipyard.
    pub fn has_shipyard(&self) -> bool {
        self.has_building("shipyard")
    }

    /// Check if any slot contains a Port.
    pub fn has_port(&self) -> bool {
        self.has_building("port")
    }
}

/// Build queue for system-level buildings, placed on StarSystem entities.
#[derive(Component, Default)]
pub struct SystemBuildingQueue {
    pub queue: Vec<BuildingOrder>,
    pub demolition_queue: Vec<DemolitionOrder>,
    pub upgrade_queue: Vec<UpgradeOrder>,
}

impl SystemBuildingQueue {
    /// Check if a given slot is currently being demolished.
    pub fn is_demolishing(&self, slot: usize) -> bool {
        self.demolition_queue.iter().any(|d| d.target_slot == slot)
    }

    /// Get the remaining demolition time for a slot, if any.
    pub fn demolition_time_remaining(&self, slot: usize) -> Option<i64> {
        self.demolition_queue.iter()
            .find(|d| d.target_slot == slot)
            .map(|d| d.time_remaining)
    }

    /// Check if a given slot is currently being upgraded.
    pub fn is_upgrading(&self, slot: usize) -> bool {
        self.upgrade_queue.iter().any(|u| u.slot_index == slot)
    }

    /// Get the upgrade order for a given slot, if any.
    pub fn upgrade_info(&self, slot: usize) -> Option<&UpgradeOrder> {
        self.upgrade_queue.iter().find(|u| u.slot_index == slot)
    }
}

/// Synchronise system building maintenance and production modifiers.
/// System buildings' maintenance costs are pushed into the first colony of each system.
/// System buildings' production bonuses (e.g. ResearchLab) are also pushed to the first colony.
pub fn sync_system_building_maintenance(
    registry: Res<BuildingRegistry>,
    system_buildings_q: Query<(Entity, &SystemBuildings)>,
    mut colonies: Query<(&Colony, &mut MaintenanceCost, &mut Production)>,
    planets: Query<&Planet>,
) {
    // Build a mapping of system entity -> system buildings
    let system_buildings: Vec<(Entity, &SystemBuildings)> = system_buildings_q.iter().collect();

    for (sys_entity, sys_bldgs) in &system_buildings {
        // Find the first colony in this system to attach modifiers to
        let colony_data: Option<()> = None;
        let _ = colony_data; // suppress warning

        for (colony, mut maint, mut prod) in &mut colonies {
            if colony.system(&planets) != Some(*sys_entity) {
                continue;
            }

            // Push maintenance modifiers for system buildings
            for (slot_idx, slot) in sys_bldgs.slots.iter().enumerate() {
                let maint_id = format!("sys_building_maint_{}", slot_idx);
                let prod_id_m = format!("sys_building_{}_minerals", slot_idx);
                let prod_id_e = format!("sys_building_{}_energy", slot_idx);
                let prod_id_r = format!("sys_building_{}_research", slot_idx);
                let prod_id_f = format!("sys_building_{}_food", slot_idx);
                if let Some(bid) = slot {
                    let Some(def) = registry.get(bid.as_str()) else {
                        maint.energy_per_hexadies.pop_modifier(&maint_id);
                        prod.minerals_per_hexadies.pop_modifier(&prod_id_m);
                        prod.energy_per_hexadies.pop_modifier(&prod_id_e);
                        prod.research_per_hexadies.pop_modifier(&prod_id_r);
                        prod.food_per_hexadies.pop_modifier(&prod_id_f);
                        continue;
                    };
                    let cost = def.maintenance;
                    if cost != Amt::ZERO {
                        maint.energy_per_hexadies.push_modifier(Modifier {
                            id: maint_id,
                            label: format!("{} (sys slot {})", def.name, slot_idx),
                            base_add: SignedAmt::from_amt(cost),
                            multiplier: SignedAmt::ZERO,
                            add: SignedAmt::ZERO,
                            expires_at: None,
                            on_expire_event: None,
                        });
                    } else {
                        maint.energy_per_hexadies.pop_modifier(&maint_id);
                    }

                    // Production bonuses from system buildings (e.g. ResearchLab)
                    let (m, e, r, f) = def.production_bonus();
                    let label = format!("{} (sys slot {})", def.name, slot_idx);
                    if m != Amt::ZERO {
                        prod.minerals_per_hexadies.push_modifier(Modifier {
                            id: prod_id_m,
                            label: label.clone(),
                            base_add: SignedAmt::from_amt(m),
                            multiplier: SignedAmt::ZERO,
                            add: SignedAmt::ZERO,
                            expires_at: None,
                            on_expire_event: None,
                        });
                    } else {
                        prod.minerals_per_hexadies.pop_modifier(&prod_id_m);
                    }
                    if e != Amt::ZERO {
                        prod.energy_per_hexadies.push_modifier(Modifier {
                            id: prod_id_e,
                            label: label.clone(),
                            base_add: SignedAmt::from_amt(e),
                            multiplier: SignedAmt::ZERO,
                            add: SignedAmt::ZERO,
                            expires_at: None,
                            on_expire_event: None,
                        });
                    } else {
                        prod.energy_per_hexadies.pop_modifier(&prod_id_e);
                    }
                    if r != Amt::ZERO {
                        prod.research_per_hexadies.push_modifier(Modifier {
                            id: prod_id_r,
                            label: label.clone(),
                            base_add: SignedAmt::from_amt(r),
                            multiplier: SignedAmt::ZERO,
                            add: SignedAmt::ZERO,
                            expires_at: None,
                            on_expire_event: None,
                        });
                    } else {
                        prod.research_per_hexadies.pop_modifier(&prod_id_r);
                    }
                    if f != Amt::ZERO {
                        prod.food_per_hexadies.push_modifier(Modifier {
                            id: prod_id_f,
                            label,
                            base_add: SignedAmt::from_amt(f),
                            multiplier: SignedAmt::ZERO,
                            add: SignedAmt::ZERO,
                            expires_at: None,
                            on_expire_event: None,
                        });
                    } else {
                        prod.food_per_hexadies.pop_modifier(&prod_id_f);
                    }
                } else {
                    maint.energy_per_hexadies.pop_modifier(&maint_id);
                    prod.minerals_per_hexadies.pop_modifier(&prod_id_m);
                    prod.energy_per_hexadies.pop_modifier(&prod_id_e);
                    prod.research_per_hexadies.pop_modifier(&prod_id_r);
                    prod.food_per_hexadies.pop_modifier(&prod_id_f);
                }
            }

            // Only apply to first colony in the system
            break;
        }
    }
}

/// Tick system-level building construction/demolition queues on StarSystem entities.
pub fn tick_system_building_queue(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(Entity, &mut SystemBuildingQueue, &mut SystemBuildings, &mut ResourceStockpile)>,
    mut event_system: ResMut<crate::event_system::EventSystem>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }

    for (system_entity, mut bq, mut buildings, mut stockpile) in &mut query {
        let mut available_minerals = stockpile.minerals;
        let mut available_energy = stockpile.energy;
        let mut minerals_consumed = Amt::ZERO;
        let mut energy_consumed = Amt::ZERO;
        let mut minerals_refunded = Amt::ZERO;
        let mut energy_refunded = Amt::ZERO;

        // --- Process construction queue ---
        for _ in 0..delta {
            if bq.queue.is_empty() {
                break;
            }
            let order = &mut bq.queue[0];

            let minerals_transfer = order.minerals_remaining.min(available_minerals);
            order.minerals_remaining = order.minerals_remaining.sub(minerals_transfer);
            available_minerals = available_minerals.sub(minerals_transfer);
            minerals_consumed = minerals_consumed.add(minerals_transfer);

            let energy_transfer = order.energy_remaining.min(available_energy);
            order.energy_remaining = order.energy_remaining.sub(energy_transfer);
            available_energy = available_energy.sub(energy_transfer);
            energy_consumed = energy_consumed.add(energy_transfer);

            order.build_time_remaining -= 1;

            if bq.queue[0].minerals_remaining == Amt::ZERO
                && bq.queue[0].energy_remaining == Amt::ZERO
                && bq.queue[0].build_time_remaining <= 0
            {
                let completed = bq.queue.remove(0);
                if completed.target_slot < buildings.slots.len() {
                    info!(
                        "System building '{}' completed in slot {}",
                        completed.building_id, completed.target_slot
                    );
                    buildings.slots[completed.target_slot] = Some(completed.building_id);
                }
            }
        }

        // --- Process demolition queue ---
        let mut completed_demolitions = Vec::new();
        for demo in bq.demolition_queue.iter_mut() {
            demo.time_remaining -= delta;
            if demo.time_remaining <= 0 {
                completed_demolitions.push(demo.target_slot);
            }
        }
        for slot_idx in completed_demolitions {
            if let Some(pos) = bq.demolition_queue.iter().position(|d| d.target_slot == slot_idx) {
                let completed = bq.demolition_queue.remove(pos);
                if slot_idx < buildings.slots.len() {
                    let building_name = buildings.slots[slot_idx]
                        .as_ref()
                        .map(|bid| bid.0.as_str())
                        .unwrap_or("Unknown");
                    info!(
                        "System building {} demolished in slot {}, refunded M:{} E:{}",
                        building_name, slot_idx, completed.minerals_refund, completed.energy_refund
                    );
                    buildings.slots[slot_idx] = None;
                    minerals_refunded = minerals_refunded.add(completed.minerals_refund);
                    energy_refunded = energy_refunded.add(completed.energy_refund);
                    event_system.fire_event(
                        "building_demolished",
                        Some(system_entity),
                        clock.elapsed,
                    );
                }
            }
        }

        // --- Process upgrade queue ---
        let mut completed_upgrades = Vec::new();
        for (idx, upgrade) in bq.upgrade_queue.iter_mut().enumerate() {
            for _ in 0..delta {
                let minerals_transfer = upgrade.minerals_remaining.min(available_minerals);
                upgrade.minerals_remaining = upgrade.minerals_remaining.sub(minerals_transfer);
                available_minerals = available_minerals.sub(minerals_transfer);
                minerals_consumed = minerals_consumed.add(minerals_transfer);

                let energy_transfer = upgrade.energy_remaining.min(available_energy);
                upgrade.energy_remaining = upgrade.energy_remaining.sub(energy_transfer);
                available_energy = available_energy.sub(energy_transfer);
                energy_consumed = energy_consumed.add(energy_transfer);

                upgrade.build_time_remaining -= 1;

                if upgrade.minerals_remaining == Amt::ZERO
                    && upgrade.energy_remaining == Amt::ZERO
                    && upgrade.build_time_remaining <= 0
                {
                    completed_upgrades.push(idx);
                    break;
                }
            }
        }
        for idx in completed_upgrades.into_iter().rev() {
            let completed = bq.upgrade_queue.remove(idx);
            if completed.slot_index < buildings.slots.len() {
                let old_name = buildings.slots[completed.slot_index]
                    .as_ref()
                    .map(|bid| bid.0.clone())
                    .unwrap_or_else(|| "empty".to_string());
                buildings.slots[completed.slot_index] = Some(completed.target_id.clone());
                info!(
                    "System building upgrade completed: {} -> {} in slot {}",
                    old_name, completed.target_id, completed.slot_index
                );
                event_system.fire_event(
                    "building_upgraded",
                    Some(system_entity),
                    clock.elapsed,
                );
            }
        }

        stockpile.minerals = stockpile.minerals.sub(minerals_consumed).add(minerals_refunded);
        stockpile.energy = stockpile.energy.sub(energy_consumed).add(energy_refunded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- #113: System vs Planet building classification ---
    // Classification is now Lua-driven via BuildingRegistry.is_system_building()
    // Tested in scripting::building_api tests.

    #[test]
    fn system_buildings_has_shipyard() {
        let sb = SystemBuildings {
            slots: vec![Some(BuildingId::new("shipyard")), None, None],
        };
        assert!(sb.has_shipyard());

        let sb_empty = SystemBuildings {
            slots: vec![Some(BuildingId::new("port")), None, None],
        };
        assert!(!sb_empty.has_shipyard());
    }

    #[test]
    fn system_buildings_has_port() {
        let sb = SystemBuildings {
            slots: vec![None, Some(BuildingId::new("port")), None],
        };
        assert!(sb.has_port());

        let sb_empty = SystemBuildings {
            slots: vec![Some(BuildingId::new("shipyard")), None, None],
        };
        assert!(!sb_empty.has_port());
    }

    #[test]
    fn system_building_queue_is_demolishing() {
        let bq = SystemBuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                target_slot: 1,
                building_id: BuildingId::new("shipyard"),
                time_remaining: 15,
                minerals_refund: Amt::ZERO,
                energy_refund: Amt::ZERO,
            }],
            upgrade_queue: Vec::new(),
        };
        assert!(bq.is_demolishing(1));
        assert!(!bq.is_demolishing(0));
        assert_eq!(bq.demolition_time_remaining(1), Some(15));
        assert_eq!(bq.demolition_time_remaining(0), None);
    }
}
