use bevy::prelude::*;

use crate::amount::Amt;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Planet, StarSystem};
use crate::components::Position;
use crate::knowledge::{
 FactSysParam, KnowledgeFact, PlayerVantage,
};
use crate::player::{AboardShip, Player, StationedAt};
use crate::scripting::building_api::BuildingId;
use crate::ship::{spawn_ship, CargoItem, Owner, Ship};
use crate::time_system::GameClock;

use super::{
    Colony, DeliverableStockpile, LastProductionTick, ResourceStockpile, SystemBuildings,
};

#[derive(Component)]
pub struct BuildQueue {
    pub queue: Vec<BuildOrder>,
}

/// #223: What kind of thing a `BuildOrder` builds.
///
/// Keeping `Ship` as the default preserves existing call sites (the previous
/// queue only built ships). `Deliverable` adds the new path used by #223:
/// completed deliverables are pushed into the system's `DeliverableStockpile`
/// instead of spawning a `Ship` entity.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum BuildKind {
    #[default]
    Ship,
    Deliverable {
        cargo_size: u32,
    },
}

pub struct BuildOrder {
    /// #223: What this order builds (Ship or Deliverable). Defaults to Ship
    /// for back-compat with existing construction sites.
    pub kind: BuildKind,
    pub design_id: String,
    pub display_name: String,
    pub minerals_cost: Amt,
    pub minerals_invested: Amt,
    pub energy_cost: Amt,
    pub energy_invested: Amt,
    /// #32: Total build time in hexadies
    pub build_time_total: i64,
    /// #32: Remaining build time in hexadies
    pub build_time_remaining: i64,
}

impl BuildOrder {
    pub fn is_complete(&self) -> bool {
        self.minerals_invested >= self.minerals_cost
            && self.energy_invested >= self.energy_cost
            && self.build_time_remaining <= 0
    }

    /// Returns the build time in hexadies for a given design_id.
    pub fn build_time_for(design_id: &str, design_registry: &crate::ship_design::ShipDesignRegistry) -> i64 {
        design_registry.build_time(design_id)
    }
}

// BuildingType enum has been removed. Use BuildingId + BuildingRegistry instead.
// BuildingId is defined in scripting::building_api.

#[derive(Component)]
pub struct Buildings {
    pub slots: Vec<Option<BuildingId>>, // None = empty slot
}

impl Buildings {
    /// Check if any slot contains a building with the given id.
    pub fn has_building(&self, id: &str) -> bool {
        self.slots.iter().any(|s| s.as_ref().is_some_and(|b| b.0 == id))
    }

    /// Check if any building in slots has the given capability (looked up via BuildingRegistry).
    pub fn has_capability(&self, capability: &str, registry: &crate::scripting::building_api::BuildingRegistry) -> bool {
        self.slots.iter().any(|slot| {
            slot.as_ref().is_some_and(|id| {
                registry.get(id.as_str()).is_some_and(|def| def.capabilities.contains_key(capability))
            })
        })
    }

    /// #35: Check if any slot contains a Shipyard
    pub fn has_shipyard(&self) -> bool {
        self.has_building("shipyard")
    }

    /// #46: Check if any slot contains a Port
    pub fn has_port(&self) -> bool {
        self.has_building("port")
    }
}

#[derive(Component, Default)]
pub struct BuildingQueue {
    pub queue: Vec<BuildingOrder>,
    pub demolition_queue: Vec<DemolitionOrder>,
    pub upgrade_queue: Vec<UpgradeOrder>,
}

pub struct BuildingOrder {
    pub building_id: BuildingId,
    pub target_slot: usize,
    pub minerals_remaining: Amt,
    pub energy_remaining: Amt,
    pub build_time_remaining: i64,
}

pub struct DemolitionOrder {
    pub target_slot: usize,
    pub building_id: BuildingId,
    pub time_remaining: i64,
    pub minerals_refund: Amt,
    pub energy_refund: Amt,
}

