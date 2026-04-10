use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildOrder, BuildQueue, BuildingOrder, BuildingQueue, BuildingType, Buildings, Colony, ColonizationQueue, ConstructionParams, DemolitionOrder, FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile, SystemBuildings, SystemBuildingQueue};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{AboardShip, Player, StationedAt};
use crate::amount::Amt;
use crate::ship::{Cargo, CommandQueue, PendingShipCommand, QueuedCommand, RulesOfEngagement, Ship, ShipHitpoints, ShipState, SurveyData};
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

/// Action for starting a refit on a docked ship.
pub struct ShipRefitAction {
    pub ship_entity: Entity,
    pub system_entity: Entity,
    pub new_modules: Vec<crate::ship::EquippedModule>,
    pub refit_time: i64,
    pub cost_minerals: Amt,
    pub cost_energy: Amt,
}

/// All actions that can be triggered from the ship panel UI.
/// Processed in draw_all_ui where mutable access is available.
#[derive(Default)]
pub struct ShipPanelActions {
    pub scrap: Option<ShipScrapAction>,
    pub cancel_command_index: Option<usize>,
    pub clear_commands: bool,
    pub cancel_current: bool,
    pub refit: Option<ShipRefitAction>,
    /// #57: ROE change action — (ship_entity, new_roe, command_delay)
    pub set_roe: Option<(Entity, RulesOfEngagement, i64)>,
    /// #59: Player wants to board the selected ship
    pub board_ship: Option<Entity>,
    /// #59: Player wants to disembark from the selected ship
    pub disembark: bool,
}

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
        .filter(|(_, c, _, _, _, _, _, _)| c.system(planets) == Some(sel_entity))
        .map(|(_, c, _, _, _, _, _, _)| c.planet)
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

    let screen = ctx.screen_rect();
    let mut close_system_view = false;
    egui::Window::new(format!("{} ({})", star.name, format_star_type(&star.star_type)))
        .fixed_pos(egui::pos2(0.0, 30.0))
        .fixed_size(egui::vec2(screen.width(), screen.height() - 60.0))
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
                                sel_entity,
                                planet_attrs,
                                colonies,
                                system_stockpiles,
                                ships_query,
                                construction_params,
                                planets,
                                hull_registry,
                                module_registry,
                                design_registry,
                            );
                        });
                }
            }

            // === System Buildings ===
            if let Ok((Some(sys_bldgs), sys_bldg_queue)) = system_buildings_q.get_mut(sel_entity) {
                ui.separator();
                ui.label(egui::RichText::new("System Buildings").strong());

                let mut sys_demolish_request: Option<(usize, BuildingType)> = None;

                for (i, slot) in sys_bldgs.slots.iter().enumerate() {
                    let is_demolishing = sys_bldg_queue
                        .as_ref()
                        .map(|bq| bq.is_demolishing(i))
                        .unwrap_or(false);

                    match slot {
                        Some(bt) if is_demolishing => {
                            let remaining = sys_bldg_queue
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
                                    sys_demolish_request = Some((i, *bt));
                                }
                            });
                        }
                        None => {
                            ui.label(format!("  [{}] (empty)", i));
                        }
                    }
                }

                if let Some((slot_idx, bt)) = sys_demolish_request {
                    if let Some(mut bq) = sys_bldg_queue {
                        let (m_refund, e_refund) = bt.demolition_refund();
                        bq.demolition_queue.push(DemolitionOrder {
                            target_slot: slot_idx,
                            building_type: bt,
                            time_remaining: bt.demolition_time(),
                            minerals_refund: m_refund,
                            energy_refund: e_refund,
                        });
                        info!("System building demolition order added: {:?} in slot {}", bt, slot_idx);
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
                        let system_building_types = [
                            BuildingType::Shipyard,
                            BuildingType::ResearchLab,
                            BuildingType::Port,
                        ];
                        let bldg_cost_mod = construction_params.building_cost_modifier.final_value();
                        let bldg_time_mod = construction_params.building_build_time_modifier.final_value();
                        let mut build_sys_building_request: Option<BuildingType> = None;
                        for bt in &system_building_types {
                            let (base_m, base_e) = bt.build_cost();
                            let eff_m = base_m.mul_amt(bldg_cost_mod);
                            let eff_e = base_e.mul_amt(bldg_cost_mod);
                            let eff_time = (bt.build_time() as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                            let tooltip = format!("M:{} E:{} | {} hexadies", eff_m, eff_e, eff_time);
                            if ui.button(bt.name()).on_hover_text(tooltip).clicked() {
                                build_sys_building_request = Some(*bt);
                            }
                        }
                        if let Some(bt) = build_sys_building_request {
                            if let Ok((_, Some(mut bq))) = system_buildings_q.get_mut(sel_entity) {
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
                                info!("System building order added: {:?} in slot {}", bt, slot_idx);
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
    if close_system_view {
        selected_system.0 = None;
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
                    if let Some(bt) = slot {
                        building_maintenance = building_maintenance.add(bt.maintenance_cost());
                    }
                }
            }
            let mut ship_maintenance = Amt::ZERO;
            let mut ships_based_here = 0u32;
            for (_, ship, _, _, _, _) in ships_query.iter() {
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

        // Build buttons - add orders to the queue
        // Uses ShipDesignRegistry when available, falls back to presets
        if let Some(mut bq) = build_queue {
            use crate::amount::Amt;
            let ship_mod = construction_params.ship_cost_modifier.final_value();
            let ship_time_mod = construction_params.ship_build_time_modifier.final_value();
            let mut build_request: Option<(String, String, Amt, Amt, i64)> = None;

            // Collect designs: from registry first, then fallback to presets
            let has_registry_designs = !design_registry.designs.is_empty();

            if has_registry_designs {
                let mut design_ids: Vec<_> = design_registry.designs.keys().cloned().collect();
                design_ids.sort();

                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for design_id in &design_ids {
                            let design = &design_registry.designs[design_id];
                            // Calculate cost from hull + modules
                            let hull = hull_registry.get(&design.hull_id);
                            let (base_m, base_e, base_time) = if let Some(hull) = hull {
                                let mods: Vec<_> = design.modules.iter()
                                    .filter_map(|a| module_registry.get(&a.module_id))
                                    .collect();
                                let (m, e, t, _maint) = crate::ship_design::design_cost(hull, &mods);
                                (m, e, t)
                            } else {
                                // Fallback to preset if hull not in registry
                                crate::ship::design_preset(design_id)
                                    .map(|p| (p.build_cost_minerals, p.build_cost_energy, p.build_time))
                                    .unwrap_or((Amt::units(200), Amt::units(100), 60))
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
            } else {
                // Fallback: use hardcoded presets
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
                            build_request = Some((preset.design_id.to_string(), preset.design_name.to_string(), eff_m, eff_e, eff_time));
                        }
                    }
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

            let mut demolish_request: Option<(usize, BuildingType)> = None;

            // Collect pending building slots so we can show in-progress orders
            let pending_orders: Vec<(usize, &str, f32)> = building_queue
                .as_ref()
                .map(|bq| {
                    bq.queue
                        .iter()
                        .map(|order| {
                            let (total_m, total_e) = order.building_type.build_cost();
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
                            let bt_time = order.building_type.build_time();
                            let time_pct = if bt_time > 0 {
                                1.0 - (order.build_time_remaining as f32 / bt_time as f32)
                            } else {
                                1.0
                            };
                            let pct = m_pct.min(e_pct).min(time_pct).max(0.0);
                            (order.target_slot, order.building_type.name(), pct)
                        })
                        .collect()
                })
                .unwrap_or_default();

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

            let pending_slots: Vec<usize> = pending_orders.iter().map(|(s, _, _)| *s).collect();
            let empty_slot = buildings
                .slots
                .iter()
                .enumerate()
                .position(|(i, s)| s.is_none() && !pending_slots.contains(&i));

            if let Some(slot_idx) = empty_slot {
                ui.separator();
                ui.label(egui::RichText::new("Build Planet Building").strong());
                let building_types = [
                    BuildingType::Mine,
                    BuildingType::PowerPlant,
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
            ..
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
        ShipState::Refitting {
            system,
            started_at,
            completes_at,
            ..
        } => {
            let total = (completes_at - started_at).max(1);
            let elapsed = (clock.elapsed - started_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "Refitting at {} ({}/{} hd, {:.0}%)",
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
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    clock: &GameClock,
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
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    command_queues: &Query<&mut CommandQueue>,
    planets: &Query<&Planet>,
    pending_commands: &Query<&PendingShipCommand>,
    hull_registry: &crate::ship_design::HullRegistry,
    module_registry: &crate::ship_design::ModuleRegistry,
    clock_elapsed: i64,
    roe_query: &Query<&RulesOfEngagement>,
    positions: &Query<&Position>,
    player_stationed: Option<Entity>,
    player_aboard_ship: Option<Entity>,
) -> ShipPanelActions {
    // Collect ship data into locals first, then draw UI, then apply mutations
    let ship_data = selected_ship.0.and_then(|ship_entity| {
        let (_, ship, state, cargo, ship_hp, survey_data) = ships_query.get(ship_entity).ok()?;
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
            colonies.iter().find_map(|(_, col, _, _, _, _, _, _)| {
                if col.system(planets) == Some(dock_sys) { Some(dock_sys) } else { None }
            })
        });
        // Check if ship is in a cancellable state (surveying or settling)
        let is_cancellable = matches!(&*state, ShipState::Surveying { .. } | ShipState::Settling { .. });
        // #103: Check if ship carries unreported survey data
        let has_survey_data = survey_data.is_some();
        let survey_data_system = survey_data.map(|sd| sd.system_name.clone());
        // Collect pending commands for this ship
        let pending_info: Option<i64> = pending_commands.iter()
            .filter(|pc| pc.ship == ship_entity)
            .map(|pc| pc.arrives_at)
            .min();
        // #98: Collect hull_id and modules for refit UI
        let ship_hull_id = ship.hull_id.clone();
        let ship_modules: Vec<crate::ship::EquippedModule> = ship.modules.clone();
        // #98: Is the ship refitting?
        let is_refitting = matches!(&*state, ShipState::Refitting { .. });
        // #57: Current ROE
        let current_roe = roe_query.get(ship_entity).copied().unwrap_or_default();
        // #57: Command delay for ROE changes
        let roe_command_delay: i64 = {
            // Determine the system the ship is at (or heading to)
            let ship_system = docked_system.or_else(|| {
                match &*state {
                    ShipState::SubLight { target_system, .. } => *target_system,
                    ShipState::InFTL { destination_system, .. } => Some(*destination_system),
                    ShipState::Surveying { target_system, .. } => Some(*target_system),
                    ShipState::Settling { system, .. } => Some(*system),
                    _ => None,
                }
            });
            player_stationed
                .and_then(|player_sys| {
                    let player_pos = positions.get(player_sys).ok()?;
                    let ship_sys = ship_system?;
                    let ship_pos = positions.get(ship_sys).ok()?;
                    let dist = crate::physics::distance_ly(player_pos, ship_pos);
                    Some(crate::physics::light_delay_hexadies(dist))
                })
                .unwrap_or(0)
        };
        // #59: Player aboard this ship?
        let is_player_aboard = ship.player_aboard;
        // #59: Can player board this ship? (ship docked at player's system, player not aboard any ship)
        let can_board = !is_player_aboard
            && player_aboard_ship.is_none()
            && docked_system.is_some()
            && docked_system == player_stationed;
        // #59: Can player disembark? (player aboard this ship and ship is docked)
        let can_disembark = is_player_aboard && docked_system.is_some();
        Some((
            ship_entity,
            ship.name.clone(),
            ship.design_id.clone(),
            ship_hp.hull,
            ship_hp.hull_max,
            ship_hp.armor,
            ship_hp.armor_max,
            ship_hp.shield,
            ship_hp.shield_max,
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
            is_cancellable,
            pending_info,
            has_survey_data,
            survey_data_system,
            ship_hull_id,
            ship_modules,
            is_refitting,
            current_roe,
            roe_command_delay,
            is_player_aboard,
            can_board,
            can_disembark,
        ))
    });

    let Some((
        ship_entity,
        name,
        design_id,
        hull_hp,
        hull_max,
        armor,
        armor_max,
        shield,
        shield_max,
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
        is_cancellable,
        pending_arrives_at,
        has_survey_data,
        survey_data_system,
        ship_hull_id,
        ship_modules,
        is_refitting,
        current_roe,
        roe_command_delay,
        is_player_aboard,
        can_board,
        can_disembark,
    )) = ship_data
    else {
        return ShipPanelActions::default();
    };

    let mut deselect_ship = false;
    let mut set_home_port: Option<Entity> = None;
    let mut actions = ShipPanelActions::default();

    // Cargo load/unload actions to apply after UI drawing
    #[derive(Default)]
    struct CargoAction {
        load_minerals: crate::amount::Amt,
        load_energy: crate::amount::Amt,
        unload_minerals: crate::amount::Amt,
        unload_energy: crate::amount::Amt,
    }
    let mut cargo_action = CargoAction::default();
    // Entity of the system at the dock (for cargo transfers via system stockpile)
    let system_entity_at_dock: Option<Entity> = docked_system.and_then(|dock_sys| {
        // Check if there's a colony at this system
        let has_colony = colonies.iter().any(|(_, col, _, _, _, _, _, _)| {
            col.system(planets) == Some(dock_sys)
        });
        if has_colony { Some(dock_sys) } else { None }
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
            // #59: Player aboard indicator
            if is_player_aboard {
                ui.label(
                    egui::RichText::new("[Player Aboard]")
                        .color(egui::Color32::from_rgb(50, 255, 50))
                        .strong(),
                );
            }
            ui.label(format!("Hull: {:.0}/{:.0}", hull_hp, hull_max));
            if armor_max > 0.0 {
                ui.label(format!("Armor: {:.0}/{:.0}", armor, armor_max));
            }
            if shield_max > 0.0 {
                ui.label(format!("Shield: {:.0}/{:.0}", shield, shield_max));
            }

            // #62: Detailed status with progress bar
            ui.label(&status_info.label);
            if let Some((elapsed, total, fraction)) = status_info.progress {
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .text(format!("{}/{} hd", elapsed, total))
                        .desired_width(200.0),
                );
            }

            // #103: Show indicator if ship carries unreported survey data
            if has_survey_data {
                let sys_name = survey_data_system.as_deref().unwrap_or("unknown");
                ui.label(
                    egui::RichText::new(format!("Carrying survey data: {}", sys_name))
                        .color(egui::Color32::from_rgb(255, 200, 50)),
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

            // #57: Rules of Engagement selector
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("ROE:");
                for roe_option in RulesOfEngagement::ALL {
                    let is_selected = current_roe == roe_option;
                    let label = roe_option.label();
                    if ui.selectable_label(is_selected, label).clicked() && !is_selected {
                        actions.set_roe = Some((ship_entity, roe_option, roe_command_delay));
                    }
                }
            });
            if roe_command_delay > 0 {
                ui.label(
                    egui::RichText::new(format!("ROE change delay: {} hd", roe_command_delay))
                        .small()
                        .color(egui::Color32::from_rgb(255, 200, 100)),
                );
            }

            // #99: Pending command in transit display
            if let Some(arrives_at) = pending_arrives_at {
                let remaining = (arrives_at - clock.elapsed).max(0);
                ui.label(
                    egui::RichText::new(format!("Command in transit... arrives in {} hd", remaining))
                        .color(egui::Color32::from_rgb(255, 191, 0)),
                );
            }

            // #99: Cancel current action button (surveying/settling)
            if is_cancellable {
                if ui.button("Cancel Current Action").clicked() {
                    actions.cancel_current = true;
                }
            }

            // #62/#99: Command queue display with cancel buttons
            if !queued_cmds.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Command Queue").strong());
                for (i, cmd_str) in queued_cmds.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("X").clicked() {
                            actions.cancel_command_index = Some(i);
                        }
                        ui.label(format!("{}. {}", i + 1, cmd_str));
                    });
                }
                if ui.button("Clear All").clicked() {
                    actions.clear_commands = true;
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

                        if system_entity_at_dock.is_some() {
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

            // #59: Board / Disembark buttons
            if can_board {
                if ui.button("Board Ship").clicked() {
                    actions.board_ship = Some(ship_entity);
                }
            }
            if can_disembark {
                if ui.button("Disembark").clicked() {
                    actions.disembark = true;
                }
            }

            // #98: Refit UI (only when docked at a colony and not already refitting)
            if let Some(dock_system) = docked_at_colony {
                if !is_refitting {
                    if let Some(hull_def) = hull_registry.get(&ship_hull_id) {
                        ui.separator();
                        ui.label(egui::RichText::new("Refit").strong());

                        // Use egui temp memory to track refit module selections
                        let refit_id = egui::Id::new(("refit_modules", ship_entity));
                        let mut refit_selections: Vec<Option<String>> = ui
                            .memory(|m| m.data.get_temp(refit_id))
                            .unwrap_or_else(|| {
                                // Initialize from current modules
                                let mut selections = Vec::new();
                                let mut mod_idx = 0;
                                for hull_slot in &hull_def.slots {
                                    for _ in 0..hull_slot.count {
                                        let current = ship_modules.get(mod_idx)
                                            .filter(|em| em.slot_type == hull_slot.slot_type)
                                            .map(|em| em.module_id.clone());
                                        selections.push(current);
                                        mod_idx += 1;
                                    }
                                }
                                selections
                            });

                        let mut slot_idx = 0;
                        for hull_slot in &hull_def.slots {
                            for i in 0..hull_slot.count {
                                let slot_label = if hull_slot.count > 1 {
                                    format!("[{}] {}_{}", hull_slot.slot_type.chars().next().unwrap_or('?').to_uppercase(), hull_slot.slot_type, i + 1)
                                } else {
                                    format!("[{}] {}", hull_slot.slot_type.chars().next().unwrap_or('?').to_uppercase(), hull_slot.slot_type)
                                };

                                let current_name = refit_selections
                                    .get(slot_idx)
                                    .and_then(|opt| opt.as_ref())
                                    .and_then(|id| module_registry.get(id))
                                    .map(|m| m.name.clone())
                                    .unwrap_or_else(|| "(empty)".to_string());

                                ui.horizontal(|ui| {
                                    ui.label(&slot_label);
                                    let combo_id = format!("refit_slot_{}", slot_idx);
                                    egui::ComboBox::from_id_salt(combo_id)
                                        .selected_text(&current_name)
                                        .show_ui(ui, |ui| {
                                            if ui.selectable_label(
                                                refit_selections.get(slot_idx).and_then(|o| o.as_ref()).is_none(),
                                                "(empty)",
                                            ).clicked() {
                                                if slot_idx < refit_selections.len() {
                                                    refit_selections[slot_idx] = None;
                                                }
                                            }
                                            let mut mod_ids: Vec<_> = module_registry
                                                .modules.iter()
                                                .filter(|(_, m)| m.slot_type == hull_slot.slot_type)
                                                .map(|(id, _)| id.clone())
                                                .collect();
                                            mod_ids.sort();
                                            for mod_id in mod_ids {
                                                let module = &module_registry.modules[&mod_id];
                                                let is_selected = refit_selections
                                                    .get(slot_idx)
                                                    .and_then(|o| o.as_ref()) == Some(&mod_id);
                                                if ui.selectable_label(is_selected, &module.name).clicked() {
                                                    if slot_idx < refit_selections.len() {
                                                        refit_selections[slot_idx] = Some(mod_id.clone());
                                                    }
                                                }
                                            }
                                        });
                                });
                                slot_idx += 1;
                            }
                        }

                        // Calculate refit cost
                        let old_mods: Vec<_> = ship_modules.iter()
                            .filter_map(|em| module_registry.get(&em.module_id))
                            .collect();
                        let new_mods: Vec<_> = refit_selections.iter()
                            .filter_map(|opt| opt.as_ref())
                            .filter_map(|id| module_registry.get(id))
                            .collect();
                        let (refit_m, refit_e, refit_time) = crate::ship_design::refit_cost(&old_mods, &new_mods, hull_def);

                        ui.label(format!("Refit cost: M:{} E:{} | {} hd", refit_m, refit_e, refit_time));

                        // Check if any module actually changed
                        let mut has_changes = false;
                        {
                            let mut idx = 0;
                            for hull_slot in &hull_def.slots {
                                for _ in 0..hull_slot.count {
                                    let old_mod = ship_modules.get(idx)
                                        .filter(|em| em.slot_type == hull_slot.slot_type)
                                        .map(|em| &em.module_id);
                                    let new_mod = refit_selections.get(idx).and_then(|o| o.as_ref());
                                    if old_mod != new_mod {
                                        has_changes = true;
                                    }
                                    idx += 1;
                                }
                            }
                        }

                        if ui.add_enabled(has_changes, egui::Button::new("Apply Refit")).clicked() {
                            // Build new modules list
                            let mut new_modules = Vec::new();
                            let mut idx = 0;
                            for hull_slot in &hull_def.slots {
                                for _ in 0..hull_slot.count {
                                    if let Some(Some(mod_id)) = refit_selections.get(idx) {
                                        new_modules.push(crate::ship::EquippedModule {
                                            slot_type: hull_slot.slot_type.clone(),
                                            module_id: mod_id.clone(),
                                        });
                                    }
                                    idx += 1;
                                }
                            }
                            actions.refit = Some(ShipRefitAction {
                                ship_entity,
                                system_entity: dock_system,
                                new_modules,
                                refit_time,
                                cost_minerals: refit_m,
                                cost_energy: refit_e,
                            });
                        }

                        // Store selections in egui temp memory
                        ui.memory_mut(|m| m.data.insert_temp(refit_id, refit_selections));
                    }
                } else {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Refitting in progress...")
                            .color(egui::Color32::from_rgb(255, 220, 80)),
                    );
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
                    // Use system entity for stockpile refund
                    if let Some(sys_e) = system_entity_at_dock {
                        let system_name = stars
                            .get(dock_system)
                            .map(|(_, s, _, _)| s.name.clone())
                            .unwrap_or_else(|_| "Unknown".to_string());
                        actions.scrap = Some(ShipScrapAction {
                            ship_entity,
                            colony_entity: sys_e,
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
        if let Ok((_, mut ship, _, _, _, _)) = ships_query.get_mut(ship_entity) {
            ship.home_port = new_home_port;
        }
    }

    // Apply cargo load/unload actions
    let has_cargo_action = cargo_action.load_minerals > Amt::ZERO
        || cargo_action.load_energy > Amt::ZERO
        || cargo_action.unload_minerals > Amt::ZERO
        || cargo_action.unload_energy > Amt::ZERO;
    if has_cargo_action {
        if let Some(sys_e) = system_entity_at_dock {
            if let Ok((mut stockpile, _)) = system_stockpiles.get_mut(sys_e) {
                if let Ok((_, _, _, Some(mut cargo), _, _)) = ships_query.get_mut(ship_entity) {
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
    if actions.scrap.is_some() {
        selected_ship.0 = None;
    }

    actions
}

/// Draws the RTS-style context menu when a ship is selected and a star is clicked.
/// #76: Commands are delayed by light-speed distance from player to ship.
#[allow(clippy::too_many_arguments)]
pub fn draw_context_menu(
    ctx: &egui::Context,
    context_menu: &mut crate::visualization::ContextMenu,
    selected_ship: &mut SelectedShip,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    command_queues: &mut Query<&mut CommandQueue>,
    positions: &Query<&Position>,
    clock: &GameClock,
    global_params: &GlobalParams,
    player_q: &Query<(Entity, &StationedAt, Option<&AboardShip>), With<Player>>,
    pending_commands_out: &mut Vec<crate::ship::PendingShipCommand>,
    colonies: &[Colony],
    planets: &Query<&Planet>,
    planet_entities: &Query<(Entity, &Planet, Option<&SystemAttributes>)>,
    hostile_systems: &std::collections::HashSet<Entity>,
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
        let Ok((_, ship, state, _, _, _)) = ships_query.get(ship_entity) else {
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
            ShipState::Refitting { system, .. } => Some(*system),
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
        .and_then(|(_, stationed, _)| {
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

    // #114: Check for colonizable planets (habitable + uncolonized) in the target system
    let colonized_planets: std::collections::HashSet<Entity> = colonies.iter()
        .map(|c| c.planet)
        .collect();
    let has_colonizable_planet = planet_entities.iter().any(|(pe, p, attrs)| {
        p.system == target_entity
            && attrs.map(|a| {
                a.habitability != crate::galaxy::Habitability::Barren
                    && a.habitability != crate::galaxy::Habitability::GasGiant
            }).unwrap_or(false)
            && !colonized_planets.contains(&pe)
    });

    // #108: Unified move — auto-route picks FTL vs sublight
    let can_move = !same_system;
    // Survey: can survey unsurveyed system (docked: immediate/delayed, non-docked: queued)
    let can_survey = crate::ship::design_can_survey(&design_id) && !target_surveyed;
    // #52/#56: Check for hostile presence at target system
    let target_has_hostile = hostile_systems.contains(&target_entity);
    // Colonize: can colonize surveyed system with at least one habitable uncolonized planet and no hostiles
    let can_colonize = crate::ship::design_can_colonize(&design_id) && has_colonizable_planet && target_surveyed && !target_has_hostile;

    let mut command: Option<ShipState> = None;
    let mut queued_command: Option<QueuedCommand> = None;
    // #76: Delayed command for remote ships (light-speed delay > 0)
    let mut delayed_command: Option<crate::ship::ShipCommand> = None;
    let mut close_menu = false;

    // No actions available at all? Close and bail
    if !can_move && !can_survey && !can_colonize {
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
                        planet: None,
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
                if let Ok((_, _, mut state, _, _, _)) = ships_query.get_mut(ship_entity) {
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
            // #108: Unified move — command queue or pending command handles FTL vs sublight
            if command_delay == 0 {
                // Queue the move; process_command_queue will auto-route
                queued_command = Some(QueuedCommand::MoveTo {
                    system: target_entity,
                });
            } else {
                delayed_command = Some(crate::ship::ShipCommand::MoveTo { destination: target_entity });
            }
        } else {
            // Non-docked: queue the default action
            queued_command = Some(QueuedCommand::MoveTo {
                system: target_entity,
            });
        }
        context_menu.open = false;
        context_menu.target_system = None;
        context_menu.execute_default = false;

        if let Some(new_state) = command {
            if let Ok((_, _, mut state, _, _, _)) = ships_query.get_mut(ship_entity) {
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

            // #108: Unified Move — auto-route picks FTL chain > FTL direct > SubLight
            if can_move && ui.button(format!("{}Move to {}", queue_prefix, target_name)).clicked() {
                if is_docked {
                    if command_delay == 0 {
                        // Queue the move; process_command_queue will auto-route
                        queued_command = Some(QueuedCommand::MoveTo {
                            system: target_entity,
                        });
                    } else {
                        delayed_command = Some(crate::ship::ShipCommand::MoveTo { destination: target_entity });
                    }
                } else {
                    queued_command = Some(QueuedCommand::MoveTo {
                        system: target_entity,
                    });
                }
                close_menu = true;
            }

            // Survey -- if Explorer + target unsurveyed
            if can_survey {
                let survey_label = if !is_docked || !same_system { format!("{}Survey", queue_prefix) } else { "Survey".to_string() };
                if ui.button(survey_label).clicked() {
                    if !is_docked {
                        // Ship in transit: queue survey (process_command_queue will auto-insert move if needed)
                        queued_command = Some(QueuedCommand::Survey {
                            system: target_entity,
                        });
                    } else if same_system {
                        if command_delay == 0 {
                            command = Some(ShipState::Surveying {
                                target_system: target_entity,
                                started_at: clock.elapsed,
                                completes_at: clock.elapsed + crate::ship::SURVEY_DURATION_HEXADIES,
                            });
                        } else {
                            delayed_command = Some(crate::ship::ShipCommand::Survey { target: target_entity });
                        }
                    } else {
                        // #108: Queue survey — process_command_queue auto-inserts move
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::MoveTo { destination: target_entity });
                            queued_command = Some(QueuedCommand::Survey {
                                system: target_entity,
                            });
                        } else {
                            queued_command = Some(QueuedCommand::Survey {
                                system: target_entity,
                            });
                        }
                    }
                    close_menu = true;
                }
            }

            // Colonize -- if ColonyShip + target has colonizable planet
            if can_colonize {
                let colonize_label = if !is_docked || !same_system { format!("{}Colonize", queue_prefix) } else { "Colonize".to_string() };
                if ui.button(colonize_label).clicked() {
                    if !is_docked {
                        // Ship in transit: queue colonize (planet auto-selected on arrival)
                        queued_command = Some(QueuedCommand::Colonize {
                            system: target_entity,
                            planet: None,
                        });
                    } else if same_system {
                        if command_delay == 0 {
                            command = Some(ShipState::Settling {
                                system: target_entity,
                                planet: None,
                                started_at: clock.elapsed,
                                completes_at: clock.elapsed + crate::ship::SETTLING_DURATION_HEXADIES,
                            });
                        } else {
                            delayed_command = Some(crate::ship::ShipCommand::Colonize);
                        }
                    } else {
                        // #108: Queue colonize — process_command_queue auto-inserts move
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::MoveTo { destination: target_entity });
                            queued_command = Some(QueuedCommand::Colonize {
                                system: target_entity,
                                planet: None,
                            });
                        } else {
                            queued_command = Some(QueuedCommand::Colonize {
                                system: target_entity,
                                planet: None,
                            });
                        }
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
        if let Ok((_, _, mut state, _, _, _)) = ships_query.get_mut(ship_entity) {
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
    ships: &Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
) -> Vec<(Entity, String, String)> {
    let mut result: Vec<(Entity, String, String)> = ships
        .iter()
        .filter_map(|(e, ship, state, _, _, _)| {
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
