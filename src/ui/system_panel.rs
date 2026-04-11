use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildOrder, BuildQueue, BuildingOrder, BuildingQueue, Buildings, Colony, ColonizationQueue, ConstructionParams, DemolitionOrder, FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile, SystemBuildings, SystemBuildingQueue, UpgradeOrder};
use crate::scripting::building_api::{BuildingId, BuildingRegistry};
use crate::components::Position;
use crate::galaxy::{Habitability, Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{AboardShip, Player, StationedAt};
use crate::amount::Amt;
use crate::ship::{Cargo, Ship, ShipHitpoints, ShipState, SurveyData};
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};
use crate::visualization::{SelectedPlanet, SelectedShip, SelectedSystem};

use super::ship_panel::ships_docked_at;

/// #114: Action to start colonizing a planet from the system panel build queue.
pub struct ColonizationAction {
    pub system_entity: Entity,
    pub target_planet: Entity,
    pub source_colony: Entity,
}

/// Draws the right-side system info panel when a star system is selected.
/// Shows star system overview, planet list with selection, and colony detail
/// for the selected planet.
#[allow(clippy::too_many_arguments)]
pub fn draw_system_panel(
    ctx: &egui::Context,
    selected_system: &mut SelectedSystem,
    selected_ship: &mut SelectedShip,
    selected_planet: &mut SelectedPlanet,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: &Query<(Entity, &StationedAt, Option<&AboardShip>), With<Player>>,
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
    system_stockpiles: &mut Query<(&mut ResourceStockpile, Option<&ResourceCapacity>), With<StarSystem>>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    positions: &Query<&Position>,
    knowledge: &KnowledgeStore,
    clock: &GameClock,
    construction_params: &ConstructionParams,
    planets: &Query<&Planet>,
    planet_entities: &Query<(Entity, &Planet, Option<&SystemAttributes>)>,
    system_buildings_q: &mut Query<(Option<&mut SystemBuildings>, Option<&mut SystemBuildingQueue>)>,
    hull_registry: &crate::ship_design::HullRegistry,
    module_registry: &crate::ship_design::ModuleRegistry,
    design_registry: &crate::ship_design::ShipDesignRegistry,
    colonization_queues: &Query<&ColonizationQueue>,
    colonization_actions_out: &mut Vec<ColonizationAction>,
    building_registry: &BuildingRegistry,
    anomalies_q: &Query<&crate::galaxy::Anomalies>,
) {
    let Some(sel_entity) = selected_system.0 else {
        return;
    };

    let Ok((_, star, star_pos, _)) = stars.get(sel_entity) else {
        return;
    };

    // #176: Determine if this is the player's local system
    let player_system = player_q.iter().next().map(|(_, s, _)| s.system);
    let is_local_system = player_system == Some(sel_entity);
    let k_data = if is_local_system { None } else { knowledge.get(sel_entity) };

    // Collect planets in this system with attributes for map rendering
    let colonized_planets: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter(|(_, c, _, _, _, _, _, _)| c.system(planets) == Some(sel_entity))
        .map(|(_, c, _, _, _, _, _, _)| c.planet)
        .collect();

    // Collect full planet info: entity, name, type, colonized, habitability
    let mut system_planets: Vec<(Entity, String, String, bool, Option<Habitability>)> = Vec::new();
    for (planet_entity, planet, attrs) in planet_entities.iter() {
        if planet.system == sel_entity {
            let is_colonized = colonized_planets.contains(&planet_entity);
            let hab = attrs.map(|a| a.habitability);
            system_planets.push((planet_entity, planet.name.clone(), planet.planet_type.clone(), is_colonized, hab));
        }
    }
    system_planets.sort_by(|a, b| a.1.cmp(&b.1));

    // Auto-select planet: if no planet selected or selected planet not in this system,
    // pick first colonized planet, or first planet
    let current_planet_valid = selected_planet.0
        .map(|pe| system_planets.iter().any(|(e, _, _, _, _)| *e == pe))
        .unwrap_or(false);
    if !current_planet_valid {
        selected_planet.0 = system_planets.iter()
            .find(|(_, _, _, colonized, _)| *colonized)
            .or(system_planets.first())
            .map(|(e, _, _, _, _)| *e);
    }

    let screen = ctx.screen_rect();
    let mut close_system_view = false;
    egui::Window::new(format!("{} ({})", star.name, format_star_type(&star.star_type)))
        .default_pos(egui::pos2(0.0, 30.0))
        .default_size(egui::vec2(screen.width(), screen.height() - 60.0))
        .title_bar(true)
        .collapsible(false)
        .show(ctx, |ui| {
            // === Back to Galaxy button ===
            if ui.button("\u{2190} Back to Galaxy").clicked() {
                close_system_view = true;
            }
            ui.separator();

            if let Ok((_, stationed, _)) = player_q.single() {
                if let Ok(player_pos) = positions.get(stationed.system) {
                    let dist = physics::distance_ly(player_pos, star_pos);
                    let delay_sd = physics::light_delay_hexadies(dist);
                    let delay_yr = physics::light_delay_years(dist);
                    ui.label(format!("Distance: {:.1} ly", dist));
                    ui.label(format!("Light delay: {} hd ({:.1} yr)", delay_sd, delay_yr));
                }
            }

            // #176: Survey status from knowledge for remote systems
            let effective_surveyed = if is_local_system {
                star.surveyed
            } else {
                k_data.map(|k| k.data.surveyed).unwrap_or(false)
            };
            if effective_surveyed {
                ui.label("Status: Surveyed");
            } else {
                ui.label("Status: Unsurveyed");
                ui.label("Approximate position only. Survey required.");
            }

            if let Some(age) = knowledge.info_age(sel_entity, clock.elapsed) {
                let years = age as f64 / HEXADIES_PER_YEAR as f64;
                let freshness = if age < 60 {
                    "FRESH"
                } else if age < 300 {
                    "AGING"
                } else if age < 600 {
                    "OLD"
                } else {
                    "VERY OLD"
                };
                ui.label(format!("Info age: {} hd ({:.1} yr) [{}]", age, years, freshness));
            }

            // #176: Remote system knowledge summary
            if !is_local_system {
                if let Some(k) = k_data {
                    let snap = &k.data;
                    ui.separator();
                    ui.label(egui::RichText::new("Remote Intelligence (light-speed delayed)").strong()
                        .color(egui::Color32::from_rgb(200, 180, 100)));
                    if snap.colonized {
                        ui.label(format!("Stockpile: M {} | E {} | F {} | A {}",
                            snap.minerals, snap.energy, snap.food, snap.authority));
                        if snap.production_minerals > Amt::ZERO || snap.production_energy > Amt::ZERO
                            || snap.production_food > Amt::ZERO || snap.production_research > Amt::ZERO
                        {
                            ui.label(format!("Production/hd: M {} | E {} | F {} | R {}",
                                snap.production_minerals, snap.production_energy,
                                snap.production_food, snap.production_research));
                        }
                        if snap.maintenance_energy > Amt::ZERO {
                            ui.label(format!("Maintenance: E {}/hd", snap.maintenance_energy));
                        }
                    }
                    if snap.has_hostile {
                        ui.label(egui::RichText::new(format!("Hostile presence (str: {:.1})", snap.hostile_strength))
                            .color(egui::Color32::from_rgb(255, 100, 100)));
                    }
                    if snap.has_port {
                        ui.label("Port facility present");
                    }
                    if snap.has_shipyard {
                        ui.label("Shipyard present");
                    }
                } else if !star.is_capital {
                    ui.separator();
                    ui.label(egui::RichText::new("No intelligence available for this system.")
                        .weak().italics());
                }
            }

            // === Anomalies / Points of Interest ===
            if let Ok(anomalies) = anomalies_q.get(sel_entity) {
                if !anomalies.discoveries.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new(format!("Anomalies ({})", anomalies.discoveries.len())).strong());
                    for anomaly in &anomalies.discoveries {
                        let discovered_yr = anomaly.discovered_at as f64 / crate::time_system::HEXADIES_PER_YEAR as f64;
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&anomaly.name).color(egui::Color32::from_rgb(200, 180, 100)));
                            ui.label(egui::RichText::new(format!("(t={:.1}yr)", discovered_yr)).weak().small());
                        });
                        ui.label(egui::RichText::new(&anomaly.description).weak().small());
                    }
                }
            }

            // === System Map Canvas ===
            if !system_planets.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("System Map").strong());
                ui.label("Click a planet to view details.");

                // Calculate map height: reserve space for panels below
                let map_height = (ui.available_height() - 200.0).max(150.0).min(400.0);
                let (response, painter) = ui.allocate_painter(
                    egui::vec2(ui.available_width(), map_height),
                    egui::Sense::click(),
                );
                let rect = response.rect;
                let center = rect.center();

                // Dark background for the map area
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(10, 10, 20));

                // Draw central star
                painter.circle_filled(center, 15.0, egui::Color32::from_rgb(255, 220, 80));
                painter.circle_stroke(center, 15.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 180, 40)));

                // Scale orbits to fit the available space
                let max_orbit_radius = (rect.width().min(rect.height()) / 2.0 - 30.0).max(50.0);
                let planet_count = system_planets.len();
                let orbit_spacing = if planet_count > 1 {
                    (max_orbit_radius - 40.0) / (planet_count as f32)
                } else {
                    max_orbit_radius * 0.5
                };

                // Draw planets on orbital rings
                for (i, (planet_entity, name, _planet_type, is_colonized, hab)) in system_planets.iter().enumerate() {
                    let orbit_r = 40.0 + (i as f32) * orbit_spacing;

                    // Orbit ring
                    painter.circle_stroke(
                        center,
                        orbit_r,
                        egui::Stroke::new(0.5, egui::Color32::from_rgba_premultiplied(80, 80, 80, 40)),
                    );

                    // Planet position (spread around orbit)
                    let angle = (i as f32) * 2.1 + 0.5;
                    let px = center.x + orbit_r * angle.cos();
                    let py = center.y + orbit_r * angle.sin();
                    let planet_pos = egui::pos2(px, py);

                    // Planet color based on habitability
                    let planet_color = match hab {
                        Some(Habitability::Ideal) => egui::Color32::from_rgb(50, 200, 50),
                        Some(Habitability::Adequate) => egui::Color32::from_rgb(150, 200, 50),
                        Some(Habitability::Marginal) => egui::Color32::from_rgb(200, 150, 50),
                        Some(Habitability::GasGiant) => egui::Color32::from_rgb(200, 130, 80),
                        Some(Habitability::Barren) => egui::Color32::from_rgb(130, 130, 130),
                        None => egui::Color32::from_rgb(100, 100, 100),
                    };

                    let planet_radius = match hab {
                        Some(Habitability::GasGiant) => 12.0,
                        _ => 8.0,
                    };

                    // Selected planet highlight
                    let is_selected = selected_planet.0 == Some(*planet_entity);
                    if is_selected {
                        painter.circle_filled(planet_pos, planet_radius + 4.0, egui::Color32::from_rgba_premultiplied(255, 255, 100, 40));
                        painter.circle_stroke(planet_pos, planet_radius + 4.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 255, 100)));
                    }

                    // Planet body
                    painter.circle_filled(planet_pos, planet_radius, planet_color);

                    // Colonized indicator ring
                    if *is_colonized {
                        painter.circle_stroke(planet_pos, planet_radius + 2.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(50, 130, 255)));
                    }

                    // Planet name label
                    painter.text(
                        egui::pos2(px, py + planet_radius + 4.0),
                        egui::Align2::CENTER_TOP,
                        name,
                        egui::FontId::proportional(10.0),
                        egui::Color32::from_rgb(200, 200, 200),
                    );

                    // Click detection on this planet
                    let click_radius = planet_radius + 6.0;
                    if response.clicked() {
                        if let Some(pointer_pos) = response.interact_pointer_pos() {
                            let dx = pointer_pos.x - px;
                            let dy = pointer_pos.y - py;
                            if (dx * dx + dy * dy).sqrt() <= click_radius {
                                selected_planet.0 = Some(*planet_entity);
                            }
                        }
                    }
                }
            }

            // === System Buildings ===
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
            if let Ok((Some(sys_bldgs), sys_bldg_queue)) = system_buildings_q.get_mut(sel_entity) {
                ui.separator();
                ui.label(egui::RichText::new("System Buildings").strong());

                let mut sys_demolish_request: Option<(usize, BuildingId)> = None;
                let mut sys_upgrade_request: Option<(usize, String, Amt, Amt, i64)> = None;
                let sys_bldg_cost_mod = construction_params.building_cost_modifier.final_value();
                let sys_bldg_time_mod = construction_params.building_build_time_modifier.final_value();

                for (i, slot) in sys_bldgs.slots.iter().enumerate() {
                    let is_demolishing = sys_bldg_queue
                        .as_ref()
                        .map(|bq| bq.is_demolishing(i))
                        .unwrap_or(false);
                    let is_upgrading = sys_bldg_queue
                        .as_ref()
                        .map(|bq| bq.is_upgrading(i))
                        .unwrap_or(false);

                    match slot {
                        Some(bid) if is_demolishing => {
                            let remaining = sys_bldg_queue
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
                            let upgrade_info = sys_bldg_queue
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
                                    sys_demolish_request = Some((i, bid.clone()));
                                }
                                // Show upgrade buttons if upgrade paths exist
                                if let Some(src_def) = def {
                                    for up in &src_def.upgrade_to {
                                        let target_def = building_registry.get(&up.target_id);
                                        let target_name = target_def.map(|d| d.name.as_str()).unwrap_or(&up.target_id);
                                        let eff_m = up.cost_minerals.mul_amt(sys_bldg_cost_mod);
                                        let eff_e = up.cost_energy.mul_amt(sys_bldg_cost_mod);
                                        let base_time = up.build_time.unwrap_or_else(|| {
                                            target_def.map(|d| d.build_time / 2).unwrap_or(5)
                                        });
                                        let eff_time = (base_time as f64 * sys_bldg_time_mod.to_f64()).ceil() as i64;
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
                                            sys_upgrade_request = Some((i, up.target_id.clone(), eff_m, eff_e, eff_time));
                                        }
                                    }
                                }
                            });
                        }
                        None => {
                            ui.label(format!("  [{}] (empty)", i));
                        }
                    }
                }

                if let Some((slot_idx, bid)) = sys_demolish_request {
                    if let Some(mut bq) = sys_bldg_queue {
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
                        info!("System building demolition order added: {} in slot {}", bid, slot_idx);
                    }
                }
                if let Some((slot_idx, target_id, minerals, energy, time)) = sys_upgrade_request {
                    if let Ok((_, Some(mut bq))) = system_buildings_q.get_mut(sel_entity) {
                        bq.upgrade_queue.push(UpgradeOrder {
                            slot_index: slot_idx,
                            target_id: BuildingId::new(&target_id),
                            minerals_remaining: minerals,
                            energy_remaining: energy,
                            build_time_remaining: time,
                        });
                        info!("System building upgrade order added: {} in slot {}", target_id, slot_idx);
                    }
                }

                // Build system building buttons
                if let Ok((Some(sys_bldgs_read), sys_bq_read)) = system_buildings_q.get(sel_entity) {
                    let pending_slots: Vec<usize> = sys_bq_read
                        .map(|bq| bq.queue.iter().map(|o| o.target_slot).collect())
                        .unwrap_or_default();
                    let empty_slot = sys_bldgs_read
                        .slots
                        .iter()
                        .enumerate()
                        .position(|(i, s)| s.is_none() && !pending_slots.contains(&i));

                    if let Some(slot_idx) = empty_slot {
                        ui.separator();
                        ui.label(egui::RichText::new("Build System Building").strong());
                        let system_building_defs = building_registry.system_buildings();
                        let bldg_cost_mod = construction_params.building_cost_modifier.final_value();
                        let bldg_time_mod = construction_params.building_build_time_modifier.final_value();
                        let mut build_sys_building_request: Option<BuildingId> = None;
                        for def in &system_building_defs {
                            let (base_m, base_e) = def.build_cost();
                            let eff_m = base_m.mul_amt(bldg_cost_mod);
                            let eff_e = base_e.mul_amt(bldg_cost_mod);
                            let eff_time = (def.build_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                            let tooltip = format!("M:{} E:{} | {} hexadies", eff_m, eff_e, eff_time);
                            if ui.button(&def.name).on_hover_text(tooltip).clicked() {
                                build_sys_building_request = Some(BuildingId::new(&def.id));
                            }
                        }
                        if let Some(bid) = build_sys_building_request {
                            if let Ok((_, Some(mut bq))) = system_buildings_q.get_mut(sel_entity) {
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
                                info!("System building order added: {} in slot {}", bid, slot_idx);
                            }
                        }
                    }
                }
            }

            // === #114: Same-system colonization ===
            // Show if this system has at least one colony and uncolonized habitable planets
            if !colonized_planets.is_empty() {
                // Collect uncolonized habitable planets in this system
                let mut colonizable: Vec<(Entity, String, String)> = Vec::new();
                for (pe, planet, attrs) in planet_entities.iter() {
                    if planet.system == sel_entity
                        && !colonized_planets.contains(&pe)
                        && attrs.map(|a| {
                            a.habitability != crate::galaxy::Habitability::Barren
                                && a.habitability != crate::galaxy::Habitability::GasGiant
                        }).unwrap_or(false)
                    {
                        // Check not already in colonization queue
                        let in_queue = colonization_queues.get(sel_entity)
                            .map(|cq| cq.orders.iter().any(|o| o.target_planet == pe))
                            .unwrap_or(false);
                        if !in_queue {
                            colonizable.push((pe, planet.name.clone(), format_planet_type(&planet.planet_type)));
                        }
                    }
                }
                colonizable.sort_by(|a, b| a.1.cmp(&b.1));

                if !colonizable.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("Colonize Planet").strong());
                    ui.label(format!(
                        "Cost: {} minerals, {} energy, {} hd",
                        crate::colony::COLONIZATION_MINERAL_COST,
                        crate::colony::COLONIZATION_ENERGY_COST,
                        crate::colony::COLONIZATION_BUILD_TIME,
                    ));

                    // Find a source colony with enough population
                    let source_colony: Option<Entity> = colonies.iter()
                        .find(|(_, c, _, _, _, _, _, _)| {
                            c.system(planets) == Some(sel_entity)
                                && c.population > crate::colony::COLONIZATION_MIN_POPULATION
                        })
                        .map(|(e, _, _, _, _, _, _, _)| e);

                    for (pe, name, ptype) in &colonizable {
                        let label = format!("{} ({})", name, ptype);
                        let enabled = source_colony.is_some();
                        if ui.add_enabled(enabled, egui::Button::new(format!("Colonize {}", label))).clicked() {
                            if let Some(source) = source_colony {
                                colonization_actions_out.push(ColonizationAction {
                                    system_entity: sel_entity,
                                    target_planet: *pe,
                                    source_colony: source,
                                });
                            }
                        }
                    }

                    if source_colony.is_none() {
                        ui.label(
                            egui::RichText::new(format!(
                                "(Need colony with >{:.0} pop)",
                                crate::colony::COLONIZATION_MIN_POPULATION
                            ))
                            .small()
                            .color(egui::Color32::from_rgb(200, 200, 100)),
                        );
                    }
                }

                // Show in-progress colonization orders
                if let Ok(cq) = colonization_queues.get(sel_entity) {
                    if !cq.orders.is_empty() {
                        ui.separator();
                        ui.label(egui::RichText::new("Colonization In Progress").strong());
                        for order in &cq.orders {
                            let planet_name = planets.get(order.target_planet)
                                .map(|p| p.name.as_str())
                                .unwrap_or("Unknown");
                            let total_time = crate::colony::COLONIZATION_BUILD_TIME;
                            let elapsed = total_time - order.build_time_remaining;
                            let pct = if total_time > 0 { elapsed as f32 / total_time as f32 } else { 1.0 };
                            ui.label(format!("{}: {}/{} hd ({:.0}%)", planet_name, elapsed, total_time, pct * 100.0));
                            let bar = egui::ProgressBar::new(pct);
                            ui.add(bar);
                        }
                    }
                }
            }

            // === Docked Ships ===
            ui.separator();
            let docked_ships = ships_docked_at(sel_entity, ships_query);
            if !docked_ships.is_empty() {
                ui.label(egui::RichText::new("Docked Ships").strong());
                for (entity, name, design_id) in &docked_ships {
                    let is_selected = selected_ship.0 == Some(*entity);
                    let design_name = design_registry.get(design_id).map(|d| d.name.as_str()).unwrap_or(design_id);
                    let label = format!(
                        "{} ({}){}",
                        name,
                        design_name,
                        if is_selected { " [selected]" } else { "" }
                    );
                    if ui.selectable_label(is_selected, &label).clicked() {
                        selected_ship.0 = Some(*entity);
                    }
                }
            }
                }); // end ScrollArea
        });
    if close_system_view {
        selected_system.0 = None;
    }

    // === Planet Info Window (independent floating window) ===
    draw_planet_window(
        ctx,
        sel_entity,
        selected_planet,
        &colonized_planets,
        stars,
        colonies,
        system_stockpiles,
        ships_query,
        construction_params,
        planets,
        planet_entities,
        hull_registry,
        module_registry,
        design_registry,
        building_registry,
    );
}