/// An order to upgrade an existing building in a slot to a new building type.
/// During the upgrade, the original building remains active. On completion,
/// the slot's building ID is replaced with the target.
pub struct UpgradeOrder {
    pub slot_index: usize,
    pub target_id: BuildingId,
    pub minerals_remaining: Amt,
    pub energy_remaining: Amt,
    pub build_time_remaining: i64,
}

impl BuildingQueue {
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

/// #32: build_time_remaining countdown, #35: shipyard check
/// #223: Deliverable orders land in the system's DeliverableStockpile rather
/// than spawning Ship entities.
#[allow(clippy::too_many_arguments)]
pub fn tick_build_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    design_registry: Res<crate::ship_design::ShipDesignRegistry>,
    building_registry: Res<crate::scripting::building_api::BuildingRegistry>,
    mut colonies: Query<(&Colony, &mut BuildQueue)>,
    mut stockpiles: Query<&mut ResourceStockpile, With<StarSystem>>,
    mut deliverable_stockpiles: Query<&mut DeliverableStockpile, With<StarSystem>>,
    positions: Query<&Position>,
    stars: Query<&StarSystem>,
    planets: Query<&Planet>,
    system_buildings: Query<&SystemBuildings>,
    mut events: MessageWriter<GameEvent>,
    empire_q: Query<Entity, With<crate::player::PlayerEmpire>>,
    player_q: Query<(&StationedAt, Option<&AboardShip>), With<Player>>,
    mut fact_sys: FactSysParam,
) {
    let ship_owner = empire_q
        .single()
        .map(Owner::Empire)
        .unwrap_or(Owner::Neutral);
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }

    // #249: Player vantage snapshot (once per tick).
    let player_info = player_q.iter().next();
    let player_system = player_info.map(|(s, _)| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| positions.get(s).ok())
        .map(|p| p.as_array());
    let player_aboard = player_info.map(|(_, a)| a.is_some()).unwrap_or(false);
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });

    // Per-order completion info (#223).
    enum Completion {
        Ship { design_id: String, display_name: String },
        Deliverable { definition_id: String, display_name: String },
    }

    // Collect build queue processing results
    struct BuildResult {
        system: Entity,
        minerals_consumed: Amt,
        energy_consumed: Amt,
        completed: Vec<Completion>,
    }

    let mut results: Vec<BuildResult> = Vec::new();

    for (colony, mut build_queue) in &mut colonies {
        let Some(sys) = colony.system(&planets) else { continue };

        // #35: Skip ship construction if system has no shipyard capability.
        // Deliverables also require a shipyard.
        let has_shipyard = system_buildings.get(sys).is_ok_and(|sb| sb.has_shipyard(&building_registry));
        if !build_queue.queue.is_empty() && !has_shipyard {
            warn!("System lacks a Shipyard; skipping construction");
            continue;
        }

        // Get current stockpile amounts for this system
        let Ok(stockpile) = stockpiles.get(sys) else { continue };
        let mut available_minerals = stockpile.minerals;
        let mut available_energy = stockpile.energy;
        let mut total_minerals_consumed = Amt::ZERO;
        let mut total_energy_consumed = Amt::ZERO;
        let mut completed: Vec<Completion> = Vec::new();

        for _ in 0..delta {
            if build_queue.queue.is_empty() {
                break;
            }
            let order = &mut build_queue.queue[0];

            let minerals_needed = order.minerals_cost.sub(order.minerals_invested);
            let minerals_transfer = minerals_needed.min(available_minerals);
            order.minerals_invested = order.minerals_invested.add(minerals_transfer);
            available_minerals = available_minerals.sub(minerals_transfer);
            total_minerals_consumed = total_minerals_consumed.add(minerals_transfer);

            let energy_needed = order.energy_cost.sub(order.energy_invested);
            let energy_transfer = energy_needed.min(available_energy);
            order.energy_invested = order.energy_invested.add(energy_transfer);
            available_energy = available_energy.sub(energy_transfer);
            total_energy_consumed = total_energy_consumed.add(energy_transfer);

            // #32: Decrement build time
            order.build_time_remaining -= 1;

            if build_queue.queue[0].is_complete() {
                let completed_order = build_queue.queue.remove(0);
                match completed_order.kind {
                    BuildKind::Ship => completed.push(Completion::Ship {
                        design_id: completed_order.design_id,
                        display_name: completed_order.display_name,
                    }),
                    BuildKind::Deliverable { .. } => {
                        completed.push(Completion::Deliverable {
                            definition_id: completed_order.design_id,
                            display_name: completed_order.display_name,
                        });
                    }
                }
            }
        }

        results.push(BuildResult {
            system: sys,
            minerals_consumed: total_minerals_consumed,
            energy_consumed: total_energy_consumed,
            completed,
        });
    }

    // Apply stockpile changes and spawn ships / enqueue deliverables
    for result in results {
        if let Ok(mut stockpile) = stockpiles.get_mut(result.system) {
            stockpile.minerals = stockpile.minerals.sub(result.minerals_consumed);
            stockpile.energy = stockpile.energy.sub(result.energy_consumed);
        }
        for c in result.completed {
            let sys_name = stars.get(result.system).map(|s| s.name.clone()).unwrap_or_default();
            match c {
                Completion::Ship { design_id, display_name } => {
                    if let Ok(pos) = positions.get(result.system) {
                        spawn_ship(
                            &mut commands,
                            &design_id,
                            display_name.clone(),
                            result.system,
                            *pos,
                            ship_owner,
                            &design_registry,
                        );
                        // #249: Dual-write ShipBuilt — routine, low-priority.
                        let event_id = fact_sys.allocate_event_id();
                        let desc = format!("{} built at {}", display_name, sys_name);
                        events.write(GameEvent {
                            id: event_id,
                            timestamp: clock.elapsed,
                            kind: GameEventKind::ShipBuilt,
                            description: desc.clone(),
                            related_system: Some(result.system),
                        });
                        let origin_pos: Option<[f64; 3]> = positions
                            .get(result.system)
                            .ok()
                            .map(|p| p.as_array());
                        if let (Some(v), Some(op)) = (vantage, origin_pos) {
                            let fact = KnowledgeFact::StructureBuilt {
                                event_id: Some(event_id),
                                system: Some(result.system),
                                kind: "ship".into(),
                                name: display_name.clone(),
                                destroyed: false,
                                detail: desc,
                            };
                            fact_sys.record(fact, op, clock.elapsed, &v);
                        }
                        info!("Ship built and launched: {}", display_name);
                    }
                }
                Completion::Deliverable { definition_id, display_name } => {
                    // #223: Push the new CargoItem into the system's DeliverableStockpile.
                    // If the component doesn't yet exist, add one via commands.
                    let item = CargoItem::Deliverable { definition_id: definition_id.clone() };
                    if let Ok(mut dstock) = deliverable_stockpiles.get_mut(result.system) {
                        dstock.push(item);
                    } else {
                        commands.entity(result.system).insert(DeliverableStockpile {
                            items: vec![item],
                        });
                    }
                    let event_id = fact_sys.allocate_event_id();
                    let desc = format!("Deliverable '{}' produced at {}", display_name, sys_name);
                    events.write(GameEvent {
                        id: event_id,
                        timestamp: clock.elapsed,
                        kind: GameEventKind::ShipBuilt,
                        description: desc.clone(),
                        related_system: Some(result.system),
                    });
                    let origin_pos: Option<[f64; 3]> =
                        positions.get(result.system).ok().map(|p| p.as_array());
                    if let (Some(v), Some(op)) = (vantage, origin_pos) {
                        let fact = KnowledgeFact::StructureBuilt {
                            event_id: Some(event_id),
                            system: Some(result.system),
                            kind: "deliverable".into(),
                            name: display_name.clone(),
                            destroyed: false,
                            detail: desc,
                        };
                        fact_sys.record(fact, op, clock.elapsed, &v);
                    }
                    info!("Deliverable produced: {} @ {}", display_name, sys_name);
                }
            }
        }
    }
}

