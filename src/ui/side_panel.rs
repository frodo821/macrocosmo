use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildOrder, BuildQueue, BuildingOrder, BuildingQueue, BuildingType, Buildings, Colony, ConstructionParams, DemolitionOrder, FoodConsumption, MaintenanceCost, Production, ResourceStockpile};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::amount::Amt;
use crate::ship::{Cargo, CommandQueue, QueuedCommand, Ship, ShipState};
use crate::technology::GlobalParams;
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};
use crate::visualization::{SelectedPlanet, SelectedShip, SelectedSystem};

/// Action returned from draw_ship_panel when the player clicks "Scrap Ship".
/// Processed in draw_all_ui where Commands is available for despawning.
pub struct ShipScrapAction {
    pub ship_entity: Entity,
    pub colony_entity: Entity,
    pub ship_name: String,
    pub system_name: String,
    pub minerals_refund: Amt,
    pub energy_refund: Amt,
}

/// Draws the right-side system info panel when a star system is selected.
/// Shows star system overview, planet list with selection, and colony detail
/// for the selected planet.
#[allow(clippy::too_many_arguments)]
pub fn draw_system_panel(
    ctx: &egui::Context,
    selected_system: &SelectedSystem,
    selected_ship: &mut SelectedShip,
    selected_planet: &mut SelectedPlanet,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: &Query<&StationedAt, With<Player>>,
    colonies: &mut Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    positions: &Query<&Position>,
    knowledge: &KnowledgeStore,
    clock: &GameClock,
    construction_params: &ConstructionParams,
    planets: &Query<&Planet>,
    planet_entities: &Query<(Entity, &Planet, Option<&SystemAttributes>)>,
) {
    let Some(sel_entity) = selected_system.0 else {
        return;
    };

    let Ok((_, star, star_pos, _)) = stars.get(sel_entity) else {
        return;
    };

    // Collect planets in this system using the entity-bearing query
    let mut system_planets: Vec<(Entity, String, String, bool)> = Vec::new();
    let colonized_planets: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter(|(_, c, _, _, _, _, _, _, _)| c.system(planets) == Some(sel_entity))
        .map(|(_, c, _, _, _, _, _, _, _)| c.planet)
        .collect();

    for (planet_entity, planet, _attrs) in planet_entities.iter() {
        if planet.system == sel_entity {
            let is_colonized = colonized_planets.contains(&planet_entity);
            system_planets.push((planet_entity, planet.name.clone(), planet.planet_type.clone(), is_colonized));
        }
    }
    system_planets.sort_by(|a, b| a.1.cmp(&b.1));

    // Auto-select planet: if no planet selected or selected planet not in this system,
    // pick first colonized planet, or first planet
    let current_planet_valid = selected_planet.0
        .map(|pe| system_planets.iter().any(|(e, _, _, _)| *e == pe))
        .unwrap_or(false);
    if !current_planet_valid {
        selected_planet.0 = system_planets.iter()
            .find(|(_, _, _, colonized)| *colonized)
            .or(system_planets.first())
            .map(|(e, _, _, _)| *e);
    }

    egui::SidePanel::right("system_panel")
        .min_width(280.0)
        .show(ctx, |ui| {
            // === Star System Overview ===
            ui.heading(format!("{} ({})", star.name, format_star_type(&star.star_type)));
            ui.separator();

            if let Ok(stationed) = player_q.single() {
                if let Ok(player_pos) = positions.get(stationed.system) {
                    let dist = physics::distance_ly(player_pos, star_pos);
                    let delay_sd = physics::light_delay_hexadies(dist);
                    let delay_yr = physics::light_delay_years(dist);
                    ui.label(format!("Distance: {:.1} ly", dist));
                    ui.label(format!("Light delay: {} hd ({:.1} yr)", delay_sd, delay_yr));
                }
            }

            if star.surveyed {
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

            // === Planet List ===
            if !system_planets.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Planets").strong());

                for (planet_entity, name, planet_type, is_colonized) in &system_planets {
                    let is_selected = selected_planet.0 == Some(*planet_entity);
                    let prefix = if is_selected { "\u{25CF} " } else { "  " };
                    let status = if *is_colonized { " [COLONIZED]" } else { "" };
                    let label_text = format!(
                        "{}{} ({}){}",
                        prefix, name, format_planet_type(planet_type), status
                    );

                    let label = if *is_colonized {
                        egui::RichText::new(&label_text).color(egui::Color32::from_rgb(100, 200, 100))
                    } else {
                        egui::RichText::new(&label_text)
                    };

                    if ui.selectable_label(is_selected, label).clicked() {
                        selected_planet.0 = Some(*planet_entity);
                    }
                }
            }

            // === Selected Planet Detail ===
            if let Some(sel_planet_entity) = selected_planet.0 {
                // Show planet attributes if surveyed
                if star.surveyed {
                    if let Ok((_, sel_planet, Some(attrs))) = planet_entities.get(sel_planet_entity) {
                        ui.separator();
                        ui.label(egui::RichText::new(format!("{} — Attributes", sel_planet.name)).strong());
                        ui.label(format!("Habitability: {:?}", attrs.habitability));
                        ui.label(format!("Minerals: {:?}", attrs.mineral_richness));
                        ui.label(format!("Energy: {:?}", attrs.energy_potential));
                        ui.label(format!("Research: {:?}", attrs.research_potential));
                        ui.label(format!("Building slots: {}", attrs.max_building_slots));
                    }
                }

                // Colony detail section (scrollable)
                let has_colony_on_planet = colonized_planets.contains(&sel_planet_entity);
                if has_colony_on_planet {
                    ui.separator();

                    let planet_attrs = planet_entities.get(sel_planet_entity).ok().and_then(|(_, _, a)| a);

                    egui::ScrollArea::vertical()
                        .max_height(400.0)
                        .show(ui, |ui| {
                            draw_colony_detail(
                                ui,
                                sel_planet_entity,
                                planet_attrs,
                                colonies,
                                ships_query,
                                construction_params,
                                planets,
                            );
                        });
                }
            }

            // === Docked Ships ===
            ui.separator();
            let docked_ships = ships_docked_at(sel_entity, ships_query);
            if !docked_ships.is_empty() {
                ui.label(egui::RichText::new("Docked Ships").strong());
                for (entity, name, design_id) in &docked_ships {
                    let is_selected = selected_ship.0 == Some(*entity);
                    let design_name = crate::ship::design_preset(design_id).map(|p| p.design_name).unwrap_or(design_id);
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
        });
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
    planet_attrs: Option<&SystemAttributes>,
    colonies: &mut Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    construction_params: &ConstructionParams,
    planets: &Query<&Planet>,
) {
    ui.label(
        egui::RichText::new("Colony")
            .strong()
            .color(egui::Color32::from_rgb(100, 200, 100)),
    );

    for (_colony_entity, colony, production, stockpile, build_queue, buildings, mut building_queue, maintenance_cost, food_consumption) in
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

        if let Some(stockpile) = stockpile {
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
                    if let Some(bt) = slot {
                        building_maintenance = building_maintenance.add(bt.maintenance_cost());
                    }
                }
            }
            let mut ship_maintenance = Amt::ZERO;
            let mut ships_based_here = 0u32;
            for (_, ship, _, _) in ships_query.iter() {
                if colony.system(planets) == Some(ship.home_port) {
                    ship_maintenance = ship_maintenance.add(crate::ship::ship_maintenance_cost(&ship.design_id));
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
                    let pct = m_pct.min(e_pct);
                    ui.horizontal(|ui| {
                        ui.label(&order.display_name);
                        let bar = egui::ProgressBar::new(pct)
                            .desired_width(100.0);
                        ui.add(bar);
                    });
                }
            }

            ui.separator();
            ui.label(egui::RichText::new("Build Ship").strong());
        }

        // Build buttons - add orders to the queue
        if let Some(mut bq) = build_queue {
            use crate::amount::Amt;
            let ship_mod = construction_params.ship_cost_modifier.final_value();
            let ship_time_mod = construction_params.ship_build_time_modifier.final_value();
            let mut build_request: Option<(&str, &str, Amt, Amt, i64)> = None;
            ui.horizontal(|ui| {
                for preset in crate::ship::all_design_presets() {
                    let base_m = preset.build_cost_minerals;
                    let base_e = preset.build_cost_energy;
                    let base_time = preset.build_time;
                    let eff_m = base_m.mul_amt(ship_mod);
                    let eff_e = base_e.mul_amt(ship_mod);
                    let eff_time = (base_time as f64 * ship_time_mod.to_f64()).ceil() as i64;
                    let tooltip = format!("M:{} E:{} | {} hd", eff_m, eff_e, eff_time);
                    if ui.button(preset.design_name).on_hover_text(tooltip).clicked() {
                        build_request = Some((preset.design_id, preset.design_name, eff_m, eff_e, eff_time));
                    }
                }
            });
            if let Some((design_id, display_name, minerals_cost, energy_cost, build_time)) = build_request {
                bq.queue.push(BuildOrder {
                    design_id: design_id.to_string(),
                    display_name: display_name.to_string(),
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

        // #46: Buildings display and construction UI
        if let Some(buildings) = buildings {
            ui.separator();
            ui.label(egui::RichText::new("Buildings").strong());

            let mut demolish_request: Option<(usize, BuildingType)> = None;

            for (i, slot) in buildings.slots.iter().enumerate() {
                let is_demolishing = building_queue
                    .as_ref()
                    .map(|bq| bq.is_demolishing(i))
                    .unwrap_or(false);

                match slot {
                    Some(bt) if is_demolishing => {
                        let remaining = building_queue
                            .as_ref()
                            .and_then(|bq| bq.demolition_time_remaining(i))
                            .unwrap_or(0);
                        ui.label(format!(
                            "  [{}] {} — Demolishing... ({} hd remaining)",
                            i,
                            bt.name(),
                            remaining
                        ));
                    }
                    Some(bt) => {
                        ui.horizontal(|ui| {
                            ui.label(format!("  [{}] {}", i, bt.name()));
                            let (m_refund, e_refund) = bt.demolition_refund();
                            let demo_time = bt.demolition_time();
                            let tooltip = format!(
                                "Demolish: {} hd | Refund M:{} E:{}",
                                demo_time, m_refund, e_refund
                            );
                            if ui
                                .small_button("Demolish")
                                .on_hover_text(tooltip)
                                .clicked()
                            {
                                demolish_request = Some((i, *bt));
                            }
                        });
                    }
                    None => {
                        ui.label(format!("  [{}] (empty)", i));
                    }
                }
            }

            if let Some((slot_idx, bt)) = demolish_request {
                if let Some(bq) = building_queue.as_mut() {
                    let (m_refund, e_refund) = bt.demolition_refund();
                    bq.demolition_queue.push(DemolitionOrder {
                        target_slot: slot_idx,
                        building_type: bt,
                        time_remaining: bt.demolition_time(),
                        minerals_refund: m_refund,
                        energy_refund: e_refund,
                    });
                    info!("Demolition order added: {:?} in slot {}", bt, slot_idx);
                }
            }

            let pending_slots: Vec<usize> = building_queue
                .as_ref()
                .map(|bq| bq.queue.iter().map(|o| o.target_slot).collect())
                .unwrap_or_default();
            let empty_slot = buildings
                .slots
                .iter()
                .enumerate()
                .position(|(i, s)| s.is_none() && !pending_slots.contains(&i));

            if let Some(slot_idx) = empty_slot {
                ui.separator();
                ui.label(egui::RichText::new("Build Building").strong());
                let building_types = [
                    BuildingType::Mine,
                    BuildingType::PowerPlant,
                    BuildingType::ResearchLab,
                    BuildingType::Shipyard,
                    BuildingType::Port,
                    BuildingType::Farm,
                ];
                let bldg_cost_mod = construction_params.building_cost_modifier.final_value();
                let bldg_time_mod = construction_params.building_build_time_modifier.final_value();
                let mut build_building_request: Option<BuildingType> = None;
                for bt in &building_types {
                    let (base_m, base_e) = bt.build_cost();
                    let eff_m = base_m.mul_amt(bldg_cost_mod);
                    let eff_e = base_e.mul_amt(bldg_cost_mod);
                    let eff_time = (bt.build_time() as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                    let tooltip = format!("M:{} E:{} | {} hexadies", eff_m, eff_e, eff_time);
                    if ui.button(bt.name()).on_hover_text(tooltip).clicked() {
                        build_building_request = Some(*bt);
                    }
                }
                if let Some(bt) = build_building_request {
                    if let Some(mut bq) = building_queue {
                        let (base_m, base_e) = bt.build_cost();
                        let eff_m = base_m.mul_amt(bldg_cost_mod);
                        let eff_e = base_e.mul_amt(bldg_cost_mod);
                        let eff_time = (bt.build_time() as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                        bq.queue.push(BuildingOrder {
                            building_type: bt,
                            target_slot: slot_idx,
                            minerals_remaining: eff_m,
                            energy_remaining: eff_e,
                            build_time_remaining: eff_time,
                        });
                        info!("Building order added: {:?} in slot {}", bt, slot_idx);
                    }
                }
            }
        }

        break;
    }
}

/// Resolve an Entity to a star system name, falling back to "Unknown".
fn system_name(
    entity: Entity,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> String {
    stars
        .get(entity)
        .map(|(_, s, _, _)| s.name.clone())
        .unwrap_or_else(|_| "Unknown".to_string())
}

/// Collected status information for the ship panel UI.
struct ShipStatusInfo {
    label: String,
    /// Progress fraction 0.0..=1.0, if applicable.
    progress: Option<(i64, i64, f32)>, // (elapsed, total, fraction)
}

/// Build a detailed status string (and optional progress) from a ShipState.
fn build_status_info(
    state: &ShipState,
    clock: &GameClock,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> ShipStatusInfo {
    match state {
        ShipState::Docked { system } => ShipStatusInfo {
            label: format!("Docked at {}", system_name(*system, stars)),
            progress: None,
        },
        ShipState::SubLight {
            target_system,
            departed_at,
            arrival_at,
            ..
        } => {
            let total = (arrival_at - departed_at).max(1);
            let elapsed = (clock.elapsed - departed_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            let dest_name = target_system
                .map(|e| system_name(e, stars))
                .unwrap_or_else(|| "deep space".to_string());
            ShipStatusInfo {
                label: format!(
                    "Moving to {} ({}/{} hd, {:.0}%)",
                    dest_name,
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
        ShipState::InFTL {
            destination_system,
            departed_at,
            arrival_at,
            ..
        } => {
            let total = (arrival_at - departed_at).max(1);
            let elapsed = (clock.elapsed - departed_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "FTL to {} ({}/{} hd, {:.0}%)",
                    system_name(*destination_system, stars),
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
        ShipState::Surveying {
            target_system,
            started_at,
            completes_at,
        } => {
            let total = (completes_at - started_at).max(1);
            let elapsed = (clock.elapsed - started_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "Surveying {} ({}/{} hd, {:.0}%)",
                    system_name(*target_system, stars),
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
        ShipState::Settling {
            system,
            started_at,
            completes_at,
        } => {
            let total = (completes_at - started_at).max(1);
            let elapsed = (clock.elapsed - started_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "Settling {} ({}/{} hd, {:.0}%)",
                    system_name(*system, stars),
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
    }
}

/// Format a QueuedCommand as a human-readable string.
fn format_queued_command(
    cmd: &QueuedCommand,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> String {
    match cmd {
        QueuedCommand::MoveTo { system, .. } => {
            format!("Move -> {}", system_name(*system, stars))
        }
        QueuedCommand::FTLTo { system, .. } => {
            format!("FTL -> {}", system_name(*system, stars))
        }
        QueuedCommand::Survey { system, .. } => {
            format!("Survey {}", system_name(*system, stars))
        }
        QueuedCommand::Colonize { .. } => "Colonize".to_string(),
    }
}

/// Draws the floating ship details panel when a ship is selected.
/// #53: Simplified - command buttons moved to context menu
/// #62: Detailed status display with progress bars and command queue
/// #64: Shows home port info and "Set Home Port" button
#[allow(clippy::too_many_arguments)]
pub fn draw_ship_panel(
    ctx: &egui::Context,
    selected_ship: &mut SelectedShip,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    clock: &GameClock,
    colonies: &mut Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    command_queues: &Query<&mut CommandQueue>,
    planets: &Query<&Planet>,
) -> Option<ShipScrapAction> {
    // Collect ship data into locals first, then draw UI, then apply mutations
    let ship_data = selected_ship.0.and_then(|ship_entity| {
        let (_, ship, state, cargo) = ships_query.get(ship_entity).ok()?;
        let docked_system = if let ShipState::Docked { system } = &*state {
            Some(*system)
        } else {
            None
        };
        let cargo_data = cargo.map(|c| (c.minerals, c.energy));
        let status_info = build_status_info(&state, clock, stars);
        let queued_cmds: Vec<String> = command_queues
            .get(ship_entity)
            .ok()
            .map(|q| {
                q.commands
                    .iter()
                    .map(|cmd| format_queued_command(cmd, stars))
                    .collect()
            })
            .unwrap_or_default();
        let home_port = ship.home_port;
        let home_port_name = stars
            .get(home_port)
            .map(|(_, s, _, _)| s.name.clone())
            .unwrap_or_else(|_| "Unknown".to_string());
        let maintenance_cost = crate::ship::ship_maintenance_cost(&ship.design_id);
        // Check if docked at a system that has a colony (for "Set Home Port" button)
        let docked_at_colony = docked_system.and_then(|dock_sys| {
            colonies.iter().find_map(|(_, col, _, _, _, _, _, _, _)| {
                if col.system(planets) == Some(dock_sys) { Some(dock_sys) } else { None }
            })
        });
        Some((
            ship_entity,
            ship.name.clone(),
            ship.design_id.clone(),
            ship.hp,
            ship.max_hp,
            ship.ftl_range,
            ship.sublight_speed,
            status_info,
            docked_system,
            cargo_data,
            queued_cmds,
            home_port,
            home_port_name,
            maintenance_cost,
            docked_at_colony,
        ))
    });

    let Some((
        ship_entity,
        name,
        design_id,
        hp,
        max_hp,
        ftl_range,
        sublight_speed,
        status_info,
        docked_system,
        cargo_data,
        queued_cmds,
        _home_port_entity,
        home_port_name,
        maintenance_cost,
        docked_at_colony,
    )) = ship_data
    else {
        return None;
    };

    let mut deselect_ship = false;
    let mut set_home_port: Option<Entity> = None;
    let mut scrap_action: Option<ShipScrapAction> = None;

    // Cargo load/unload actions to apply after UI drawing
    #[derive(Default)]
    struct CargoAction {
        load_minerals: crate::amount::Amt,
        load_energy: crate::amount::Amt,
        unload_minerals: crate::amount::Amt,
        unload_energy: crate::amount::Amt,
    }
    let mut cargo_action = CargoAction::default();
    // Entity of the colony at the docked system (for cargo transfers)
    let colony_entity_at_dock: Option<Entity> = docked_system.and_then(|dock_sys| {
        colonies.iter().find_map(|(e, col, _, _, _, _, _, _, _)| {
            if col.system(planets) == Some(dock_sys) { Some(e) } else { None }
        })
    });

    egui::Window::new("Selected Ship")
        .anchor(egui::Align2::RIGHT_BOTTOM, [-270.0, -130.0])
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("Ship: {}", name))
                    .strong()
                    .color(egui::Color32::from_rgb(100, 200, 255)),
            );
            let design_display_name = crate::ship::design_preset(&design_id).map(|p| p.design_name).unwrap_or(&design_id);
            ui.label(format!("Type: {}", design_display_name));
            ui.label(format!("HP: {:.0}/{:.0}", hp, max_hp));

            // #62: Detailed status with progress bar
            ui.label(&status_info.label);
            if let Some((elapsed, total, fraction)) = status_info.progress {
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .text(format!("{}/{} hd", elapsed, total))
                        .desired_width(200.0),
                );
            }

            if ftl_range > 0.0 {
                ui.label(format!("FTL range: {:.1} ly", ftl_range));
            } else {
                ui.label("No FTL capability");
            }
            ui.label(format!(
                "Sub-light speed: {:.0}% c",
                sublight_speed * 100.0
            ));

            // #64: Home port and maintenance info
            ui.separator();
            ui.label(format!("Home Port: {}", home_port_name));
            ui.label(format!(
                "Maintenance: {:.1} E/hd (charged to {})",
                maintenance_cost, home_port_name
            ));

            // #62: Command queue display
            if !queued_cmds.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Command Queue").strong());
                for (i, cmd_str) in queued_cmds.iter().enumerate() {
                    ui.label(format!("  {}. {}", i + 1, cmd_str));
                }
            }

            ui.label(
                egui::RichText::new("Click a star to issue commands")
                    .weak()
                    .italics(),
            );

            // Cargo section for Courier ships docked at a colony
            if let Some(_docked_system) = docked_system {
                if design_id == "courier_mk1" {
                    if let Some((cargo_m, cargo_e)) = cargo_data {
                        ui.separator();
                        ui.label(egui::RichText::new("Cargo").strong());
                        ui.label(format!("Minerals: {}", cargo_m));
                        ui.label(format!("Energy: {}", cargo_e));

                        if colony_entity_at_dock.is_some() {
                            ui.horizontal(|ui| {
                                if ui.button("Load M +100").clicked() {
                                    cargo_action.load_minerals = crate::amount::Amt::units(100);
                                }
                                if ui.button("Load E +100").clicked() {
                                    cargo_action.load_energy = crate::amount::Amt::units(100);
                                }
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Unload M").clicked() {
                                    cargo_action.unload_minerals = cargo_m;
                                }
                                if ui.button("Unload E").clicked() {
                                    cargo_action.unload_energy = cargo_e;
                                }
                            });
                        }
                    }
                }
            }

            // #64: Set Home Port button (only when docked at a colony)
            if let Some(dock_system) = docked_at_colony {
                if ui.button("Set Home Port").clicked() {
                    set_home_port = Some(dock_system);
                }
            }

            // #79: Scrap Ship button (only when docked at a colony)
            if let Some(dock_system) = docked_at_colony {
                let (refund_m, refund_e) = crate::ship::ship_scrap_refund(&design_id);
                let scrap_label = format!("Scrap Ship (+{} M, +{} E)", refund_m, refund_e);
                let response = ui.button(&scrap_label)
                    .on_hover_text("Dismantle this ship and recover 50% of build cost");
                if response.clicked() {
                    // Find colony entity at dock system
                    if let Some(colony_e) = colony_entity_at_dock {
                        let system_name = stars
                            .get(dock_system)
                            .map(|(_, s, _, _)| s.name.clone())
                            .unwrap_or_else(|_| "Unknown".to_string());
                        scrap_action = Some(ShipScrapAction {
                            ship_entity,
                            colony_entity: colony_e,
                            ship_name: name.clone(),
                            system_name,
                            minerals_refund: refund_m,
                            energy_refund: refund_e,
                        });
                    }
                }
            }

            if ui.button("Deselect ship").clicked() {
                deselect_ship = true;
            }
        });

    // Apply deselect
    if deselect_ship {
        selected_ship.0 = None;
    }

    // #64: Apply home port change
    if let Some(new_home_port) = set_home_port {
        if let Ok((_, mut ship, _, _)) = ships_query.get_mut(ship_entity) {
            ship.home_port = new_home_port;
        }
    }

    // Apply cargo load/unload actions
    let has_cargo_action = cargo_action.load_minerals > Amt::ZERO
        || cargo_action.load_energy > Amt::ZERO
        || cargo_action.unload_minerals > Amt::ZERO
        || cargo_action.unload_energy > Amt::ZERO;
    if has_cargo_action {
        if let Some(colony_e) = colony_entity_at_dock {
            if let Ok((_, _, _, Some(mut stockpile), _, _, _, _, _)) = colonies.get_mut(colony_e) {
                if let Ok((_, _, _, Some(mut cargo))) = ships_query.get_mut(ship_entity) {
                    if cargo_action.load_minerals > Amt::ZERO {
                        let transfer = cargo_action.load_minerals.min(stockpile.minerals);
                        stockpile.minerals = stockpile.minerals.sub(transfer);
                        cargo.minerals = cargo.minerals.add(transfer);
                    }
                    if cargo_action.load_energy > Amt::ZERO {
                        let transfer = cargo_action.load_energy.min(stockpile.energy);
                        stockpile.energy = stockpile.energy.sub(transfer);
                        cargo.energy = cargo.energy.add(transfer);
                    }
                    if cargo_action.unload_minerals > Amt::ZERO {
                        let transfer = cargo_action.unload_minerals.min(cargo.minerals);
                        cargo.minerals = cargo.minerals.sub(transfer);
                        stockpile.minerals = stockpile.minerals.add(transfer);
                    }
                    if cargo_action.unload_energy > Amt::ZERO {
                        let transfer = cargo_action.unload_energy.min(cargo.energy);
                        cargo.energy = cargo.energy.sub(transfer);
                        stockpile.energy = stockpile.energy.add(transfer);
                    }
                }
            }
        }
    }

    // If scrapping, clear selection (despawn handled in draw_all_ui)
    if scrap_action.is_some() {
        selected_ship.0 = None;
    }

    scrap_action
}

/// Draws the RTS-style context menu when a ship is selected and a star is clicked.
/// #76: Commands are delayed by light-speed distance from player to ship.
#[allow(clippy::too_many_arguments)]
pub fn draw_context_menu(
    ctx: &egui::Context,
    context_menu: &mut crate::visualization::ContextMenu,
    selected_ship: &mut SelectedShip,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    command_queues: &mut Query<&mut CommandQueue>,
    positions: &Query<&Position>,
    clock: &GameClock,
    global_params: &GlobalParams,
    player_q: &Query<&StationedAt, With<Player>>,
    pending_commands_out: &mut Vec<crate::ship::PendingShipCommand>,
    colonies: &[Colony],
    planets: &Query<&Planet>,
) {
    if !context_menu.open {
        return;
    }

    let Some(ship_entity) = selected_ship.0 else {
        context_menu.open = false;
        return;
    };

    let Some(target_entity) = context_menu.target_system else {
        context_menu.open = false;
        return;
    };

    // Collect ship data
    let ship_data = {
        let Ok((_, ship, state, _)) = ships_query.get(ship_entity) else {
            context_menu.open = false;
            return;
        };
        let docked_system = if let ShipState::Docked { system } = &*state {
            Some(*system)
        } else {
            None
        };
        // For non-docked ships, determine origin position from current state
        let current_destination_system = match &*state {
            ShipState::SubLight { target_system, .. } => *target_system,
            ShipState::InFTL { destination_system, .. } => Some(*destination_system),
            ShipState::Surveying { target_system, .. } => Some(*target_system),
            ShipState::Settling { system, .. } => Some(*system),
            ShipState::Docked { .. } => None, // handled via docked_system
        };
        (
            ship.name.clone(),
            ship.design_id.clone(),
            ship.ftl_range,
            ship.sublight_speed,
            docked_system,
            current_destination_system,
        )
    };

    let (ship_name, design_id, ftl_range, sublight_speed, docked_system, current_destination_system) = ship_data;

    let is_docked = docked_system.is_some();

    // For docked ships, the origin is the docked system.
    // For non-docked ships, the origin is their current destination (where they'll end up).
    let origin_system = if let Some(ds) = docked_system {
        Some(ds)
    } else {
        current_destination_system
    };

    let Some(origin_system) = origin_system else {
        // No origin determinable; close menu
        context_menu.open = false;
        return;
    };

    let same_system = is_docked && target_entity == origin_system;

    // #76: Calculate light-speed delay from player to ship
    let command_delay: i64 = player_q
        .single()
        .ok()
        .and_then(|stationed| {
            let player_pos = positions.get(stationed.system).ok()?;
            let ship_pos = positions.get(origin_system).ok()?;
            let dist = physics::distance_ly(player_pos, ship_pos);
            Some(physics::light_delay_hexadies(dist))
        })
        .unwrap_or(0);

    // Collect target star data
    let Ok((_, target_star, target_pos, target_attrs)) = stars.get(target_entity) else {
        context_menu.open = false;
        return;
    };
    let Ok(origin_pos) = positions.get(origin_system) else {
        context_menu.open = false;
        return;
    };

    let dist = physics::distance_ly(origin_pos, target_pos);
    let target_name = target_star.name.clone();
    let target_surveyed = target_star.surveyed;
    let target_colonized = colonies.iter().any(|c| planets.get(c.planet).ok().map(|p| p.system) == Some(target_entity));
    let target_habitable = target_attrs
        .map(|a| {
            a.habitability != crate::galaxy::Habitability::Barren
                && a.habitability != crate::galaxy::Habitability::GasGiant
        })
        .unwrap_or(false);

    // #48: Use effective FTL range including tech bonuses
    let effective_ftl_range = ftl_range + global_params.ftl_range_bonus;
    let can_ftl = !same_system && effective_ftl_range > 0.0 && target_surveyed && dist <= effective_ftl_range;
    let can_move = !same_system;
    // Survey: can survey nearby unsurveyed system (including from docked at same system if unsurveyed)
    let can_survey = is_docked && crate::ship::design_can_survey(&design_id) && !target_surveyed;
    let can_colonize = is_docked && crate::ship::design_can_colonize(&design_id) && target_habitable && !target_colonized && target_surveyed && same_system;

    let origin_pos_arr = origin_pos.as_array();
    let target_pos_arr = target_pos.as_array();

    let mut command: Option<ShipState> = None;
    let mut queued_command: Option<QueuedCommand> = None;
    // #76: Delayed command for remote ships (light-speed delay > 0)
    let mut delayed_command: Option<crate::ship::ShipCommand> = None;
    let mut close_menu = false;

    // No actions available at all? Close and bail
    if !can_move && !can_ftl && !can_survey && !can_colonize {
        context_menu.open = false;
        return;
    }

    // Shift+click: execute default action immediately without showing menu
    if context_menu.execute_default {
        if is_docked && same_system {
            // Same system: default is survey or colonize
            if can_survey {
                if command_delay == 0 {
                    command = Some(ShipState::Surveying {
                        target_system: target_entity,
                        started_at: clock.elapsed,
                        completes_at: clock.elapsed + crate::ship::SURVEY_DURATION_HEXADIES,
                    });
                } else {
                    delayed_command = Some(crate::ship::ShipCommand::Survey { target: target_entity });
                }
            } else if can_colonize {
                if command_delay == 0 {
                    command = Some(ShipState::Settling {
                        system: target_entity,
                        started_at: clock.elapsed,
                        completes_at: clock.elapsed + crate::ship::SETTLING_DURATION_HEXADIES,
                    });
                } else {
                    delayed_command = Some(crate::ship::ShipCommand::Colonize);
                }
            }
            context_menu.open = false;
            context_menu.target_system = None;
            context_menu.execute_default = false;
            if let Some(new_state) = command {
                if let Ok((_, _, mut state, _)) = ships_query.get_mut(ship_entity) {
                    *state = new_state;
                }
            }
            if let Some(ship_cmd) = delayed_command {
                info!("Command sent to {} (arrives in {} hd)", ship_name, command_delay);
                pending_commands_out.push(crate::ship::PendingShipCommand {
                    ship: ship_entity,
                    command: ship_cmd,
                    arrives_at: clock.elapsed + command_delay,
                });
            }
            return;
        } else if is_docked {
            if can_ftl {
                if command_delay == 0 {
                    let effective_ftl_speed = crate::ship::INITIAL_FTL_SPEED_C * global_params.ftl_speed_multiplier;
                    let travel_time = (dist * crate::time_system::HEXADIES_PER_YEAR as f64 / effective_ftl_speed).ceil() as i64;
                    let travel_time = travel_time.max(1);
                    command = Some(ShipState::InFTL {
                        origin_system,
                        destination_system: target_entity,
                        departed_at: clock.elapsed,
                        arrival_at: clock.elapsed + travel_time,
                    });
                } else {
                    delayed_command = Some(crate::ship::ShipCommand::FTLTo { destination: target_entity });
                }
            } else {
                if command_delay == 0 {
                    let travel_time = physics::sublight_travel_hexadies(dist, sublight_speed);
                    command = Some(ShipState::SubLight {
                        origin: origin_pos_arr,
                        destination: target_pos_arr,
                        target_system: Some(target_entity),
                        departed_at: clock.elapsed,
                        arrival_at: clock.elapsed + travel_time,
                    });
                } else {
                    delayed_command = Some(crate::ship::ShipCommand::SubLightTo { destination: target_entity });
                }
            }
        } else {
            // Non-docked: queue the default action
            if can_ftl {
                queued_command = Some(QueuedCommand::FTLTo {
                    system: target_entity,
                    expected_position: origin_pos_arr,
                });
            } else {
                queued_command = Some(QueuedCommand::MoveTo {
                    system: target_entity,
                    expected_position: origin_pos_arr,
                });
            }
        }
        context_menu.open = false;
        context_menu.target_system = None;
        context_menu.execute_default = false;

        if let Some(new_state) = command {
            if let Ok((_, _, mut state, _)) = ships_query.get_mut(ship_entity) {
                *state = new_state;
                selected_ship.0 = None;
            }
        }
        if let Some(ship_cmd) = delayed_command {
            info!("Command sent to {} (arrives in {} hd)", ship_name, command_delay);
            pending_commands_out.push(crate::ship::PendingShipCommand {
                ship: ship_entity,
                command: ship_cmd,
                arrives_at: clock.elapsed + command_delay,
            });
            selected_ship.0 = None;
        }
        if let Some(qc) = queued_command {
            if let Ok(mut queue) = command_queues.get_mut(ship_entity) {
                queue.commands.push(qc);
                selected_ship.0 = None;
            }
        }
        return;
    }

    let menu_pos = egui::pos2(context_menu.position[0], context_menu.position[1]);
    let queue_prefix = if is_docked { "" } else { "Queue: " };

    egui::Window::new("Ship Commands")
        .fixed_pos(menu_pos)
        .resizable(false)
        .collapsible(false)
        .title_bar(false)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("{} -> {}", ship_name, target_name))
                    .strong(),
            );
            ui.label(format!("Distance: {:.1} ly", dist));
            // #76: Show command delay if player is remote
            if command_delay > 0 {
                ui.label(
                    egui::RichText::new(format!("Command delay: {} hd", command_delay))
                        .color(egui::Color32::from_rgb(255, 200, 100)),
                );
            }
            if !is_docked {
                ui.label(
                    egui::RichText::new("(commands will be queued)")
                        .weak()
                        .italics(),
                );
            }
            ui.separator();

            // Move (Sub-light) -- available when targeting a different system
            if can_move && ui.button(format!("{}Move (Sub-light)", queue_prefix)).clicked() {
                if is_docked {
                    if command_delay == 0 {
                        let travel_time = physics::sublight_travel_hexadies(dist, sublight_speed);
                        command = Some(ShipState::SubLight {
                            origin: origin_pos_arr,
                            destination: target_pos_arr,
                            target_system: Some(target_entity),
                            departed_at: clock.elapsed,
                            arrival_at: clock.elapsed + travel_time,
                        });
                    } else {
                        delayed_command = Some(crate::ship::ShipCommand::SubLightTo { destination: target_entity });
                    }
                } else {
                    queued_command = Some(QueuedCommand::MoveTo {
                        system: target_entity,
                        expected_position: origin_pos_arr,
                    });
                }
                close_menu = true;
            }

            // FTL Jump -- if ship has FTL + target surveyed + in range
            if can_ftl {
                if ui.button(format!("{}FTL Jump", queue_prefix)).clicked() {
                    if is_docked {
                        if command_delay == 0 {
                            let effective_ftl_speed = crate::ship::INITIAL_FTL_SPEED_C * global_params.ftl_speed_multiplier;
                            let travel_time = (dist * crate::time_system::HEXADIES_PER_YEAR as f64 / effective_ftl_speed).ceil() as i64;
                            let travel_time = travel_time.max(1);
                            command = Some(ShipState::InFTL {
                                origin_system,
                                destination_system: target_entity,
                                departed_at: clock.elapsed,
                                arrival_at: clock.elapsed + travel_time,
                            });
                        } else {
                            delayed_command = Some(crate::ship::ShipCommand::FTLTo { destination: target_entity });
                        }
                    } else {
                        queued_command = Some(QueuedCommand::FTLTo {
                            system: target_entity,
                            expected_position: origin_pos_arr,
                        });
                    }
                    close_menu = true;
                }
            }

            // Survey -- if Explorer + target unsurveyed (docked only)
            if can_survey {
                if ui.button("Survey").clicked() {
                    if command_delay == 0 {
                        let survey_time = physics::light_delay_hexadies(dist) * 2 + 5;
                        command = Some(ShipState::Surveying {
                            target_system: target_entity,
                            started_at: clock.elapsed,
                            completes_at: clock.elapsed + survey_time,
                        });
                    } else {
                        delayed_command = Some(crate::ship::ShipCommand::Survey { target: target_entity });
                    }
                    close_menu = true;
                }
            }

            // Colonize -- if ColonyShip + target habitable + uncolonized (docked only)
            if can_colonize {
                if ui.button("Colonize").clicked() {
                    if command_delay == 0 {
                        command = Some(ShipState::Settling {
                            system: target_entity,
                            started_at: clock.elapsed,
                            completes_at: clock.elapsed + crate::ship::SETTLING_DURATION_HEXADIES,
                        });
                    } else {
                        delayed_command = Some(crate::ship::ShipCommand::Colonize);
                    }
                    close_menu = true;
                }
            }

            ui.separator();
            if ui.button("Cancel").clicked() {
                close_menu = true;
            }
        });

    if close_menu {
        context_menu.open = false;
        context_menu.target_system = None;
    }

    // Apply immediate command (docked ships, no delay)
    if let Some(new_state) = command {
        if let Ok((_, _, mut state, _)) = ships_query.get_mut(ship_entity) {
            *state = new_state;
            selected_ship.0 = None;
        }
    }

    // #76: Apply delayed command (docked ships, light-speed delay > 0)
    if let Some(ship_cmd) = delayed_command {
        info!("Command sent to {} (arrives in {} hd)", ship_name, command_delay);
        pending_commands_out.push(crate::ship::PendingShipCommand {
            ship: ship_entity,
            command: ship_cmd,
            arrives_at: clock.elapsed + command_delay,
        });
        selected_ship.0 = None;
    }

    // Apply queued command (non-docked ships)
    if let Some(qc) = queued_command {
        if let Ok(mut queue) = command_queues.get_mut(ship_entity) {
            queue.commands.push(qc);
            selected_ship.0 = None;
        }
    }
}

/// Helper to collect ships docked at a given system.
fn ships_docked_at(
    system: Entity,
    ships: &Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
) -> Vec<(Entity, String, String)> {
    let mut result: Vec<(Entity, String, String)> = ships
        .iter()
        .filter_map(|(e, ship, state, _)| {
            if let ShipState::Docked { system: s } = &*state {
                if *s == system {
                    return Some((e, ship.name.clone(), ship.design_id.clone()));
                }
            }
            None
        })
        .collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    result
}