/// Draws the floating planet info window when a planet is selected.
/// Shows planet attributes, colony detail, buildings, and build queue.
#[allow(clippy::too_many_arguments)]
fn draw_planet_window(
    ctx: &egui::Context,
    system_entity: Entity,
    selected_planet: &mut SelectedPlanet,
    colonized_planets: &std::collections::HashSet<Entity>,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
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
    system_stockpiles: &mut Query<(&mut ResourceStockpile, Option<&ResourceCapacity>), With<StarSystem>>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    construction_params: &ConstructionParams,
    planets: &Query<&Planet>,
    planet_entities: &Query<(Entity, &Planet, Option<&SystemAttributes>)>,
    hull_registry: &crate::ship_design::HullRegistry,
    module_registry: &crate::ship_design::ModuleRegistry,
    design_registry: &crate::ship_design::ShipDesignRegistry,
    building_registry: &BuildingRegistry,
) {
    let Some(sel_planet_entity) = selected_planet.0 else {
        return;
    };

    // Verify planet belongs to this system
    let Ok((_, sel_planet, attrs)) = planet_entities.get(sel_planet_entity) else {
        return;
    };
    if sel_planet.system != system_entity {
        return;
    }

    let is_surveyed = stars.get(system_entity).map(|(_, s, _, _)| s.surveyed).unwrap_or(false);
    let planet_name = sel_planet.name.clone();
    let planet_type = format_planet_type(&sel_planet.planet_type);

    let mut open = true;
    egui::Window::new(format!("{} ({})", planet_name, planet_type))
        .id(egui::Id::new("planet_info_window"))
        .order(egui::Order::Foreground)
        .default_pos(egui::pos2(400.0, 200.0))
        .default_size(egui::vec2(350.0, 400.0))
        .resizable(true)
        .collapsible(true)
        .open(&mut open)
        .show(ctx, |ui| {
            // Planet attributes (if surveyed)
            if is_surveyed {
                if let Some(attrs) = attrs {
                    ui.label(egui::RichText::new("Attributes").strong());
                    ui.label(format!("Habitability: {:?}", attrs.habitability));
                    ui.label(format!("Minerals: {:?}", attrs.mineral_richness));
                    ui.label(format!("Energy: {:?}", attrs.energy_potential));
                    ui.label(format!("Research: {:?}", attrs.research_potential));
                    ui.label(format!("Building slots: {}", attrs.max_building_slots));
                    ui.separator();
                }
            } else {
                ui.label("System not yet surveyed.");
                ui.separator();
            }

            // Colony detail (if colonized)
            let has_colony_on_planet = colonized_planets.contains(&sel_planet_entity);
            if has_colony_on_planet {
                let planet_attrs = planet_entities.get(sel_planet_entity).ok().and_then(|(_, _, a)| a);

                egui::ScrollArea::vertical()
                    .max_height(500.0)
                    .show(ui, |ui| {
                        draw_colony_detail(
                            ui,
                            sel_planet_entity,
                            system_entity,
                            planet_attrs,
                            colonies,
                            system_stockpiles,
                            ships_query,
                            construction_params,
                            planets,
                            hull_registry,
                            module_registry,
                            design_registry,
                            building_registry,
                        );
                    });
            } else {
                ui.label(
                    egui::RichText::new("Uncolonized")
                        .color(egui::Color32::from_rgb(180, 180, 180)),
                );
            }
        });

    if !open {
        selected_planet.0 = None;
    }
}

