use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildQueue, BuildingOrder, BuildingQueue, Buildings, Colony, ConstructionParams, DemolitionOrder, FoodConsumption, MaintenanceCost, Production, ResourceStockpile, UpgradeOrder};
use crate::scripting::building_api::{BuildingId, BuildingRegistry};
use crate::galaxy::SystemAttributes;
use crate::amount::Amt;
use crate::ship::{Cargo, Ship, ShipHitpoints, ShipState, SurveyData};

/// Draws colony detail for a specific planet. Called within a ScrollArea.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_colony_detail(
    ui: &mut egui::Ui,
    planet_entity: Entity,
    system_entity: Entity,
    planet_attrs: Option<&SystemAttributes>,
    colonies: &mut Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    system_stockpiles: &mut Query<(&mut ResourceStockpile, Option<&crate::colony::ResourceCapacity>), With<crate::galaxy::StarSystem>>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    construction_params: &ConstructionParams,
    planets: &Query<&crate::galaxy::Planet>,
    _hull_registry: &crate::ship_design::HullRegistry,
    _module_registry: &crate::ship_design::ModuleRegistry,
    design_registry: &crate::ship_design::ShipDesignRegistry,
    building_registry: &BuildingRegistry,
) {
    ui.label(
        egui::RichText::new("Colony")
            .strong()
            .color(egui::Color32::from_rgb(100, 200, 100)),
    );

    for (_colony_entity, colony, production, _build_queue, buildings, mut building_queue, maintenance_cost, food_consumption) in
        colonies.iter_mut()
    {
        if colony.planet != planet_entity {
            continue;
        }

        // #69: Show population with carrying capacity
        let carrying_cap = {
            use crate::amount::Amt;
            use crate::galaxy::{BASE_CARRYING_CAPACITY, FOOD_PER_POP_PER_HEXADIES};
            let hab_score = planet_attrs.map(|a| a.habitability).unwrap_or(0.5);
            let k_habitat = BASE_CARRYING_CAPACITY * hab_score;
            let food_prod = production.map(|p| p.food_per_hexadies.final_value()).unwrap_or(Amt::ZERO);
            let k_food = if FOOD_PER_POP_PER_HEXADIES.raw() > 0 {
                food_prod.div_amt(FOOD_PER_POP_PER_HEXADIES).to_f64()
            } else {
                k_habitat
            };
            k_habitat.min(k_food).max(1.0)
        };
        ui.label(format!("Population: {:.0} / {:.0}", colony.population, carrying_cap));

        if let Some(prod) = production {
            use crate::amount::SignedAmt;
            let green = egui::Color32::from_rgb(100, 200, 100);
            let red = egui::Color32::from_rgb(255, 100, 100);

            ui.label(egui::RichText::new("Income/hd:").strong());

            // Food: production - consumption
            let food_prod = prod.food_per_hexadies.final_value();
            let food_cons = food_consumption.map(|fc| fc.food_per_hexadies.final_value()).unwrap_or(crate::amount::Amt::ZERO);
            let food_net = SignedAmt::from_amt(food_prod).add(SignedAmt(0 - SignedAmt::from_amt(food_cons).raw()));
            let food_color = if food_net.raw() > 0 { green } else if food_net.raw() < 0 { red } else { egui::Color32::GRAY };
            ui.horizontal(|ui| {
                ui.label("  Food:    ");
                ui.label(egui::RichText::new(food_net.display()).color(food_color));
                if food_cons > crate::amount::Amt::ZERO {
                    ui.label(format!("(produce {}, consume {})", food_prod, food_cons));
                }
            });

            // Energy: production - maintenance
            let energy_prod = prod.energy_per_hexadies.final_value();
            let maint = maintenance_cost.map(|mc| mc.energy_per_hexadies.final_value()).unwrap_or(crate::amount::Amt::ZERO);
            let energy_net = SignedAmt::from_amt(energy_prod).add(SignedAmt(0 - SignedAmt::from_amt(maint).raw()));
            let energy_color = if energy_net.raw() > 0 { green } else if energy_net.raw() < 0 { red } else { egui::Color32::GRAY };
            ui.horizontal(|ui| {
                ui.label("  Energy:  ");
                ui.label(egui::RichText::new(energy_net.display()).color(energy_color));
                if maint > crate::amount::Amt::ZERO {
                    ui.label(format!("(produce {}, maintain {})", energy_prod, maint));
                }
            });

            // Minerals: just production
            let minerals_prod = prod.minerals_per_hexadies.final_value();
            let minerals_net = SignedAmt::from_amt(minerals_prod);
            let minerals_color = if minerals_net.raw() > 0 { green } else { egui::Color32::GRAY };
            ui.horizontal(|ui| {
                ui.label("  Minerals:");
                ui.label(egui::RichText::new(minerals_net.display()).color(minerals_color));
            });

            // Research: just production (flow, no consumption)
            let research_prod = prod.research_per_hexadies.final_value();
            ui.horizontal(|ui| {
                ui.label("  Research:");
                ui.label(format!("{}", research_prod));
            });
        }

        if let Ok((stockpile, _)) = system_stockpiles.get(system_entity) {
            ui.label(format!(
                "Stockpile: F {} | E {} | M {} | A {}",
                stockpile.food, stockpile.energy, stockpile.minerals, stockpile.authority,
            ));
        }

        // #51/#64: Maintenance cost summary
        {
            use crate::amount::Amt;
            let mut building_maintenance = Amt::ZERO;
            if let Some(b) = buildings {
                for slot in &b.slots {
                    if let Some(bid) = slot {
                        building_maintenance = building_maintenance.add(
                            building_registry.get(bid.as_str()).map(|d| d.maintenance).unwrap_or(Amt::ZERO)
                        );
                    }
                }
            }
            let mut ship_maintenance = Amt::ZERO;
            let mut ships_based_here = 0u32;
            for (_, ship, _, _, _, _) in ships_query.iter() {
                if colony.system(planets) == Some(ship.home_port) {
                    ship_maintenance = ship_maintenance.add(design_registry.maintenance(&ship.design_id));
                    ships_based_here += 1;
                }
            }
            let total_maintenance = building_maintenance.add(ship_maintenance);
            if total_maintenance > Amt::ZERO {
                ui.label(format!("Maintenance: {} E/hd", total_maintenance));
                ui.label(format!("  Buildings: {} E/hd", building_maintenance));
            }
            if ships_based_here > 0 {
                ui.label(format!(
                    "Ships based here: {} (maintenance: {} E/hd)",
                    ships_based_here, ship_maintenance
                ));
            }
        }

        // Note: Ship Build Queue and Build Ship UI moved to the system panel right pane (#134).
        // Ship construction is a system-level concern (shipyard is a system building).

        // #46: Planet buildings display and construction UI
        if let Some(buildings) = buildings {
            ui.separator();
            ui.label(egui::RichText::new("Planet Buildings").strong());

            let mut demolish_request: Option<(usize, BuildingId)> = None;
            let mut upgrade_request: Option<(usize, String, Amt, Amt, i64)> = None;

            // Collect pending building slots so we can show in-progress orders
            let pending_orders: Vec<(usize, String, f32)> = building_queue
                .as_ref()
                .map(|bq| {
                    bq.queue
                        .iter()
                        .map(|order| {
                            let def = building_registry.get(order.building_id.as_str());
                            let (total_m, total_e) = def.map(|d| d.build_cost()).unwrap_or((Amt::ZERO, Amt::ZERO));
                            let m_pct = if total_m.raw() > 0 {
                                1.0 - (order.minerals_remaining.raw() as f32 / total_m.raw() as f32)
                            } else {
                                1.0
                            };
                            let e_pct = if total_e.raw() > 0 {
                                1.0 - (order.energy_remaining.raw() as f32 / total_e.raw() as f32)
                            } else {
                                1.0
                            };
                            let bt_time = def.map(|d| d.build_time).unwrap_or(10);
                            let time_pct = if bt_time > 0 {
                                1.0 - (order.build_time_remaining as f32 / bt_time as f32)
                            } else {
                                1.0
                            };
                            let pct = m_pct.min(e_pct).min(time_pct).max(0.0);
                            let name = def.map(|d| d.name.clone()).unwrap_or_else(|| order.building_id.0.clone());
                            (order.target_slot, name, pct)
                        })
                        .collect()
                })
                .unwrap_or_default();

            let bldg_cost_mod = construction_params.building_cost_modifier.final_value();
            let bldg_time_mod = construction_params.building_build_time_modifier.final_value();

            for (i, slot) in buildings.slots.iter().enumerate() {
                let is_demolishing = building_queue
                    .as_ref()
                    .map(|bq| bq.is_demolishing(i))
                    .unwrap_or(false);
                let is_upgrading = building_queue
                    .as_ref()
                    .map(|bq| bq.is_upgrading(i))
                    .unwrap_or(false);

                match slot {
                    Some(bid) if is_demolishing => {
                        let remaining = building_queue
                            .as_ref()
                            .and_then(|bq| bq.demolition_time_remaining(i))
                            .unwrap_or(0);
                        let name = building_registry.get(bid.as_str()).map(|d| d.name.as_str()).unwrap_or(bid.as_str());
                        ui.label(format!(
                            "  [{}] {} — Demolishing... ({} hd remaining)",
                            i, name, remaining
                        ));
                    }
                    Some(bid) if is_upgrading => {
                        let upgrade_info = building_queue
                            .as_ref()
                            .and_then(|bq| bq.upgrade_info(i));
                        let name = building_registry.get(bid.as_str()).map(|d| d.name.as_str()).unwrap_or(bid.as_str());
                        let target_name = upgrade_info
                            .and_then(|u| building_registry.get(u.target_id.as_str()))
                            .map(|d| d.name.as_str())
                            .unwrap_or("?");
                        let remaining = upgrade_info.map(|u| u.build_time_remaining).unwrap_or(0);
                        ui.label(format!(
                            "  [{}] {} — Upgrading to {} ({} hd remaining)",
                            i, name, target_name, remaining
                        ));
                    }
                    Some(bid) => {
                        let def = building_registry.get(bid.as_str());
                        let name = def.map(|d| d.name.as_str()).unwrap_or(bid.as_str());
                        let (m_refund, e_refund) = def.map(|d| d.demolition_refund()).unwrap_or((Amt::ZERO, Amt::ZERO));
                        let demo_time = def.map(|d| d.demolition_time()).unwrap_or(0);
                        ui.horizontal(|ui| {
                            ui.label(format!("  [{}] {}", i, name));
                            let tooltip = format!(
                                "Demolish: {} hd | Refund M:{} E:{}",
                                demo_time, m_refund, e_refund
                            );
                            if ui
                                .small_button("Demolish")
                                .on_hover_text(tooltip)
                                .clicked()
                            {
                                demolish_request = Some((i, bid.clone()));
                            }
                            // Show upgrade buttons if upgrade paths exist
                            if let Some(src_def) = def {
                                for up in &src_def.upgrade_to {
                                    let target_def = building_registry.get(&up.target_id);
                                    let target_name = target_def.map(|d| d.name.as_str()).unwrap_or(&up.target_id);
                                    let eff_m = up.cost_minerals.mul_amt(bldg_cost_mod);
                                    let eff_e = up.cost_energy.mul_amt(bldg_cost_mod);
                                    let base_time = up.build_time.unwrap_or_else(|| {
                                        target_def.map(|d| d.build_time / 2).unwrap_or(5)
                                    });
                                    let eff_time = (base_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                                    let tooltip = format!(
                                        "Upgrade to {} (M:{} E:{} | {} hd)",
                                        target_name, eff_m, eff_e, eff_time
                                    );
                                    let btn_label = format!("-> {}", target_name);
                                    if ui
                                        .small_button(&btn_label)
                                        .on_hover_text(tooltip)
                                        .clicked()
                                    {
                                        upgrade_request = Some((i, up.target_id.clone(), eff_m, eff_e, eff_time));
                                    }
                                }
                            }
                        });
                    }
                    None => {
                        if let Some((_, name, pct)) = pending_orders.iter().find(|(s, _, _)| *s == i) {
                            ui.horizontal(|ui| {
                                ui.label(format!("  [{}] (Building: {})", i, name));
                                let bar = egui::ProgressBar::new(*pct).desired_width(80.0);
                                ui.add(bar);
                            });
                        } else {
                            ui.label(format!("  [{}] (empty)", i));
                        }
                    }
                }
            }

            if let Some((slot_idx, bid)) = demolish_request {
                if let Some(bq) = building_queue.as_mut() {
                    let def = building_registry.get(bid.as_str());
                    let (m_refund, e_refund) = def.map(|d| d.demolition_refund()).unwrap_or((Amt::ZERO, Amt::ZERO));
                    let demo_time = def.map(|d| d.demolition_time()).unwrap_or(0);
                    bq.demolition_queue.push(DemolitionOrder {
                        target_slot: slot_idx,
                        building_id: bid.clone(),
                        time_remaining: demo_time,
                        minerals_refund: m_refund,
                        energy_refund: e_refund,
                    });
                    info!("Demolition order added: {} in slot {}", bid, slot_idx);
                }
            }
            if let Some((slot_idx, target_id, minerals, energy, time)) = upgrade_request {
                if let Some(bq) = building_queue.as_mut() {
                    bq.upgrade_queue.push(UpgradeOrder {
                        slot_index: slot_idx,
                        target_id: BuildingId::new(&target_id),
                        minerals_remaining: minerals,
                        energy_remaining: energy,
                        build_time_remaining: time,
                    });
                    info!("Upgrade order added: {} in slot {}", target_id, slot_idx);
                }
            }

            let pending_slots: Vec<usize> = pending_orders.iter().map(|(s, _, _)| *s).collect();
            let empty_slot = buildings
                .slots
                .iter()
                .enumerate()
                .position(|(i, s)| s.is_none() && !pending_slots.contains(&i));

            if let Some(slot_idx) = empty_slot {
                ui.separator();
                ui.label(egui::RichText::new("Build Planet Building").strong());
                let planet_building_defs = building_registry.planet_buildings();
                let mut build_building_request: Option<BuildingId> = None;
                for def in &planet_building_defs {
                    let (base_m, base_e) = def.build_cost();
                    let eff_m = base_m.mul_amt(bldg_cost_mod);
                    let eff_e = base_e.mul_amt(bldg_cost_mod);
                    let eff_time = (def.build_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                    let tooltip = format!("M:{} E:{} | {} hexadies", eff_m, eff_e, eff_time);
                    if ui.button(&def.name).on_hover_text(tooltip).clicked() {
                        build_building_request = Some(BuildingId::new(&def.id));
                    }
                }
                if let Some(bid) = build_building_request {
                    if let Some(mut bq) = building_queue {
                        let def = building_registry.get(bid.as_str());
                        let (base_m, base_e) = def.map(|d| d.build_cost()).unwrap_or((Amt::ZERO, Amt::ZERO));
                        let base_time = def.map(|d| d.build_time).unwrap_or(10);
                        let eff_m = base_m.mul_amt(bldg_cost_mod);
                        let eff_e = base_e.mul_amt(bldg_cost_mod);
                        let eff_time = (base_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                        bq.queue.push(BuildingOrder {
                            building_id: bid.clone(),
                            target_slot: slot_idx,
                            minerals_remaining: eff_m,
                            energy_remaining: eff_e,
                            build_time_remaining: eff_time,
                        });
                        info!("Building order added: {} in slot {}", bid, slot_idx);
                    }
                }
            }
        }

        break;
    }
}
