mod colony_detail;
mod planet_window;

use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildOrder, BuildQueue, BuildingOrder, BuildingQueue, Buildings, Colony, ColonizationQueue, ConstructionParams, DemolitionOrder, FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile, SystemBuildings, SystemBuildingQueue, UpgradeOrder};
use crate::scripting::building_api::{BuildingId, BuildingRegistry};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{AboardShip, Player, StationedAt};
use crate::amount::Amt;
use crate::ship::{Cargo, Ship, ShipHitpoints, ShipState, SurveyData};
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};
use crate::visualization::{SelectedPlanet, SelectedShip, SelectedSystem};

use super::ship_panel::ships_docked_at;
use planet_window::draw_planet_window;

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
    let mut system_planets: Vec<(Entity, String, String, bool, Option<f64>)> = Vec::new();
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

                    // Planet color based on habitability score
                    let planet_color = match hab {
                        Some(v) if *v >= 0.9 => egui::Color32::from_rgb(50, 200, 50),     // Ideal
                        Some(v) if *v >= 0.6 => egui::Color32::from_rgb(150, 200, 50),    // Adequate
                        Some(v) if *v >= 0.3 => egui::Color32::from_rgb(200, 150, 50),    // Marginal
                        Some(v) if *v > 0.0 => egui::Color32::from_rgb(130, 130, 130),    // Barren
                        Some(_) => egui::Color32::from_rgb(200, 130, 80),                  // Uninhabitable (gas giant)
                        None => egui::Color32::from_rgb(100, 100, 100),
                    };

                    let planet_radius = match hab {
                        Some(v) if *v <= 0.0 => 12.0, // Gas giant size
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
                        && attrs.map(|a| crate::galaxy::is_colonizable(a.habitability)).unwrap_or(false)
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
