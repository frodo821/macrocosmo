//! #273: Arrival-side handling of `RemoteCommand::Colony` payloads.
//!
//! `communication::process_pending_commands` is the scheduler — it decides
//! which `PendingCommand` entities are ready to apply and marks the
//! matching `CommandLog` entry arrived. The actual queue mutation (cost /
//! time resolution against `BuildingRegistry` / `ShipDesignRegistry`,
//! modifier application, and pushing onto the target's build / demolition
//! / upgrade queues) lives here, in the module where the queue types
//! themselves live.

use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::{
    BuildKind, BuildOrder, BuildQueue, BuildingOrder, BuildingQueue, Buildings, Colony,
    DemolitionOrder, SystemBuildingQueue, SystemBuildings, UpgradeOrder,
};
use crate::communication::{ColonyCommand, ColonyCommandKind};
use crate::scripting::building_api::{BuildingDefinition, BuildingId, BuildingRegistry};
use crate::ship_design::ShipDesignRegistry;

pub type ApplyColoniesQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Colony,
        &'static Buildings,
        &'static mut BuildingQueue,
        &'static mut BuildQueue,
    ),
>;

pub type ApplySystemBuildingsQuery<'w, 's> =
    Query<'w, 's, (&'static SystemBuildings, &'static mut SystemBuildingQueue)>;