pub fn tick_building_queue(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut query: Query<(Entity, &Colony, &mut BuildingQueue, &mut Buildings)>,
    mut stockpiles: Query<&mut ResourceStockpile, With<StarSystem>>,
    planets: Query<&Planet>,
    mut event_system: ResMut<crate::event_system::EventSystem>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }

    // Collect changes per system to apply afterwards
    struct SystemDelta {
        minerals_consumed: Amt,
        energy_consumed: Amt,
        minerals_refunded: Amt,
        energy_refunded: Amt,
    }
    let mut system_deltas: std::collections::HashMap<Entity, SystemDelta> = std::collections::HashMap::new();

    for (colony_entity, colony, mut bq, mut buildings) in &mut query {
        let Some(sys) = colony.system(&planets) else { continue };

        // Get available resources from system stockpile
        let Ok(stockpile) = stockpiles.get(sys) else { continue };
        let mut available_minerals = stockpile.minerals;
        let mut available_energy = stockpile.energy;

        // Track how much we consume/refund for this colony
        let mut minerals_consumed = Amt::ZERO;
        let mut energy_consumed = Amt::ZERO;
        let mut minerals_refunded = Amt::ZERO;
        let mut energy_refunded = Amt::ZERO;

        // Also account for deltas already accumulated for this system by previous colonies
        if let Some(existing) = system_deltas.get(&sys) {
            available_minerals = available_minerals.sub(existing.minerals_consumed).add(existing.minerals_refunded);
            available_energy = available_energy.sub(existing.energy_consumed).add(existing.energy_refunded);
        }

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

            // #232: Only advance the countdown when resources actually moved
            // this tick, or when no resources are needed anymore. Otherwise
            // a starved build would have its timer drain past zero while
            // the completion check (which also requires 0 remaining cost)
            // keeps blocking completion.
            let transferred = minerals_transfer > Amt::ZERO || energy_transfer > Amt::ZERO;
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
                if completed.target_slot < buildings.slots.len() {
                    info!(
                        "Building '{}' completed in slot {}",
                        completed.building_id, completed.target_slot
                    );
                    buildings.slots[completed.target_slot] = Some(completed.building_id);
                } else {
                    warn!(
                        "Building '{}' completed but target slot {} is out of range (max {})",
                        completed.building_id,
                        completed.target_slot,
                        buildings.slots.len()
                    );
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
                        .map(|bid| bid.0.clone())
                        .unwrap_or_else(|| "Unknown".to_string());
                    buildings.slots[slot_idx] = None;
                    minerals_refunded = minerals_refunded.add(completed.minerals_refund);
                    energy_refunded = energy_refunded.add(completed.energy_refund);
                    info!(
                        "Building {} demolished in slot {}, refunded M:{} E:{}",
                        building_name, slot_idx, completed.minerals_refund, completed.energy_refund
                    );
                    event_system.fire_event(
                        "building_demolished",
                        Some(colony_entity),
                        clock.elapsed,
                    );
                    let mut payload = std::collections::HashMap::new();
                    payload.insert("cause".to_string(), "demolished".to_string());
                    payload.insert("building_id".to_string(), building_name);
                    payload.insert("slot".to_string(), slot_idx.to_string());
                    event_system.fire_event_with_payload(
                        "macrocosmo:building_lost",
                        Some(colony_entity),
                        clock.elapsed,
                        payload,
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

                // #232: See construction-queue branch above — only tick the
                // timer when progress is happening or no progress is needed.
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
        // Process completed upgrades in reverse to keep indices valid
        for idx in completed_upgrades.into_iter().rev() {
            let completed = bq.upgrade_queue.remove(idx);
            if completed.slot_index < buildings.slots.len() {
                let old_name = buildings.slots[completed.slot_index]
                    .as_ref()
                    .map(|bid| bid.0.clone())
                    .unwrap_or_else(|| "empty".to_string());
                buildings.slots[completed.slot_index] = Some(completed.target_id.clone());
                info!(
                    "Building upgrade completed: {} -> {} in slot {}",
                    old_name, completed.target_id, completed.slot_index
                );
                event_system.fire_event(
                    "building_upgraded",
                    Some(colony_entity),
                    clock.elapsed,
                );
            }
        }

        let entry = system_deltas.entry(sys).or_insert(SystemDelta {
            minerals_consumed: Amt::ZERO,
            energy_consumed: Amt::ZERO,
            minerals_refunded: Amt::ZERO,
            energy_refunded: Amt::ZERO,
        });
        entry.minerals_consumed = entry.minerals_consumed.add(minerals_consumed);
        entry.energy_consumed = entry.energy_consumed.add(energy_consumed);
        entry.minerals_refunded = entry.minerals_refunded.add(minerals_refunded);
        entry.energy_refunded = entry.energy_refunded.add(energy_refunded);
    }

    // Apply all stockpile changes
    for (sys, delta) in system_deltas {
        if let Ok(mut stockpile) = stockpiles.get_mut(sys) {
            stockpile.minerals = stockpile.minerals.sub(delta.minerals_consumed).add(delta.minerals_refunded);
            stockpile.energy = stockpile.energy.sub(delta.energy_consumed).add(delta.energy_refunded);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship_design::{ShipDesignDefinition, ShipDesignRegistry};

    fn test_design_registry() -> ShipDesignRegistry {
        let mut registry = ShipDesignRegistry::default();
        registry.insert(ShipDesignDefinition {
            id: "explorer_mk1".to_string(),
            name: "Explorer Mk.I".to_string(),
            description: String::new(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            can_survey: true,
            can_colonize: false,
            maintenance: Amt::new(0, 500),
            build_cost_minerals: Amt::units(200),
            build_cost_energy: Amt::units(100),
            build_time: 60,
            hp: 50.0,
            sublight_speed: 0.75,
            ftl_range: 10.0,
            revision: 0,
        });
        registry.insert(ShipDesignDefinition {
            id: "colony_ship_mk1".to_string(),
            name: "Colony Ship Mk.I".to_string(),
            description: String::new(),
            hull_id: "frigate".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: true,
            maintenance: Amt::units(1),
            build_cost_minerals: Amt::units(500),
            build_cost_energy: Amt::units(300),
            build_time: 120,
            hp: 100.0,
            sublight_speed: 0.5,
            ftl_range: 15.0,
            revision: 0,
        });
        registry.insert(ShipDesignDefinition {
            id: "courier_mk1".to_string(),
            name: "Courier Mk.I".to_string(),
            description: String::new(),
            hull_id: "courier_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::new(0, 300),
            build_cost_minerals: Amt::units(100),
            build_cost_energy: Amt::units(50),
            build_time: 30,
            hp: 35.0,
            sublight_speed: 0.80,
            ftl_range: 0.0,
            revision: 0,
        });
        registry
    }

    fn make_order(minerals_cost: Amt, minerals_invested: Amt, energy_cost: Amt, energy_invested: Amt) -> BuildOrder {
        let build_time = 60;
        BuildOrder {
            kind: BuildKind::default(),
            design_id: "explorer_mk1".to_string(),
            display_name: "Explorer".to_string(),
            minerals_cost,
            minerals_invested,
            energy_cost,
            energy_invested,
            build_time_total: build_time,
            build_time_remaining: 0, // for is_complete tests, set to 0
        }
    }

    #[test]
    fn build_order_complete_when_both_met() {
        let order = make_order(Amt::units(100), Amt::units(100), Amt::units(50), Amt::units(50));
        assert!(order.is_complete());
    }

    #[test]
    fn build_order_incomplete_minerals_short() {
        let order = make_order(Amt::units(100), Amt::units(80), Amt::units(50), Amt::units(50));
        assert!(!order.is_complete());
    }

    #[test]
    fn build_order_incomplete_energy_short() {
        let order = make_order(Amt::units(100), Amt::units(100), Amt::units(50), Amt::units(30));
        assert!(!order.is_complete());
    }

    #[test]
    fn build_order_incomplete_time_remaining() {
        let mut order = make_order(Amt::units(100), Amt::units(100), Amt::units(50), Amt::units(50));
        order.build_time_remaining = 5;
        assert!(!order.is_complete());
    }

    // BuildingType enum tests replaced with BuildingId + BuildingRegistry tests.
    // Production bonus, build cost, build time, and maintenance values are now
    // tested in scripting::building_api tests (loaded from Lua).

    #[test]
    fn buildings_slots_empty() {
        let buildings = Buildings {
            slots: vec![None; 5],
        };
        assert_eq!(buildings.slots.len(), 5);
        assert!(buildings.slots.iter().all(|s| s.is_none()));
    }

    #[test]
    fn buildings_slots_with_buildings() {
        let mut buildings = Buildings {
            slots: vec![None; 5],
        };
        buildings.slots[0] = Some(BuildingId::new("mine"));
        buildings.slots[2] = Some(BuildingId::new("power_plant"));

        assert_eq!(buildings.slots[0], Some(BuildingId::new("mine")));
        assert_eq!(buildings.slots[1], None);
        assert_eq!(buildings.slots[2], Some(BuildingId::new("power_plant")));
    }

    #[test]
    fn has_shipyard_true() {
        let buildings = Buildings {
            slots: vec![Some(BuildingId::new("mine")), Some(BuildingId::new("shipyard")), None],
        };
        assert!(buildings.has_shipyard());
    }

    #[test]
    fn has_shipyard_false() {
        let buildings = Buildings {
            slots: vec![Some(BuildingId::new("mine")), Some(BuildingId::new("power_plant")), None],
        };
        assert!(!buildings.has_shipyard());
    }

    #[test]
    fn production_focus_labels() {
        assert_eq!(super::super::ProductionFocus::balanced().label(), "Balanced");
        assert_eq!(super::super::ProductionFocus::minerals().label(), "Minerals");
        assert_eq!(super::super::ProductionFocus::energy().label(), "Energy");
        assert_eq!(super::super::ProductionFocus::research().label(), "Research");
    }

    #[test]
    fn build_order_build_time_for() {
        let registry = test_design_registry();
        assert_eq!(BuildOrder::build_time_for("explorer_mk1", &registry), 60);
        assert_eq!(BuildOrder::build_time_for("colony_ship_mk1", &registry), 120);
        assert_eq!(BuildOrder::build_time_for("courier_mk1", &registry), 30);
        assert_eq!(BuildOrder::build_time_for("unknown", &registry), 60);
    }

    // --- #46: Port tests ---

    #[test]
    fn has_port_true() {
        let buildings = Buildings {
            slots: vec![Some(BuildingId::new("mine")), Some(BuildingId::new("port")), None],
        };
        assert!(buildings.has_port());
    }

    #[test]
    fn has_port_false() {
        let buildings = Buildings {
            slots: vec![Some(BuildingId::new("mine")), Some(BuildingId::new("shipyard")), None],
        };
        assert!(!buildings.has_port());
    }

    #[test]
    fn building_queue_is_demolishing() {
        let bq = BuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                target_slot: 2,
                building_id: BuildingId::new("mine"),
                time_remaining: 5,
                minerals_refund: Amt::ZERO,
                energy_refund: Amt::ZERO,
            }],
            upgrade_queue: Vec::new(),
        };
        assert!(bq.is_demolishing(2));
        assert!(!bq.is_demolishing(0));
        assert_eq!(bq.demolition_time_remaining(2), Some(5));
        assert_eq!(bq.demolition_time_remaining(0), None);
    }
}