/// Format a type id into a display name (e.g. "yellow_dwarf" -> "Yellow Dwarf").
fn format_star_type(type_id: &str) -> String {
    type_id
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Format a planet_type id into a display name.
fn format_planet_type(planet_type: &str) -> String {
    format_star_type(planet_type)
}

/// Draws colony detail for a specific planet. Called within a ScrollArea.
#[allow(clippy::too_many_arguments)]
fn draw_colony_detail(
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
    system_stockpiles: &mut Query<(&mut ResourceStockpile, Option<&ResourceCapacity>), With<StarSystem>>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    construction_params: &ConstructionParams,
    planets: &Query<&Planet>,
    hull_registry: &crate::ship_design::HullRegistry,
    module_registry: &crate::ship_design::ModuleRegistry,
    design_registry: &crate::ship_design::ShipDesignRegistry,
    building_registry: &BuildingRegistry,
) {
    ui.label(
        egui::RichText::new("Colony")
            .strong()
            .color(egui::Color32::from_rgb(100, 200, 100)),
    );

    for (_colony_entity, colony, production, build_queue, buildings, mut building_queue, maintenance_cost, food_consumption) in
        colonies.iter_mut()
    {
        if colony.planet != planet_entity {
            continue;
        }

        // #69: Show population with carrying capacity
        let carrying_cap = {
            use crate::amount::Amt;
            use crate::galaxy::{BASE_CARRYING_CAPACITY, FOOD_PER_POP_PER_HEXADIES};
            let hab_score = planet_attrs.map(|a| a.habitability.base_score()).unwrap_or(0.5);
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

        // Build queue
        if let Some(ref bq) = build_queue {
            ui.separator();
            ui.label(egui::RichText::new("Build Queue").strong());

            if bq.queue.is_empty() {
                ui.label("[empty]");
            } else {
                for order in &bq.queue {
                    let m_pct = if order.minerals_cost.raw() > 0 {
                        (order.minerals_invested.raw() as f32 / order.minerals_cost.raw() as f32).min(1.0)
                    } else {
                        1.0
                    };
                    let e_pct = if order.energy_cost.raw() > 0 {
                        (order.energy_invested.raw() as f32 / order.energy_cost.raw() as f32).min(1.0)
                    } else {
                        1.0
                    };
                    let time_pct = if order.build_time_total > 0 {
                        ((order.build_time_total - order.build_time_remaining) as f32
                            / order.build_time_total as f32)
                            .min(1.0)
                    } else {
                        1.0
                    };
                    let pct = m_pct.min(e_pct).min(time_pct);
                    ui.horizontal(|ui| {
                        ui.label(&order.display_name);
                        let bar = egui::ProgressBar::new(pct)
                            .desired_width(100.0);
                        ui.add(bar);
                        // Show what's blocking progress
                        if m_pct < 1.0 || e_pct < 1.0 {
                            ui.label(egui::RichText::new("(awaiting resources)").weak().small());
                        } else if time_pct < 1.0 {
                            ui.label(egui::RichText::new(format!("{} hd", order.build_time_remaining)).weak().small());
                        }
                    });
                }
            }

            ui.separator();
            ui.label(egui::RichText::new("Build Ship").strong());
        }

        // Build buttons - add orders to the queue from ShipDesignRegistry
        if let Some(mut bq) = build_queue {
            use crate::amount::Amt;
            let ship_mod = construction_params.ship_cost_modifier.final_value();
            let ship_time_mod = construction_params.ship_build_time_modifier.final_value();
            let mut build_request: Option<(String, String, Amt, Amt, i64)> = None;

            let design_ids = design_registry.all_design_ids();

            if !design_ids.is_empty() {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for design_id in &design_ids {
                            let design = &design_registry.designs[design_id];
                            // Calculate cost from hull + modules, fallback to design-level values
                            let hull = hull_registry.get(&design.hull_id);
                            let (base_m, base_e, base_time) = if let Some(hull) = hull {
                                let mods: Vec<_> = design.modules.iter()
                                    .filter_map(|a| module_registry.get(&a.module_id))
                                    .collect();
                                let (m, e, t, _maint) = crate::ship_design::design_cost(hull, &mods);
                                (m, e, t)
                            } else {
                                // Fallback to design-level costs
                                (design.build_cost_minerals, design.build_cost_energy, design.build_time)
                            };
                            let eff_m = base_m.mul_amt(ship_mod);
                            let eff_e = base_e.mul_amt(ship_mod);
                            let eff_time = (base_time as f64 * ship_time_mod.to_f64()).ceil() as i64;
                            let tooltip = format!("M:{} E:{} | {} hd", eff_m, eff_e, eff_time);
                            if ui.button(&design.name).on_hover_text(tooltip).clicked() {
                                build_request = Some((design_id.clone(), design.name.clone(), eff_m, eff_e, eff_time));
                            }
                        }
                    });
                });
            }

            if let Some((design_id, display_name, minerals_cost, energy_cost, build_time)) = build_request {
                bq.queue.push(BuildOrder {
                    design_id,
                    display_name: display_name.clone(),
                    minerals_cost,
                    minerals_invested: Amt::ZERO,
                    energy_cost,
                    energy_invested: Amt::ZERO,
                    build_time_total: build_time,
                    build_time_remaining: build_time,
                });
                info!("Build order added: {}", display_name);
            }
        }

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