/// Apply a `ColonyCommand` that has just arrived at `target_system`. Cost,
/// time, and refund amounts are resolved here against the *current*
/// registry + modifier state at the target (see #270 design note: "the
/// order takes effect under the conditions at the target when it
/// arrives"). The payload only carries ids and slot indices.
///
/// Silently warns and drops on unknown ids / missing slots / missing
/// target components — arrival should never panic.
pub fn apply_colony_command(
    cc: &ColonyCommand,
    target_system: Entity,
    br: &BuildingRegistry,
    sdr: &ShipDesignRegistry,
    bldg_cost_mod: Amt,
    bldg_time_mod: Amt,
    colonies: &mut ApplyColoniesQuery,
    sys_buildings_q: &mut ApplySystemBuildingsQuery,
) {
    match &cc.kind {
        ColonyCommandKind::QueueBuilding {
            building_id,
            target_slot,
        } => {
            let Some(def) = br.get(building_id) else {
                warn!("QueueBuilding: unknown building_id '{}'", building_id);
                return;
            };
            let (base_m, base_e) = def.build_cost();
            let eff_m = base_m.mul_amt(bldg_cost_mod);
            let eff_e = base_e.mul_amt(bldg_cost_mod);
            let eff_time = (def.build_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
            let order = BuildingOrder {
                building_id: BuildingId::new(building_id),
                target_slot: *target_slot,
                minerals_remaining: eff_m,
                energy_remaining: eff_e,
                build_time_remaining: eff_time,
            };
            match cc.target_planet {
                Some(planet) => push_planet_building_order(planet, order, colonies),
                None => {
                    if let Ok((_, mut sbq)) = sys_buildings_q.get_mut(target_system) {
                        sbq.queue.push(order);
                    } else {
                        warn!(
                            "QueueBuilding (system): target_system {:?} has no SystemBuildingQueue",
                            target_system
                        );
                    }
                }
            }
        }
        ColonyCommandKind::DemolishBuilding { target_slot } => match cc.target_planet {
            Some(planet) => {
                let mut found = false;
                for (colony, buildings, mut bq, _) in colonies.iter_mut() {
                    if colony.planet != planet {
                        continue;
                    }
                    found = true;
                    let Some(Some(bid)) = buildings.slots.get(*target_slot).cloned() else {
                        warn!(
                            "DemolishBuilding (planet): slot {} is empty or out of bounds",
                            target_slot
                        );
                        break;
                    };
                    let Some(def) = br.get(bid.as_str()) else {
                        warn!(
                            "DemolishBuilding (planet): unknown building '{}' in slot {}; dropping order to avoid silent free demolition",
                            bid, target_slot
                        );
                        break;
                    };
                    let (m_ref, e_ref) = def.demolition_refund();
                    bq.demolition_queue.push(DemolitionOrder {
                        target_slot: *target_slot,
                        building_id: bid,
                        time_remaining: def.demolition_time(),
                        minerals_refund: m_ref,
                        energy_refund: e_ref,
                    });
                    break;
                }
                if !found {
                    warn!(
                        "DemolishBuilding (planet): no colony found on planet {:?}",
                        planet
                    );
                }
            }
            None => {
                let Ok((sys_buildings, mut sbq)) = sys_buildings_q.get_mut(target_system) else {
                    warn!(
                        "DemolishBuilding (system): target_system {:?} has no SystemBuildings/Queue",
                        target_system
                    );
                    return;
                };
                let Some(Some(bid)) = sys_buildings.slots.get(*target_slot).cloned() else {
                    warn!(
                        "DemolishBuilding (system): slot {} is empty or out of bounds",
                        target_slot
                    );
                    return;
                };
                let Some(def) = br.get(bid.as_str()) else {
                    warn!(
                        "DemolishBuilding (system): unknown building '{}' in slot {}; dropping order",
                        bid, target_slot
                    );
                    return;
                };
                let (m_ref, e_ref) = def.demolition_refund();
                sbq.demolition_queue.push(DemolitionOrder {
                    target_slot: *target_slot,
                    building_id: bid,
                    time_remaining: def.demolition_time(),
                    minerals_refund: m_ref,
                    energy_refund: e_ref,
                });
            }
        },
        ColonyCommandKind::UpgradeBuilding {
            slot_index,
            target_id,
        } => {
            let upgrade_order = |source_def: &BuildingDefinition,
                                 target_id: &str|
             -> Option<UpgradeOrder> {
                let up = source_def
                    .upgrade_to
                    .iter()
                    .find(|u| u.target_id == target_id)?;
                let eff_m = up.cost_minerals.mul_amt(bldg_cost_mod);
                let eff_e = up.cost_energy.mul_amt(bldg_cost_mod);
                let base_time = up
                    .build_time
                    .unwrap_or_else(|| br.get(target_id).map(|d| d.build_time / 2).unwrap_or(5));
                let eff_time = (base_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                Some(UpgradeOrder {
                    slot_index: *slot_index,
                    target_id: BuildingId::new(target_id),
                    minerals_remaining: eff_m,
                    energy_remaining: eff_e,
                    build_time_remaining: eff_time,
                })
            };
            match cc.target_planet {
                Some(planet) => {
                    let mut handled = false;
                    for (colony, buildings, mut bq, _) in colonies.iter_mut() {
                        if colony.planet != planet {
                            continue;
                        }
                        handled = true;
                        let Some(Some(source_bid)) = buildings.slots.get(*slot_index).cloned()
                        else {
                            warn!("UpgradeBuilding (planet): slot {} empty or OOB", slot_index);
                            break;
                        };
                        let Some(source_def) = br.get(source_bid.as_str()) else {
                            warn!(
                                "UpgradeBuilding (planet): unknown source building '{}'",
                                source_bid
                            );
                            break;
                        };
                        if let Some(order) = upgrade_order(source_def, target_id) {
                            bq.upgrade_queue.push(order);
                        } else {
                            warn!(
                                "UpgradeBuilding (planet): no upgrade path '{}' -> '{}'",
                                source_bid, target_id
                            );
                        }
                        break;
                    }
                    if !handled {
                        warn!("UpgradeBuilding (planet): no colony on planet {:?}", planet);
                    }
                }
                None => {
                    let Ok((sys_buildings, mut sbq)) = sys_buildings_q.get_mut(target_system)
                    else {
                        warn!(
                            "UpgradeBuilding (system): target_system {:?} missing components",
                            target_system
                        );
                        return;
                    };
                    let Some(Some(source_bid)) = sys_buildings.slots.get(*slot_index).cloned()
                    else {
                        warn!("UpgradeBuilding (system): slot {} empty or OOB", slot_index);
                        return;
                    };
                    let Some(source_def) = br.get(source_bid.as_str()) else {
                        warn!(
                            "UpgradeBuilding (system): unknown source building '{}'",
                            source_bid
                        );
                        return;
                    };
                    if let Some(order) = upgrade_order(source_def, target_id) {
                        sbq.upgrade_queue.push(order);
                    } else {
                        warn!(
                            "UpgradeBuilding (system): no upgrade path '{}' -> '{}'",
                            source_bid, target_id
                        );
                    }
                }
            }
        }
        ColonyCommandKind::QueueShipBuild {
            host_colony,
            design_id,
            build_kind,
        } => {
            let Ok((_, _, _, mut build_q)) = colonies.get_mut(*host_colony) else {
                warn!(
                    "QueueShipBuild: host_colony {:?} has no BuildQueue",
                    host_colony
                );
                return;
            };
            let Some(design) = sdr.get(design_id) else {
                warn!("QueueShipBuild: unknown design_id '{}'", design_id);
                return;
            };
            let minerals_cost = design.build_cost_minerals;
            let energy_cost = design.build_cost_energy;
            let build_time_total = sdr.build_time(design_id);
            let display_name = design.name.clone();
            build_q.queue.push(BuildOrder {
                kind: build_kind.clone(),
                design_id: design_id.clone(),
                display_name,
                minerals_cost,
                minerals_invested: Amt::ZERO,
                energy_cost,
                energy_invested: Amt::ZERO,
                build_time_total,
                build_time_remaining: build_time_total,
            });
        }
        ColonyCommandKind::QueueDeliverableBuild {
            host_colony,
            def_id,
            display_name,
            cargo_size,
            minerals_cost,
            energy_cost,
            build_time,
        } => {
            let Ok((_, _, _, mut build_q)) = colonies.get_mut(*host_colony) else {
                warn!(
                    "QueueDeliverableBuild: host_colony {:?} has no BuildQueue",
                    host_colony
                );
                return;
            };
            build_q.queue.push(BuildOrder {
                kind: BuildKind::Deliverable {
                    cargo_size: *cargo_size,
                },
                design_id: def_id.clone(),
                display_name: display_name.clone(),
                minerals_cost: *minerals_cost,
                minerals_invested: Amt::ZERO,
                energy_cost: *energy_cost,
                energy_invested: Amt::ZERO,
                build_time_total: *build_time,
                build_time_remaining: *build_time,
            });
        }
    }
}

fn push_planet_building_order(
    planet: Entity,
    order: BuildingOrder,
    colonies: &mut ApplyColoniesQuery,
) {
    for (colony, _, mut bq, _) in colonies.iter_mut() {
        if colony.planet == planet {
            bq.queue.push(order);
            return;
        }
    }
    warn!(
        "QueueBuilding (planet): no colony found on planet {:?}",
        planet
    );
}
