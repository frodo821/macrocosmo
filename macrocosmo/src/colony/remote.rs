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
use crate::communication::{BuildingKind, BuildingScope, ColonyCommand, RemoteCommand};
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

/// #275: Read-only `Planet` lookup used by `CancelBuildingOrder` to scope
/// the order-id search to colonies in the target system.
pub type ApplyPlanetsQuery<'w, 's> = Query<'w, 's, &'static crate::galaxy::Planet>;

/// Apply a `RemoteCommand` that has just arrived. Cost, time, and refund
/// amounts are resolved here against the *current* registry + modifier
/// state at the target for building ops and ship builds;
/// `DeliverableBuild` carries a pre-computed payload (defs live in
/// `StructureRegistry`, which isn't plumbed into this handler).
///
/// Silently warns and drops on unknown ids / missing slots / missing
/// target components — arrival should never panic.
#[allow(clippy::too_many_arguments)]
pub fn apply_remote_command(
    cmd: &RemoteCommand,
    target_system: Entity,
    br: &BuildingRegistry,
    sdr: &ShipDesignRegistry,
    bldg_cost_mod: Amt,
    bldg_time_mod: Amt,
    colonies: &mut ApplyColoniesQuery,
    sys_buildings_q: &mut ApplySystemBuildingsQuery,
    planets: &ApplyPlanetsQuery,
) {
    match cmd {
        RemoteCommand::BuildShip { .. } | RemoteCommand::SetProductionFocus { .. } => {
            // Pre-#270 orphan API — not yet wired to any UI; intentional no-op.
        }
        RemoteCommand::Colony(cc) => apply_building_command(
            cc,
            target_system,
            br,
            bldg_cost_mod,
            bldg_time_mod,
            colonies,
            sys_buildings_q,
        ),
        RemoteCommand::ShipBuild {
            host_colony,
            design_id,
            build_kind,
        } => {
            let Ok((_, _, _, mut build_q)) = colonies.get_mut(*host_colony) else {
                warn!("ShipBuild: host_colony {:?} has no BuildQueue", host_colony);
                return;
            };
            let Some(design) = sdr.get(design_id) else {
                warn!("ShipBuild: unknown design_id '{}'", design_id);
                return;
            };
            let build_time_total = sdr.build_time(design_id);
            build_q.push_order(BuildOrder {
                order_id: 0,
                kind: build_kind.clone(),
                design_id: design_id.clone(),
                display_name: design.name.clone(),
                minerals_cost: design.build_cost_minerals,
                minerals_invested: Amt::ZERO,
                energy_cost: design.build_cost_energy,
                energy_invested: Amt::ZERO,
                build_time_total,
                build_time_remaining: build_time_total,
            });
        }
        RemoteCommand::DeliverableBuild {
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
                    "DeliverableBuild: host_colony {:?} has no BuildQueue",
                    host_colony
                );
                return;
            };
            build_q.push_order(BuildOrder {
                order_id: 0,
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
        RemoteCommand::CancelBuildingOrder { order_id } => apply_cancel_building_order(
            *order_id,
            target_system,
            colonies,
            sys_buildings_q,
            planets,
        ),
        RemoteCommand::CancelShipOrder {
            host_colony,
            order_id,
        } => {
            let Ok((_, _, _, mut build_q)) = colonies.get_mut(*host_colony) else {
                warn!(
                    "CancelShipOrder: host_colony {:?} has no BuildQueue (race with colony despawn?)",
                    host_colony
                );
                return;
            };
            match build_q.remove_order(*order_id) {
                Some(o) => {
                    info!(
                        "Cancelled ship order id={} design={} on host_colony {:?}",
                        order_id, o.design_id, host_colony
                    );
                }
                None => {
                    warn!(
                        "CancelShipOrder: order_id={} not found on host_colony {:?} \
                         (likely raced with completion or another cancel)",
                        order_id, host_colony
                    );
                }
            }
        }
    }
}

/// #275: Cancel a building / demolition / upgrade order by `order_id`.
/// The dispatch does not carry scope. The handler first scopes to the
/// target system (via the Planet lookup): iterate colonies whose planet
/// lives in `target_system` and ask each BuildingQueue to cancel the id.
/// If none match, fall back to the system-level SystemBuildingQueue.
/// First hit wins; the matching order is removed.
///
/// Why scope to `target_system`: `order_id` counters are per-queue, so
/// the same numeric id can occur independently on different colonies.
/// Without scoping, a cancel dispatched for system A could accidentally
/// cancel an unrelated order on system B. Restricting the search to
/// `target_system` reuses the information the player (and the command)
/// already carried.
///
/// Race semantics: if no queue in `target_system` holds the id (order
/// already completed, or was cancelled by a racing dispatch), we warn
/// and drop. Never panics. No resource refund on construction cancel
/// (invested resources are forfeit); demolition cancel just revokes
/// the pending refund.
fn apply_cancel_building_order(
    order_id: u64,
    target_system: Entity,
    colonies: &mut ApplyColoniesQuery,
    sys_buildings_q: &mut ApplySystemBuildingsQuery,
    planets: &ApplyPlanetsQuery,
) {
    for (colony, _b, mut bq, _) in colonies.iter_mut() {
        let Ok(planet) = planets.get(colony.planet) else {
            continue;
        };
        if planet.system != target_system {
            continue;
        }
        if let Some(kind) = bq.cancel_order(order_id) {
            info!(
                "Cancelled planet building order id={} ({:?}) on system {:?}",
                order_id, kind, target_system
            );
            return;
        }
    }
    if let Ok((_, mut sbq)) = sys_buildings_q.get_mut(target_system) {
        if let Some(kind) = sbq.cancel_order(order_id) {
            info!(
                "Cancelled system building order id={} ({:?}) on system {:?}",
                order_id, kind, target_system
            );
            return;
        }
    }
    warn!(
        "CancelBuildingOrder: order_id={} not found on system {:?} \
         (likely raced with completion or another cancel)",
        order_id, target_system
    );
}

fn apply_building_command(
    cc: &ColonyCommand,
    target_system: Entity,
    br: &BuildingRegistry,
    bldg_cost_mod: Amt,
    bldg_time_mod: Amt,
    colonies: &mut ApplyColoniesQuery,
    sys_buildings_q: &mut ApplySystemBuildingsQuery,
) {
    match &cc.kind {
        BuildingKind::Queue {
            building_id,
            target_slot,
        } => {
            let Some(def) = br.get(building_id) else {
                warn!("Queue: unknown building_id '{}'", building_id);
                return;
            };
            let (base_m, base_e) = def.build_cost();
            let eff_m = base_m.mul_amt(bldg_cost_mod);
            let eff_e = base_e.mul_amt(bldg_cost_mod);
            let eff_time = (def.build_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
            let order = BuildingOrder {
                order_id: 0,
                building_id: BuildingId::new(building_id),
                target_slot: *target_slot,
                minerals_remaining: eff_m,
                energy_remaining: eff_e,
                build_time_remaining: eff_time,
            };
            match cc.scope {
                BuildingScope::Planet(planet) => {
                    push_planet_building_order(planet, order, colonies)
                }
                BuildingScope::System => {
                    if let Ok((_, mut sbq)) = sys_buildings_q.get_mut(target_system) {
                        sbq.push_build_order(order);
                    } else {
                        warn!(
                            "Queue (system): target_system {:?} has no SystemBuildingQueue",
                            target_system
                        );
                    }
                }
            }
        }
        BuildingKind::Demolish { target_slot } => match cc.scope {
            BuildingScope::Planet(planet) => {
                let mut found = false;
                for (colony, buildings, mut bq, _) in colonies.iter_mut() {
                    if colony.planet != planet {
                        continue;
                    }
                    found = true;
                    let Some(Some(bid)) = buildings.slots.get(*target_slot).cloned() else {
                        warn!(
                            "Demolish (planet): slot {} is empty or out of bounds",
                            target_slot
                        );
                        break;
                    };
                    let Some(def) = br.get(bid.as_str()) else {
                        warn!(
                            "Demolish (planet): unknown building '{}' in slot {}; dropping order to avoid silent free demolition",
                            bid, target_slot
                        );
                        break;
                    };
                    let (m_ref, e_ref) = def.demolition_refund();
                    bq.push_demolition_order(DemolitionOrder {
                        order_id: 0,
                        target_slot: *target_slot,
                        building_id: bid,
                        time_remaining: def.demolition_time(),
                        minerals_refund: m_ref,
                        energy_refund: e_ref,
                    });
                    break;
                }
                if !found {
                    warn!("Demolish (planet): no colony found on planet {:?}", planet);
                }
            }
            BuildingScope::System => {
                let Ok((sys_buildings, mut sbq)) = sys_buildings_q.get_mut(target_system) else {
                    warn!(
                        "Demolish (system): target_system {:?} has no SystemBuildings/Queue",
                        target_system
                    );
                    return;
                };
                let Some(Some(bid)) = sys_buildings.slots.get(*target_slot).cloned() else {
                    warn!(
                        "Demolish (system): slot {} is empty or out of bounds",
                        target_slot
                    );
                    return;
                };
                let Some(def) = br.get(bid.as_str()) else {
                    warn!(
                        "Demolish (system): unknown building '{}' in slot {}; dropping order",
                        bid, target_slot
                    );
                    return;
                };
                let (m_ref, e_ref) = def.demolition_refund();
                sbq.push_demolition_order(DemolitionOrder {
                    order_id: 0,
                    target_slot: *target_slot,
                    building_id: bid,
                    time_remaining: def.demolition_time(),
                    minerals_refund: m_ref,
                    energy_refund: e_ref,
                });
            }
        },
        BuildingKind::Upgrade {
            slot_index,
            target_id,
        } => {
            let upgrade_order =
                |source_def: &BuildingDefinition, target_id: &str| -> Option<UpgradeOrder> {
                    let up = source_def
                        .upgrade_to
                        .iter()
                        .find(|u| u.target_id == target_id)?;
                    let eff_m = up.cost_minerals.mul_amt(bldg_cost_mod);
                    let eff_e = up.cost_energy.mul_amt(bldg_cost_mod);
                    let base_time = up.build_time.unwrap_or_else(|| {
                        br.get(target_id).map(|d| d.build_time / 2).unwrap_or(5)
                    });
                    let eff_time = (base_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                    Some(UpgradeOrder {
                        order_id: 0,
                        slot_index: *slot_index,
                        target_id: BuildingId::new(target_id),
                        minerals_remaining: eff_m,
                        energy_remaining: eff_e,
                        build_time_remaining: eff_time,
                    })
                };
            match cc.scope {
                BuildingScope::Planet(planet) => {
                    let mut handled = false;
                    for (colony, buildings, mut bq, _) in colonies.iter_mut() {
                        if colony.planet != planet {
                            continue;
                        }
                        handled = true;
                        let Some(Some(source_bid)) = buildings.slots.get(*slot_index).cloned()
                        else {
                            warn!("Upgrade (planet): slot {} empty or OOB", slot_index);
                            break;
                        };
                        let Some(source_def) = br.get(source_bid.as_str()) else {
                            warn!("Upgrade (planet): unknown source building '{}'", source_bid);
                            break;
                        };
                        if let Some(order) = upgrade_order(source_def, target_id) {
                            bq.push_upgrade_order(order);
                        } else {
                            warn!(
                                "Upgrade (planet): no upgrade path '{}' -> '{}'",
                                source_bid, target_id
                            );
                        }
                        break;
                    }
                    if !handled {
                        warn!("Upgrade (planet): no colony on planet {:?}", planet);
                    }
                }
                BuildingScope::System => {
                    let Ok((sys_buildings, mut sbq)) = sys_buildings_q.get_mut(target_system)
                    else {
                        warn!(
                            "Upgrade (system): target_system {:?} missing components",
                            target_system
                        );
                        return;
                    };
                    let Some(Some(source_bid)) = sys_buildings.slots.get(*slot_index).cloned()
                    else {
                        warn!("Upgrade (system): slot {} empty or OOB", slot_index);
                        return;
                    };
                    let Some(source_def) = br.get(source_bid.as_str()) else {
                        warn!("Upgrade (system): unknown source building '{}'", source_bid);
                        return;
                    };
                    if let Some(order) = upgrade_order(source_def, target_id) {
                        sbq.push_upgrade_order(order);
                    } else {
                        warn!(
                            "Upgrade (system): no upgrade path '{}' -> '{}'",
                            source_bid, target_id
                        );
                    }
                }
            }
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
            bq.push_build_order(order);
            return;
        }
    }
    warn!(
        "QueueBuilding (planet): no colony found on planet {:?}",
        planet
    );
}
