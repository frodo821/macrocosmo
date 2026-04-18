use bevy::prelude::*;

use crate::amount::{Amt, SignedAmt};
use crate::components::Position;
use crate::galaxy::Planet;
use crate::modifier::Modifier;
use crate::scripting::building_api::{BuildingId, BuildingRegistry};
use crate::ship::{Owner, Ship, ShipState};
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

use super::{
    BuildingOrder, CancelledOrderKind, Colony, DemolitionOrder, LastProductionTick,
    MaintenanceCost, ResourceStockpile, UpgradeOrder,
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
        self.slots
            .iter()
            .any(|s| s.as_ref().is_some_and(|b| b.0 == id))
    }

    /// Check if any building in slots has the given capability (looked up via BuildingRegistry).
    pub fn has_capability(&self, capability: &str, registry: &BuildingRegistry) -> bool {
        self.slots.iter().any(|slot| {
            slot.as_ref().is_some_and(|id| {
                registry
                    .get(id.as_str())
                    .is_some_and(|def| def.capabilities.contains_key(capability))
            })
        })
    }

    /// Get a named parameter from the first building with the given capability.
    /// Returns None if no building has the capability or the param is not defined.
    pub fn get_capability_param(
        &self,
        capability: &str,
        param: &str,
        registry: &BuildingRegistry,
    ) -> Option<f64> {
        for slot in &self.slots {
            if let Some(id) = slot {
                if let Some(def) = registry.get(id.as_str()) {
                    if let Some(cap) = def.capabilities.get(capability) {
                        return cap.get(param);
                    }
                }
            }
        }
        None
    }

    /// Check if any slot contains a Shipyard.
    /// Delegates to capability check when a registry is available.
    pub fn has_shipyard(&self, registry: &BuildingRegistry) -> bool {
        self.has_capability("shipyard", registry)
    }

    /// Check if any slot contains a Port.
    /// Delegates to capability check when a registry is available.
    pub fn has_port(&self, registry: &BuildingRegistry) -> bool {
        self.has_capability("port", registry)
    }

    /// Get the port FTL range bonus from the port capability, if present.
    /// Falls back to 10.0 if the param is not specified.
    pub fn port_ftl_range_bonus(&self, registry: &BuildingRegistry) -> f64 {
        self.get_capability_param("port", "ftl_range_bonus", registry)
            .unwrap_or(0.0)
    }

    /// Get the port travel time factor from the port capability, if present.
    /// Falls back to 1.0 (no reduction) if no port or param not specified.
    pub fn port_travel_time_factor(&self, registry: &BuildingRegistry) -> f64 {
        self.get_capability_param("port", "travel_time_factor", registry)
            .unwrap_or(1.0)
    }
}

/// Build queue for system-level buildings, placed on StarSystem entities.
#[derive(Component, Default)]
pub struct SystemBuildingQueue {
    pub queue: Vec<BuildingOrder>,
    pub demolition_queue: Vec<DemolitionOrder>,
    pub upgrade_queue: Vec<UpgradeOrder>,
    /// #275: Shared monotonic counter for `order_id`. See
    /// `BuildingQueue::next_order_id` for rationale.
    pub next_order_id: u64,
}

impl SystemBuildingQueue {
    fn allocate_order_id(&mut self) -> u64 {
        if self.next_order_id == 0 {
            self.next_order_id = 1;
        }
        let id = self.next_order_id;
        self.next_order_id = self.next_order_id.wrapping_add(1);
        id
    }

    /// #275: Push a construction order, auto-assigning `order_id`.
    pub fn push_build_order(&mut self, mut order: BuildingOrder) -> u64 {
        let id = self.allocate_order_id();
        order.order_id = id;
        self.queue.push(order);
        id
    }

    /// #275: Push a demolition order, auto-assigning `order_id`.
    pub fn push_demolition_order(&mut self, mut order: DemolitionOrder) -> u64 {
        let id = self.allocate_order_id();
        order.order_id = id;
        self.demolition_queue.push(order);
        id
    }

