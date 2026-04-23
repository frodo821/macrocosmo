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

/// System-level buildings capacity on StarSystem entities.
/// The actual building data lives on station Ship entities via `SlotAssignment`.
#[derive(Component)]
pub struct SystemBuildings {
    pub max_slots: usize,
}

impl Default for SystemBuildings {
    fn default() -> Self {
        Self {
            max_slots: DEFAULT_SYSTEM_BUILDING_SLOTS,
        }
    }
}

/// Slot assignment for station ships that occupy a system building slot.
/// The `usize` is the slot number within the system's `SystemBuildings.max_slots`.
#[derive(Component, Clone, Copy, Debug)]
pub struct SlotAssignment(pub usize);

// ---------------------------------------------------------------------------
// Reverse mapping: ship design_id -> BuildingId
// ---------------------------------------------------------------------------

/// Build a reverse index from ship `design_id` to `BuildingId` using the
/// `BuildingRegistry`. Entries that have a `ship_design_id` field are included.
pub fn build_reverse_design_map(
    registry: &BuildingRegistry,
) -> std::collections::HashMap<String, BuildingId> {
    registry
        .buildings
        .values()
        .filter_map(|def| {
            def.ship_design_id
                .as_ref()
                .map(|did| (did.clone(), BuildingId::new(&def.id)))
        })
        .collect()
}

/// Resolve a station ship's `Ship.design_id` to a `BuildingId` via the registry.
pub fn ship_to_building_id(ship: &Ship, registry: &BuildingRegistry) -> Option<BuildingId> {
    registry
        .buildings
        .values()
        .find(|def| {
            def.ship_design_id
                .as_deref()
                .is_some_and(|did| did == ship.design_id)
        })
        .map(|def| BuildingId::new(&def.id))
}

// ---------------------------------------------------------------------------
// Query-based helper functions
// ---------------------------------------------------------------------------

/// The query type used by most helper functions to find station ships.
pub type StationShipQuery<'w, 's> =
    Query<'w, 's, (Entity, &'static Ship, &'static ShipState, &'static SlotAssignment)>;

/// Find all station ships at a system with their slot assignments.
pub fn station_ships_at_system<'a>(
    system: Entity,
    ships: &'a Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
) -> Vec<(Entity, &'a Ship, usize)> {
    ships
        .iter()
        .filter_map(|(entity, ship, state, slot)| match state {
            ShipState::InSystem { system: s } if *s == system => Some((entity, ship, slot.0)),
            ShipState::Refitting { system: s, .. } if *s == system => {
                Some((entity, ship, slot.0))
            }
            _ => None,
        })
        .collect()
}

