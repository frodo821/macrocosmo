use bevy::prelude::*;

use crate::ai::command_consumer::BuildResearchParams;
use crate::ai::command_handlers::find_empire_entity;
use crate::ai::command_params::{
    BUILDING_ID, DEFINITION_ID, DESIGN_ID, required_str, target_system,
};
use crate::colony::Colony;
use crate::colony::building_queue::{BuildKind, BuildOrder, BuildQueue, BuildingOrder};
use crate::galaxy::{Planet, Sovereignty, StarSystem};
use crate::player::{Empire, Faction};
use crate::ship::Owner;
use macrocosmo_core::amount::Amt;

/// Handle `build_ship`: queue construction of the specified ship design at
/// a system with a shipyard owned by the faction.
pub(crate) fn handle_build_ship(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let design_id = match required_str(params, DESIGN_ID) {
        Ok(s) => s.to_string(),
        Err(_) => {
            warn!("build_ship command missing design_id param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("build_ship: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let target_system = target_system(params);

    queue_ship_at_shipyard(empire_entity, &design_id, target_system, sovereignty, br);
}

/// Handle `fortify_system`: queue construction of a default combat ship
/// design at a system with a shipyard.
pub(crate) fn handle_fortify_system(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("fortify_system: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let target_system = target_system(params);

    let design_id = match required_str(params, DESIGN_ID) {
        Ok(s) => s.to_string(),
        _ => {
            let Some(ref registry) = br.design_registry else {
                warn!("fortify_system: ShipDesignRegistry not available");
                return;
            };
            let combat_design = registry
                .designs
                .values()
                .find(|d| d.is_direct_buildable && !d.can_survey && !d.can_colonize);
            match combat_design {
                Some(d) => d.id.clone(),
                None => match registry.designs.values().find(|d| d.is_direct_buildable) {
                    Some(d) => d.id.clone(),
                    None => {
                        debug!("fortify_system: no buildable designs in registry");
                        return;
                    }
                },
            }
        }
    };

    queue_ship_at_shipyard(empire_entity, &design_id, target_system, sovereignty, br);
}

/// Handle `build_structure`: queue a building at a colony or system owned by
/// the faction.
pub(crate) fn handle_build_structure(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let building_id_str = match required_str(params, BUILDING_ID) {
        Ok(s) => s.to_string(),
        Err(_) => {
            warn!("build_structure command missing building_id param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("build_structure: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let Some(ref building_registry) = br.building_registry else {
        warn!("build_structure: BuildingRegistry not available");
        return;
    };

    let Some(building_def) = building_registry.get(&building_id_str) else {
        warn!("build_structure: unknown building '{}'", building_id_str);
        return;
    };

    let minerals_cost = building_def.minerals_cost;
    let energy_cost = building_def.energy_cost;
    let build_time = building_def.build_time;
    let is_system_building = building_def.is_system_building;
    let target_system = target_system(params);

    let owned_systems: std::collections::HashSet<Entity> = sovereignty
        .iter()
        .filter(|(_, sov)| sov.owner == Some(Owner::Empire(empire_entity)))
        .map(|(e, _)| e)
        .collect();

    if is_system_building {
        let bid = crate::scripting::building_api::BuildingId::new(&building_id_str);
        let core_systems: std::collections::HashSet<Entity> =
            br.core_at_system.iter().map(|at| at.0).collect();
        if let Some(ref target) = target_system {
            if !owned_systems.contains(target) || !core_systems.contains(target) {
                debug!(
                    "build_structure (system): target {:?} not owned or lacks Core",
                    target
                );
                return;
            }
        }
        let mut queued = false;
        for (sys_entity, sys_bldgs, mut sbq) in br.system_builds.iter_mut() {
            if !owned_systems.contains(&sys_entity) {
                continue;
            }
            if !core_systems.contains(&sys_entity) {
                continue;
            }
            if let Some(ref target) = target_system {
                if sys_entity != *target {
                    continue;
                }
            }
            if sbq
                .queue
                .iter()
                .any(|o| o.building_id.as_str() == building_id_str)
            {
                debug!(
                    "build_structure (system): '{}' already queued at {:?}, skipping",
                    building_id_str, sys_entity
                );
                continue;
            }
            let sys_slots = crate::colony::system_buildings::slots_view(
                sys_entity,
                sys_bldgs.max_slots,
                &br.station_ships,
                building_registry,
            );
            let pending_slots: std::collections::HashSet<usize> =
                sbq.queue.iter().map(|o| o.target_slot).collect();
            let empty_slot = sys_slots
                .iter()
                .enumerate()
                .position(|(i, s)| s.is_none() && !pending_slots.contains(&i));
            let Some(slot_idx) = empty_slot else {
                continue;
            };
            sbq.push_build_order(crate::colony::building_queue::BuildingOrder {
                order_id: 0,
                building_id: bid.clone(),
                target_slot: slot_idx,
                minerals_remaining: minerals_cost,
                energy_remaining: energy_cost,
                build_time_remaining: build_time,
            });
            info!(
                "build_structure (system): queued '{}' at system {:?} (slot {}) for empire {:?}",
                building_id_str, sys_entity, slot_idx, empire_entity
            );
            queued = true;
            break;
        }
        if !queued {
            debug!(
                "build_structure (system): no Core-equipped system with a free slot for '{}' (empire {:?})",
                building_id_str, empire_entity
            );
        }
        return;
    }

    let bid = crate::scripting::building_api::BuildingId::new(&building_id_str);
    let mut built = false;
    for (colony_entity, colony, buildings, mut building_queue) in br.colonies.iter_mut() {
        let colony_system = colony.system(&br.planets);
        let Some(sys) = colony_system else { continue };
        if !owned_systems.contains(&sys) {
            continue;
        }
        if let Some(ref target) = target_system {
            if sys != *target {
                continue;
            }
        }

        if building_queue
            .queue
            .iter()
            .any(|o| o.building_id.as_str() == building_id_str)
        {
            debug!(
                "build_structure (planet): '{}' already queued at colony {:?} (system {:?}), skipping duplicate emission",
                building_id_str, colony_entity, sys
            );
            continue;
        }

        let pending_slots: std::collections::HashSet<usize> =
            building_queue.queue.iter().map(|o| o.target_slot).collect();
        let empty_slot = buildings
            .slots
            .iter()
            .enumerate()
            .position(|(i, s)| s.is_none() && !pending_slots.contains(&i));
        let Some(slot_idx) = empty_slot else { continue };

        building_queue.push_build_order(BuildingOrder {
            order_id: 0,
            building_id: bid.clone(),
            target_slot: slot_idx,
            minerals_remaining: minerals_cost,
            energy_remaining: energy_cost,
            build_time_remaining: build_time,
        });
        info!(
            "build_structure: queued '{}' at colony {:?} (slot {}) for empire {:?}",
            building_id_str, colony_entity, slot_idx, empire_entity
        );
        built = true;
        break;
    }

    if !built {
        debug!(
            "build_structure: no colony with empty slot found for building '{}' (empire {:?})",
            building_id_str, empire_entity
        );
    }
}

/// Handle `build_deliverable`: queue a deliverable for construction at a
/// colony owned by the issuing faction.
pub(crate) fn handle_build_deliverable(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let definition_id = match required_str(params, DEFINITION_ID) {
        Ok(s) => s.to_string(),
        Err(_) => {
            warn!("build_deliverable command missing definition_id param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!(
                "build_deliverable: no empire found for faction {:?}",
                issuer
            );
            return;
        }
    };

    let target_system = target_system(params);

    let owned_systems: Vec<Entity> = sovereignty
        .iter()
        .filter(|(_, sov)| sov.owner == Some(Owner::Empire(empire_entity)))
        .map(|(e, _)| e)
        .collect();

    let chosen_system = match target_system {
        Some(t) if owned_systems.contains(&t) => Some(t),
        Some(_) | None => owned_systems.first().copied(),
    };

    let Some(sys_entity) = chosen_system else {
        debug!(
            "build_deliverable: faction {:?} has no owned system for '{}'",
            issuer, definition_id
        );
        return;
    };

    let Some(ref registry) = br.deliverable_registry else {
        warn!("build_deliverable: DeliverableRegistry not available");
        return;
    };
    let Some(def) = registry.get(&definition_id) else {
        warn!(
            "build_deliverable: unknown deliverable definition '{}'",
            definition_id
        );
        return;
    };
    let Some(meta) = def.deliverable.as_ref() else {
        warn!(
            "build_deliverable: definition '{}' is not shipyard-buildable \
             (no DeliverableMetadata — declared via `define_structure`?)",
            definition_id
        );
        return;
    };

    let order = BuildOrder {
        order_id: 0,
        kind: BuildKind::Deliverable {
            cargo_size: meta.cargo_size,
        },
        design_id: definition_id.clone(),
        display_name: def.name.clone(),
        minerals_cost: meta.cost.minerals,
        minerals_invested: Amt::ZERO,
        energy_cost: meta.cost.energy,
        energy_invested: Amt::ZERO,
        build_time_total: meta.build_time,
        build_time_remaining: meta.build_time,
    };

    let host_colony =
        pick_host_colony(sys_entity, empire_entity, &mut br.build_queues, &br.planets);

    if let Some((colony_entity, mut bq)) = host_colony {
        if bq.queue.iter().any(|o| {
            matches!(o.kind, BuildKind::Deliverable { .. }) && o.design_id == definition_id
        }) {
            debug!(
                "build_deliverable: '{}' already queued at colony {:?} (system {:?}), skipping",
                definition_id, colony_entity, sys_entity
            );
            return;
        }
        bq.push_order(order);
        info!(
            "build_deliverable: queued '{}' at colony {:?} (system {:?}) for empire {:?}",
            definition_id, colony_entity, sys_entity, empire_entity
        );
    } else {
        warn!(
            "build_deliverable: chosen system {:?} has no owned colony to host '{}' (empire {:?})",
            sys_entity, definition_id, empire_entity
        );
    }
}

fn pick_host_colony<'a>(
    sys: Entity,
    empire: Entity,
    build_queues: &'a mut Query<(
        Entity,
        &'static Colony,
        &'static crate::faction::FactionOwner,
        &'static mut BuildQueue,
    )>,
    planets: &Query<&Planet>,
) -> Option<(Entity, Mut<'a, BuildQueue>)> {
    for (colony_entity, colony, faction_owner, build_queue) in build_queues.iter_mut() {
        if colony.system(planets) != Some(sys) {
            continue;
        }
        if faction_owner.0 != empire {
            continue;
        }
        return Some((colony_entity, build_queue));
    }
    None
}

fn queue_ship_at_shipyard(
    empire_entity: Entity,
    design_id: &str,
    target_system: Option<Entity>,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    br: &mut BuildResearchParams,
) -> bool {
    let Some(ref design_registry) = br.design_registry else {
        warn!("build_ship/fortify: ShipDesignRegistry not available");
        return false;
    };
    let Some(design) = design_registry.get(design_id) else {
        warn!("build_ship/fortify: unknown design '{}'", design_id);
        return false;
    };
    if !design.is_direct_buildable {
        warn!(
            "build_ship/fortify: design '{}' is not direct-buildable (installation hull)",
            design_id
        );
        return false;
    }

    let owned_systems: Vec<Entity> = sovereignty
        .iter()
        .filter(|(_, sov)| sov.owner == Some(Owner::Empire(empire_entity)))
        .map(|(e, _)| e)
        .collect();

    let shipyard_system = if let Some(target) = target_system {
        if owned_systems.contains(&target) && has_shipyard_check(target, &br.sys_mods_q) {
            Some(target)
        } else {
            owned_systems
                .iter()
                .find(|&&sys| has_shipyard_check(sys, &br.sys_mods_q))
                .copied()
        }
    } else {
        owned_systems
            .iter()
            .find(|&&sys| has_shipyard_check(sys, &br.sys_mods_q))
            .copied()
    };

    let Some(sys_entity) = shipyard_system else {
        debug!(
            "build_ship/fortify: no system with shipyard found for empire {:?}",
            empire_entity
        );
        return false;
    };

    let build_time = design_registry.build_time(design_id);
    let order = BuildOrder {
        order_id: 0,
        kind: BuildKind::default(),
        design_id: design_id.to_string(),
        display_name: design.name.clone(),
        minerals_cost: design.build_cost_minerals,
        minerals_invested: Amt::ZERO,
        energy_cost: design.build_cost_energy,
        energy_invested: Amt::ZERO,
        build_time_total: build_time,
        build_time_remaining: build_time,
    };

    let host_colony =
        pick_host_colony(sys_entity, empire_entity, &mut br.build_queues, &br.planets);

    if let Some((colony_entity, mut build_queue)) = host_colony {
        if build_queue
            .queue
            .iter()
            .any(|o| matches!(o.kind, BuildKind::Ship) && o.design_id == design_id)
        {
            debug!(
                "build_ship/fortify: '{}' already queued at colony {:?} (system {:?}), skipping duplicate emission",
                design_id, colony_entity, sys_entity
            );
            return false;
        }
        build_queue.push_order(order);
        info!(
            "build_ship/fortify: queued '{}' at colony {:?} (system {:?}) for empire {:?}",
            design_id, colony_entity, sys_entity, empire_entity
        );
        true
    } else {
        warn!(
            "build_ship/fortify: shipyard system {:?} has no owned colony to host build order (empire {:?})",
            sys_entity, empire_entity
        );
        false
    }
}

fn has_shipyard_check(system: Entity, sys_mods_q: &Query<&crate::galaxy::SystemModifiers>) -> bool {
    sys_mods_q
        .get(system)
        .map(|m| m.shipyard_build_parallel_slots.value().final_value() > Amt::ZERO)
        .unwrap_or(false)
}