    /// #275: Push an upgrade order, auto-assigning `order_id`.
    pub fn push_upgrade_order(&mut self, mut order: UpgradeOrder) -> u64 {
        let id = self.allocate_order_id();
        order.order_id = id;
        self.upgrade_queue.push(order);
        id
    }

    /// #275: See `BuildingQueue::cancel_order`.
    pub fn cancel_order(&mut self, order_id: u64) -> Option<CancelledOrderKind> {
        if let Some(pos) = self.queue.iter().position(|o| o.order_id == order_id) {
            self.queue.remove(pos);
            return Some(CancelledOrderKind::Construction);
        }
        if let Some(pos) = self
            .demolition_queue
            .iter()
            .position(|o| o.order_id == order_id)
        {
            self.demolition_queue.remove(pos);
            return Some(CancelledOrderKind::Demolition);
        }
        if let Some(pos) = self
            .upgrade_queue
            .iter()
            .position(|o| o.order_id == order_id)
        {
            self.upgrade_queue.remove(pos);
            return Some(CancelledOrderKind::Upgrade);
        }
        None
    }

    /// Check if a given slot is currently being demolished.
    pub fn is_demolishing(&self, slot: usize) -> bool {
        self.demolition_queue.iter().any(|d| d.target_slot == slot)
    }