/// Check if system has a building with the given id.
pub fn system_has_building(
    system: Entity,
    building_id: &str,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> bool {
    let reverse = build_reverse_design_map(registry);
    for (_entity, ship, state, _slot) in ships.iter() {
        let in_system = match state {
            ShipState::InSystem { system: s } => *s == system,
            ShipState::Refitting { system: s, .. } => *s == system,
            _ => false,
        };
        if !in_system {
            continue;
        }
        if let Some(bid) = reverse.get(&ship.design_id) {
            if bid.0 == building_id {
                return true;
            }
        }
    }
    false
}

/// Check if system has a building with the given capability.
pub fn system_has_capability(
    system: Entity,
    capability: &str,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> bool {
    let reverse = build_reverse_design_map(registry);
    for (_entity, ship, state, _slot) in ships.iter() {
        let in_system = match state {
            ShipState::InSystem { system: s } => *s == system,
            ShipState::Refitting { system: s, .. } => *s == system,
            _ => false,
        };
        if !in_system {
            continue;
        }
        if let Some(bid) = reverse.get(&ship.design_id) {
            if let Some(def) = registry.get(bid.as_str()) {
                if def.capabilities.contains_key(capability) {
                    return true;
                }
            }
        }
    }
    false
}

/// Get capability param from first building at system with given capability.
pub fn system_capability_param(
    system: Entity,
    capability: &str,
    param: &str,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> Option<f64> {
    let reverse = build_reverse_design_map(registry);
    for (_entity, ship, state, _slot) in ships.iter() {
        let in_system = match state {
            ShipState::InSystem { system: s } => *s == system,
            ShipState::Refitting { system: s, .. } => *s == system,
            _ => false,
        };
        if !in_system {
            continue;
        }
        if let Some(bid) = reverse.get(&ship.design_id) {
            if let Some(def) = registry.get(bid.as_str()) {
                if let Some(cap) = def.capabilities.get(capability) {
                    return cap.get(param);
                }
            }
        }
    }
    None
}

/// Check if system has a shipyard (via capability).
pub fn system_has_shipyard(
    system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> bool {
    system_has_capability(system, "shipyard", ships, registry)
}

/// Check if system has a port (via capability).
pub fn system_has_port(
    system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> bool {
    system_has_capability(system, "port", ships, registry)
}

/// Get the port FTL range bonus, if present.
pub fn port_ftl_range_bonus(
    system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> f64 {
    system_capability_param(system, "port", "ftl_range_bonus", ships, registry).unwrap_or(0.0)
}

/// Get the port travel time factor, if present.
pub fn port_travel_time_factor(
    system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> f64 {
    system_capability_param(system, "port", "travel_time_factor", ships, registry).unwrap_or(1.0)
}

/// Find the next available slot number at a system.
pub fn next_free_slot(
    system: Entity,
    max_slots: usize,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
) -> Option<usize> {
    let occupied: std::collections::HashSet<usize> = ships
        .iter()
        .filter_map(|(_e, _ship, state, slot)| {
            let in_system = match state {
                ShipState::InSystem { system: s } => *s == system,
                ShipState::Refitting { system: s, .. } => *s == system,
                _ => false,
            };
            if in_system { Some(slot.0) } else { None }
        })
        .collect();
    (0..max_slots).find(|i| !occupied.contains(i))
}

/// Get the BuildingId for a specific slot at a system, if occupied by a station ship.
pub fn building_id_in_slot(
    system: Entity,
    slot: usize,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> Option<BuildingId> {
    let reverse = build_reverse_design_map(registry);
    for (_entity, ship, state, slot_assign) in ships.iter() {
        if slot_assign.0 != slot {
            continue;
        }
        let in_system = match state {
            ShipState::InSystem { system: s } => *s == system,
            ShipState::Refitting { system: s, .. } => *s == system,
            _ => false,
        };
        if in_system {
            return reverse.get(&ship.design_id).cloned();
        }
    }
    None
}

/// Collect all (slot_index, BuildingId) pairs for occupied slots at a system.
pub fn occupied_slots(
    system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> Vec<(usize, BuildingId)> {
    let reverse = build_reverse_design_map(registry);
    let mut result = Vec::new();
    for (_entity, ship, state, slot) in ships.iter() {
        let in_system = match state {
            ShipState::InSystem { system: s } => *s == system,
            ShipState::Refitting { system: s, .. } => *s == system,
            _ => false,
        };
        if !in_system {
            continue;
        }
        if let Some(bid) = reverse.get(&ship.design_id) {
            result.push((slot.0, bid.clone()));
        }
    }
    result.sort_by_key(|(slot, _)| *slot);
    result
}

/// Build a Vec<Option<BuildingId>> view of slots (for backward-compatible rendering).
/// Returns a vec of size `max_slots` with Some(BuildingId) in occupied slots.
pub fn slots_view(
    system: Entity,
    max_slots: usize,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    registry: &BuildingRegistry,
) -> Vec<Option<BuildingId>> {
    let mut slots = vec![None; max_slots];
    for (slot_idx, bid) in occupied_slots(system, ships, registry) {
        if slot_idx < max_slots {
            slots[slot_idx] = Some(bid);
        }
    }
    slots
}

/// Find the ship entity in a specific slot at a system.
pub fn ship_in_slot(
    system: Entity,
    slot: usize,
    ships: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
) -> Option<Entity> {
    ships.iter().find_map(|(entity, _ship, state, slot_assign)| {
        if slot_assign.0 != slot {
            return None;
        }
        let in_system = match state {
            ShipState::InSystem { system: s } => *s == system,
            ShipState::Refitting { system: s, .. } => *s == system,
            _ => false,
        };
        if in_system { Some(entity) } else { None }
    })
}

// ---------------------------------------------------------------------------
// SystemBuildingQueue
// ---------------------------------------------------------------------------

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
/// Now reads station ships with SlotAssignment instead of SystemBuildings slots.
pub fn sync_system_building_maintenance(
    registry: Res<BuildingRegistry>,
    system_buildings_q: Query<Entity, With<SystemBuildings>>,
    station_ships: Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    mut colonies: Query<(&Colony, &mut MaintenanceCost)>,
    planets: Query<&Planet>,
) {
    let reverse = build_reverse_design_map(&registry);
    let system_entities: Vec<Entity> = system_buildings_q.iter().collect();

    for sys_entity in &system_entities {
        // Get station ships at this system
        let stations = station_ships_at_system(*sys_entity, &station_ships);

        for (colony, mut maint) in &mut colonies {
            if colony.system(&planets) != Some(*sys_entity) {
                continue;
            }

            // Build set of active slot indices for cleanup
            let active_slots: std::collections::HashSet<usize> =
                stations.iter().map(|(_, _, slot)| *slot).collect();

            // Push maintenance modifiers for each station ship
            for &(_ship_entity, ship, slot_idx) in &stations {
                let maint_id = format!("sys_building_maint_{}", slot_idx);
                if let Some(bid) = reverse.get(&ship.design_id) {
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

            // Clean up modifiers for slots that no longer have station ships
            let to_remove: Vec<String> = maint
                .energy_per_hexadies
                .modifiers()
                .iter()
                .filter(|m| {
                    m.id.starts_with("sys_building_maint_")
                        && m.id
                            .strip_prefix("sys_building_maint_")
                            .and_then(|s| s.parse::<usize>().ok())
                            .is_some_and(|idx| !active_slots.contains(&idx))
                })
                .map(|m| m.id.clone())
                .collect();
            for id in to_remove {
                maint.energy_per_hexadies.pop_modifier(&id);
            }

            // Only apply to first colony in the system
            break;
        }
    }
}

/// Sync `system.*` modifiers from system-building definitions onto the
/// `SystemModifiers` component of each StarSystem. This replaces the old
/// capability-based queries (`system_has_shipyard`, `port_ftl_range_bonus`, etc.)
/// with modifier-driven reads.
pub fn sync_system_capability_modifiers(
    registry: Res<BuildingRegistry>,
    mut systems: Query<(Entity, &mut crate::galaxy::SystemModifiers)>,
    station_ships: Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
) {
    let reverse = build_reverse_design_map(&registry);

    for (sys_entity, mut sys_mods) in &mut systems {
        // Clear previous building-sourced modifiers (prefix "syscap:")
        clear_prefixed(&mut sys_mods.shipyard_capacity, "syscap:");
        clear_prefixed(&mut sys_mods.port_ftl_range_bonus, "syscap:");
        clear_prefixed(&mut sys_mods.port_travel_time_factor, "syscap:");
        clear_prefixed(&mut sys_mods.port_repair, "syscap:");

        for (ship_entity, ship, state, _slot) in &station_ships {
            let in_system = match state {
                ShipState::InSystem { system } => *system == sys_entity,
                ShipState::Refitting { system, .. } => *system == sys_entity,
                _ => false,
            };
            if !in_system {
                continue;
            }
            let Some(bid) = reverse.get(&ship.design_id) else {
                continue;
            };
            let Some(def) = registry.get(bid.as_str()) else {
                continue;
            };
            for pm in &def.modifiers {
                let id = format!("syscap:{}[{:?}]:{}", def.id, ship_entity, pm.target);
                let modifier = pm.to_modifier(id, &def.name);
                match pm.target.as_str() {
                    "system.shipyard_capacity" => {
                        sys_mods.shipyard_capacity.push_modifier(modifier);
                    }
                    "system.port_ftl_range_bonus" => {
                        sys_mods.port_ftl_range_bonus.push_modifier(modifier);
                    }
                    "system.port_travel_time_factor" => {
                        sys_mods.port_travel_time_factor.push_modifier(modifier);
                    }
                    "system.port_repair" => {
                        sys_mods.port_repair.push_modifier(modifier);
                    }
                    _ => {} // Not a system capability target — handled by colony sync
                }
            }
        }
    }
}

/// Remove all modifiers with the given prefix from a ScopedModifiers.
fn clear_prefixed(scope: &mut crate::modifier::ScopedModifiers, prefix: &str) {
    let to_remove: Vec<String> = scope
        .value()
        .modifiers()
        .iter()
        .filter(|m| m.id.starts_with(prefix))
        .map(|m| m.id.clone())
        .collect();
    for id in to_remove {
        scope.pop_modifier(&id);
    }
}

/// Which sub-queue a `SystemBuildingQueue` head order came from. Mirrors
/// the `BuildingQueue` scheduler: serializes construction / upgrade /
/// demolition so only the oldest order advances per tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SysQueueKind {
    Construction,
    Upgrade,
    Demolition,
}

fn next_pending_sys(bq: &SystemBuildingQueue) -> Option<SysQueueKind> {
    let c = bq.queue.first().map(|o| o.order_id);
    let u = bq.upgrade_queue.first().map(|o| o.order_id);
    let d = bq.demolition_queue.first().map(|o| o.order_id);
    [
        (c, SysQueueKind::Construction),
        (u, SysQueueKind::Upgrade),
        (d, SysQueueKind::Demolition),
    ]
    .into_iter()
    .filter_map(|(id, k)| id.map(|id| (id, k)))
    .min_by_key(|(id, _)| *id)
    .map(|(_, k)| k)
}

/// Tick system-level building construction/demolition queues on StarSystem entities.
/// On construction completion, spawn a station Ship with SlotAssignment.
/// On demolition, despawn the station Ship in that slot.
#[allow(clippy::too_many_arguments)]
pub fn tick_system_building_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(
        Entity,
        &mut SystemBuildingQueue,
        &SystemBuildings,
        &mut ResourceStockpile,
        &Position,
    )>,
    mut event_system: ResMut<crate::event_system::EventSystem>,
    building_registry: Res<BuildingRegistry>,
    design_registry: Res<ShipDesignRegistry>,
    faction_q: Query<&crate::faction::FactionOwner>,
    ship_q: Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }

    for (system_entity, mut bq, buildings, mut stockpile, sys_position) in &mut query {
        let mut available_minerals = stockpile.minerals;
        let mut available_energy = stockpile.energy;
        let mut minerals_consumed = Amt::ZERO;
        let mut energy_consumed = Amt::ZERO;
        let mut minerals_refunded = Amt::ZERO;
        let mut energy_refunded = Amt::ZERO;

        // Serialize the three sub-queues by `order_id`. See
        // `BuildingQueue::next_pending` for the rationale.
        for _ in 0..delta {
            let Some(kind) = next_pending_sys(&bq) else {
                break;
            };
            match kind {
                SysQueueKind::Construction => {
                    let order = &mut bq.queue[0];

                    let minerals_transfer = order.minerals_remaining.min(available_minerals);
                    order.minerals_remaining = order.minerals_remaining.sub(minerals_transfer);
                    available_minerals = available_minerals.sub(minerals_transfer);
                    minerals_consumed = minerals_consumed.add(minerals_transfer);

                    let energy_transfer = order.energy_remaining.min(available_energy);
                    order.energy_remaining = order.energy_remaining.sub(energy_transfer);
                    available_energy = available_energy.sub(energy_transfer);
                    energy_consumed = energy_consumed.add(energy_transfer);

                    let transferred =
                        minerals_transfer > Amt::ZERO || energy_transfer > Amt::ZERO;
                    let no_more_needed = order.minerals_remaining == Amt::ZERO
                        && order.energy_remaining == Amt::ZERO;
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
                        let completed_slot = completed.target_slot;
                        if completed_slot < buildings.max_slots {
                            info!(
                                "System building '{}' completed in slot {}",
                                completed.building_id, completed_slot
                            );
                            let completed_id = completed.building_id.0.clone();
                            // #281: Fire macrocosmo:building_built for system-level construction.
                            let mut payload = std::collections::HashMap::new();
                            payload.insert("cause".to_string(), "construction".to_string());
                            payload.insert("building_id".to_string(), completed_id.clone());
                            payload.insert("slot".to_string(), completed_slot.to_string());
                            payload.insert(
                                "system".to_string(),
                                system_entity.to_bits().to_string(),
                            );
                            event_system.fire_event_with_payload(
                                Some(system_entity),
                                clock.elapsed,
                                Box::new(crate::event_system::LuaDefinedEventContext::new(
                                    crate::event_system::BUILDING_BUILT_EVENT,
                                    payload,
                                )),
                            );

                            // Spawn a station Ship with SlotAssignment if the building has ship_design_id.
                            if let Some(def) = building_registry.get(&completed_id) {
                                if let Some(ref design_id) = def.ship_design_id {
                                    let owner = faction_q
                                        .get(system_entity)
                                        .ok()
                                        .map(|fo| Owner::Empire(fo.0))
                                        .unwrap_or(Owner::Neutral);
                                    let station_name = def.name.clone();
                                    let ship_entity = crate::ship::spawn_ship(
                                        &mut commands,
                                        design_id,
                                        station_name,
                                        system_entity,
                                        *sys_position,
                                        owner,
                                        &design_registry,
                                    );
                                    commands
                                        .entity(ship_entity)
                                        .insert(SlotAssignment(completed_slot));
                                    info!(
                                        "Spawned station ship '{}' for system building '{}' in slot {}",
                                        design_id, def.id, completed_slot
                                    );
                                }
                            }
                        }
                    }
                }
                SysQueueKind::Upgrade => {
                    let upgrade = &mut bq.upgrade_queue[0];

                    let minerals_transfer = upgrade.minerals_remaining.min(available_minerals);
                    upgrade.minerals_remaining = upgrade.minerals_remaining.sub(minerals_transfer);
                    available_minerals = available_minerals.sub(minerals_transfer);
                    minerals_consumed = minerals_consumed.add(minerals_transfer);

                    let energy_transfer = upgrade.energy_remaining.min(available_energy);
                    upgrade.energy_remaining = upgrade.energy_remaining.sub(energy_transfer);
                    available_energy = available_energy.sub(energy_transfer);
                    energy_consumed = energy_consumed.add(energy_transfer);

                    let transferred =
                        minerals_transfer > Amt::ZERO || energy_transfer > Amt::ZERO;
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
                        let completed = bq.upgrade_queue.remove(0);
                        if completed.slot_index < buildings.max_slots {
                            // Find the old building name from the ship in this slot.
                            let old_name = building_id_in_slot(
                                system_entity,
                                completed.slot_index,
                                &ship_q,
                                &building_registry,
                            )
                            .map(|bid| bid.0)
                            .unwrap_or_else(|| "empty".to_string());

                            // Despawn old station ship and spawn the upgraded one.
                            if let Some(old_ship) =
                                ship_in_slot(system_entity, completed.slot_index, &ship_q)
                            {
                                commands.entity(old_ship).despawn();
                            }
                            // Spawn the new station ship for the upgraded building.
                            if let Some(def) =
                                building_registry.get(completed.target_id.as_str())
                            {
                                if let Some(ref design_id) = def.ship_design_id {
                                    let owner = faction_q
                                        .get(system_entity)
                                        .ok()
                                        .map(|fo| Owner::Empire(fo.0))
                                        .unwrap_or(Owner::Neutral);
                                    let ship_entity = crate::ship::spawn_ship(
                                        &mut commands,
                                        design_id,
                                        def.name.clone(),
                                        system_entity,
                                        *sys_position,
                                        owner,
                                        &design_registry,
                                    );
                                    commands
                                        .entity(ship_entity)
                                        .insert(SlotAssignment(completed.slot_index));
                                }
                            }

                            info!(
                                "System building upgrade completed: {} -> {} in slot {}",
                                old_name, completed.target_id, completed.slot_index
                            );
                            event_system.fire_event(
                                "building_upgraded",
                                Some(system_entity),
                                clock.elapsed,
                            );
                            let mut payload = std::collections::HashMap::new();
                            payload.insert("cause".to_string(), "upgrade".to_string());
                            payload.insert(
                                "building_id".to_string(),
                                completed.target_id.0.clone(),
                            );
                            payload.insert("previous_id".to_string(), old_name);
                            payload.insert(
                                "slot".to_string(),
                                completed.slot_index.to_string(),
                            );
                            payload.insert(
                                "system".to_string(),
                                system_entity.to_bits().to_string(),
                            );
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
                }
                SysQueueKind::Demolition => {
                    let demo = &mut bq.demolition_queue[0];
                    demo.time_remaining -= 1;
                    if demo.time_remaining <= 0 {
                        let completed = bq.demolition_queue.remove(0);
                        let slot_idx = completed.target_slot;
                        if slot_idx < buildings.max_slots {
                            let building_name = completed.building_id.0.as_str();
                            info!(
                                "System building {} demolished in slot {}, refunded M:{} E:{}",
                                building_name,
                                slot_idx,
                                completed.minerals_refund,
                                completed.energy_refund
                            );

                            // Despawn the station ship in this slot.
                            if let Some(ship_entity) =
                                ship_in_slot(system_entity, slot_idx, &ship_q)
                            {
                                commands.entity(ship_entity).despawn();
                                info!(
                                    "Despawned station ship in slot {} for demolished building '{}'",
                                    slot_idx, building_name
                                );
                            }

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
            ship_design_id: Some("station_shipyard_v1".to_string()),
            colony_slots: None,
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
            ship_design_id: Some("station_port_v1".to_string()),
            colony_slots: None,
        });
        registry
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

    #[test]
    fn system_buildings_default_max_slots() {
        let sb = SystemBuildings::default();
        assert_eq!(sb.max_slots, DEFAULT_SYSTEM_BUILDING_SLOTS);
    }
}