    /// Get the remaining demolition time for a slot, if any.
    pub fn demolition_time_remaining(&self, slot: usize) -> Option<i64> {
        self.demolition_queue
            .iter()
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

/// Synchronise system building maintenance. System buildings' maintenance
/// costs are pushed as modifiers onto the MaintenanceCost of the first colony
/// of each system.
///
/// #241: Production bonuses from system buildings (e.g. ResearchLab) are now
/// handled uniformly by `sync_building_modifiers` via the unified `modifiers`
/// field — this system only handles maintenance.
pub fn sync_system_building_maintenance(
    registry: Res<BuildingRegistry>,
    system_buildings_q: Query<(Entity, &SystemBuildings)>,
    mut colonies: Query<(&Colony, &mut MaintenanceCost)>,
    planets: Query<&Planet>,
) {
    let system_buildings: Vec<(Entity, &SystemBuildings)> = system_buildings_q.iter().collect();

    for (sys_entity, sys_bldgs) in &system_buildings {
        for (colony, mut maint) in &mut colonies {
            if colony.system(&planets) != Some(*sys_entity) {
                continue;
            }

            for (slot_idx, slot) in sys_bldgs.slots.iter().enumerate() {
                let maint_id = format!("sys_building_maint_{}", slot_idx);
                if let Some(bid) = slot {
                    let Some(def) = registry.get(bid.as_str()) else {
                        maint.energy_per_hexadies.pop_modifier(&maint_id);
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
                } else {
                    maint.energy_per_hexadies.pop_modifier(&maint_id);
                }
            }

            // Only apply to first colony in the system
            break;
        }
    }
}

/// #386: Rebuild `SystemBuildings.slots` from station Ship entities present in
/// each system. This makes `SystemBuildings` a derived view — the source of
/// truth is the set of station ships with `ShipState::InSystem`. A reverse
/// index (ship design_id → BuildingId) is built from `BuildingRegistry` entries
/// that carry a `ship_design_id`.
pub fn sync_system_buildings_from_ships(
    mut sys_buildings_q: Query<(Entity, &mut SystemBuildings)>,
    ship_q: Query<(&Ship, &ShipState)>,
    building_registry: Res<BuildingRegistry>,
) {
    // Build reverse index: ship_design_id → BuildingId
    let reverse: std::collections::HashMap<&str, BuildingId> = building_registry
        .buildings
        .values()
        .filter_map(|def| {
            def.ship_design_id
                .as_deref()
                .map(|did| (did, BuildingId::new(&def.id)))
        })
        .collect();

    if reverse.is_empty() {
        return;
    }

    for (system_entity, mut sys_buildings) in &mut sys_buildings_q {
        // Collect all station ship building_ids in this system.
        let mut station_building_ids: Vec<BuildingId> = Vec::new();
        for (ship, state) in &ship_q {
            if let ShipState::InSystem { system } = state {
                if *system == system_entity {
                    if let Some(bid) = reverse.get(ship.design_id.as_str()) {
                        station_building_ids.push(bid.clone());
                    }
                }
            }
        }

        // If there are no station ships at all in this system, skip sync.
        // This preserves backward compatibility with pre-migration setups
        // where SystemBuildings was populated directly without station ships.
        if station_building_ids.is_empty() {
            continue;
        }

        // Reconcile slots with station ships.
        // 1. Clear slots whose building has ship_design_id but no matching
        //    station ship (stale entry from demolished station).
        // 2. Place station ships that aren't yet in any slot into empty slots.
        // 3. Leave non-station building slots (no ship_design_id) untouched.
        let slot_count = sys_buildings.slots.len();
        let mut new_slots = sys_buildings.slots.clone();
        let mut consumed: Vec<bool> = vec![false; station_building_ids.len()];

        // Pass 1: validate existing station-building slots against ships.
        for slot in &mut new_slots {
            if let Some(bid) = slot {
                if let Some(def) = building_registry.get(bid.as_str()) {
                    if def.ship_design_id.is_some() {
                        // This slot should be backed by a station ship.
                        if let Some(idx) = station_building_ids
                            .iter()
                            .enumerate()
                            .find(|(i, sb)| !consumed[*i] && *sb == bid)
                            .map(|(i, _)| i)
                        {
                            consumed[idx] = true;
                        } else {
                            // No matching station ship — clear stale slot.
                            *slot = None;
                        }
                    }
                }
            }
        }

        // Pass 2: place unconsumed station ships into empty slots.
        let mut next_empty = 0;
        for (i, bid) in station_building_ids.into_iter().enumerate() {
            if consumed[i] {
                continue;
            }
            while next_empty < slot_count && new_slots[next_empty].is_some() {
                next_empty += 1;
            }
            if next_empty < slot_count {
                new_slots[next_empty] = Some(bid);
                next_empty += 1;
            }
        }

        // Only mutate if different (avoids unnecessary change detection).
        if new_slots != sys_buildings.slots {
            sys_buildings.slots = new_slots;
        }
    }
}

/// Tick system-level building construction/demolition queues on StarSystem entities.
/// #386: On construction completion, if the building has a `ship_design_id`,
/// spawn a station Ship entity at the system. On demolition, despawn the
/// matching station Ship.
///
/// #387 TODO: For the future Loitering case (mobile shipyard not InSystem),
/// resource sourcing should deduct from the ship's Cargo instead of the
/// system's ResourceStockpile. This is a follow-up task.
pub fn tick_system_building_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(
        Entity,
        &mut SystemBuildingQueue,
        &mut SystemBuildings,
        &mut ResourceStockpile,
        &Position,
    )>,
    mut event_system: ResMut<crate::event_system::EventSystem>,
    building_registry: Res<BuildingRegistry>,
    design_registry: Res<ShipDesignRegistry>,
    faction_q: Query<&crate::faction::FactionOwner>,
    ship_q: Query<(Entity, &Ship, &ShipState)>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }

    for (system_entity, mut bq, mut buildings, mut stockpile, sys_position) in &mut query {
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

            // #232: Gate timer advance on actual progress (see
            // build_tick::maybe_tick_build_time docstring). Mirrors the
            // planet-level building queue so starved system builds don't
            // sink into negative-time limbo.
            let transferred = minerals_transfer > Amt::ZERO || energy_transfer > Amt::ZERO;
            let no_more_needed =
                order.minerals_remaining == Amt::ZERO && order.energy_remaining == Amt::ZERO;
            super::build_tick::maybe_tick_build_time(
                &mut order.build_time_remaining,
                transferred,
                no_more_needed,
            );

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
                    let completed_id = completed.building_id.0.clone();
                    let completed_slot = completed.target_slot;
                    buildings.slots[completed.target_slot] = Some(completed.building_id);
                    // #281: Fire macrocosmo:building_built for system-level
                    // construction. `colony` is omitted since system
                    // buildings attach to the StarSystem entity itself.
                    let mut payload = std::collections::HashMap::new();
                    payload.insert("cause".to_string(), "construction".to_string());
                    payload.insert("building_id".to_string(), completed_id);
                    payload.insert("slot".to_string(), completed_slot.to_string());
                    payload.insert("system".to_string(), system_entity.to_bits().to_string());
                    event_system.fire_event_with_payload(
                        Some(system_entity),
                        clock.elapsed,
                        Box::new(crate::event_system::LuaDefinedEventContext::new(
                            crate::event_system::BUILDING_BUILT_EVENT,
                            payload,
                        )),
                    );

                    // #386: Spawn a station Ship if the building has ship_design_id.
                    if let Some(def) = building_registry.get(
                        buildings.slots[completed_slot]
                            .as_ref()
                            .map(|b| b.0.as_str())
                            .unwrap_or(""),
                    ) {
                        if let Some(ref design_id) = def.ship_design_id {
                            let owner = faction_q
                                .get(system_entity)
                                .ok()
                                .map(|fo| Owner::Empire(fo.0))
                                .unwrap_or(Owner::Neutral);
                            let station_name = def.name.clone();
                            crate::ship::spawn_ship(
                                &mut commands,
                                design_id,
                                station_name,
                                system_entity,
                                *sys_position,
                                owner,
                                &design_registry,
                            );
                            info!(
                                "Spawned station ship '{}' for system building '{}'",
                                design_id, def.id
                            );
                        }
                    }
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
            if let Some(pos) = bq
                .demolition_queue
                .iter()
                .position(|d| d.target_slot == slot_idx)
            {
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

                    // #386: Despawn the corresponding station Ship if one exists.
                    if let Some(bid) = &buildings.slots[slot_idx] {
                        if let Some(def) = building_registry.get(bid.as_str()) {
                            if let Some(ref design_id) = def.ship_design_id {
                                // Find and despawn the station ship matching this
                                // design at this system.
                                for (ship_entity, ship, ship_state) in &ship_q {
                                    if ship.design_id == *design_id {
                                        if let ShipState::InSystem { system } = ship_state {
                                            if *system == system_entity {
                                                commands.entity(ship_entity).despawn();
                                                info!(
                                                    "Despawned station ship '{}' for demolished building '{}'",
                                                    design_id, def.id
                                                );
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

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

                // #232: Gate timer advance on actual progress.
                let transferred = minerals_transfer > Amt::ZERO || energy_transfer > Amt::ZERO;
                let no_more_needed = upgrade.minerals_remaining == Amt::ZERO
                    && upgrade.energy_remaining == Amt::ZERO;
                super::build_tick::maybe_tick_build_time(
                    &mut upgrade.build_time_remaining,
                    transferred,
                    no_more_needed,
                );

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
                event_system.fire_event("building_upgraded", Some(system_entity), clock.elapsed);
                // #281: Fire macrocosmo:building_built with cause="upgrade"
                // for system-level upgrade completion.
                let mut payload = std::collections::HashMap::new();
                payload.insert("cause".to_string(), "upgrade".to_string());
                payload.insert("building_id".to_string(), completed.target_id.0.clone());
                payload.insert("previous_id".to_string(), old_name);
                payload.insert("slot".to_string(), completed.slot_index.to_string());
                payload.insert("system".to_string(), system_entity.to_bits().to_string());
                event_system.fire_event_with_payload(
                    Some(system_entity),
                    clock.elapsed,
                    Box::new(crate::event_system::LuaDefinedEventContext::new(
                        crate::event_system::BUILDING_BUILT_EVENT,
                        payload,
                    )),
                );
            }
        }

        stockpile.minerals = stockpile
            .minerals
            .sub(minerals_consumed)
            .add(minerals_refunded);
        stockpile.energy = stockpile.energy.sub(energy_consumed).add(energy_refunded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::building_api::{BuildingDefinition, CapabilityParams};
    use std::collections::HashMap;

    fn test_building_registry() -> BuildingRegistry {
        let mut registry = BuildingRegistry::default();
        let mut shipyard_caps = HashMap::new();
        shipyard_caps.insert(
            "shipyard".to_string(),
            CapabilityParams {
                params: {
                    let mut m = HashMap::new();
                    m.insert("concurrent_builds".to_string(), 1.0);
                    m
                },
            },
        );
        registry.insert(BuildingDefinition {
            id: "shipyard".to_string(),
            name: "Shipyard".to_string(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 30,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: true,
            capabilities: shipyard_caps,
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
        });

        let mut port_caps = HashMap::new();
        port_caps.insert(
            "port".to_string(),
            CapabilityParams {
                params: {
                    let mut m = HashMap::new();
                    m.insert("ftl_range_bonus".to_string(), 10.0);
                    m.insert("travel_time_factor".to_string(), 0.8);
                    m
                },
            },
        );
        registry.insert(BuildingDefinition {
            id: "port".to_string(),
            name: "Port".to_string(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 40,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: true,
            capabilities: port_caps,
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
        });
        registry
    }

    // --- #113: System vs Planet building classification ---
    // Classification is now Lua-driven via BuildingRegistry.is_system_building()
    // Tested in scripting::building_api tests.

    #[test]
    fn system_buildings_has_shipyard() {
        let registry = test_building_registry();
        let sb = SystemBuildings {
            slots: vec![Some(BuildingId::new("shipyard")), None, None],
        };
        assert!(sb.has_shipyard(&registry));

        let sb_empty = SystemBuildings {
            slots: vec![Some(BuildingId::new("port")), None, None],
        };
        assert!(!sb_empty.has_shipyard(&registry));
    }

    #[test]
    fn system_buildings_has_port() {
        let registry = test_building_registry();
        let sb = SystemBuildings {
            slots: vec![None, Some(BuildingId::new("port")), None],
        };
        assert!(sb.has_port(&registry));

        let sb_empty = SystemBuildings {
            slots: vec![Some(BuildingId::new("shipyard")), None, None],
        };
        assert!(!sb_empty.has_port(&registry));
    }

    #[test]
    fn system_buildings_has_capability() {
        let registry = test_building_registry();
        let sb = SystemBuildings {
            slots: vec![
                Some(BuildingId::new("shipyard")),
                Some(BuildingId::new("port")),
                None,
            ],
        };
        assert!(sb.has_capability("shipyard", &registry));
        assert!(sb.has_capability("port", &registry));
        assert!(!sb.has_capability("nonexistent", &registry));
    }

    #[test]
    fn system_buildings_port_params() {
        let registry = test_building_registry();
        let sb = SystemBuildings {
            slots: vec![None, Some(BuildingId::new("port")), None],
        };
        assert_eq!(sb.port_ftl_range_bonus(&registry), 10.0);
        assert_eq!(sb.port_travel_time_factor(&registry), 0.8);

        // No port: defaults
        let sb_no_port = SystemBuildings {
            slots: vec![Some(BuildingId::new("shipyard")), None, None],
        };
        assert_eq!(sb_no_port.port_ftl_range_bonus(&registry), 0.0);
        assert_eq!(sb_no_port.port_travel_time_factor(&registry), 1.0);
    }

    #[test]
    fn system_building_queue_is_demolishing() {
        let bq = SystemBuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                order_id: 0,
                target_slot: 1,
                building_id: BuildingId::new("shipyard"),
                time_remaining: 15,
                minerals_refund: Amt::ZERO,
                energy_refund: Amt::ZERO,
            }],
            upgrade_queue: Vec::new(),
            next_order_id: 0,
        };
        assert!(bq.is_demolishing(1));
        assert!(!bq.is_demolishing(0));
        assert_eq!(bq.demolition_time_remaining(1), Some(15));
        assert_eq!(bq.demolition_time_remaining(0), None);
    }
}
