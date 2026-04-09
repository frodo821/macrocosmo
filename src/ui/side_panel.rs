use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildOrder, BuildQueue, Colony, Production, ResourceStockpile};
use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::ship::{Cargo, Ship, ShipState, ShipType};
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};
use crate::visualization::{SelectedShip, SelectedSystem};

/// Draws the right-side system info panel when a star system is selected.
#[allow(clippy::too_many_arguments)]
pub fn draw_system_panel(
    ctx: &egui::Context,
    selected_system: &SelectedSystem,
    selected_ship: &mut SelectedShip,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: &Query<&StationedAt, With<Player>>,
    colonies: &mut Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
    )>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    positions: &Query<&Position>,
    knowledge: &KnowledgeStore,
    clock: &GameClock,
) {
    let Some(sel_entity) = selected_system.0 else {
        return;
    };

    let Ok((_, star, star_pos, attrs)) = stars.get(sel_entity) else {
        return;
    };

    egui::SidePanel::right("system_panel")
        .min_width(260.0)
        .show(ctx, |ui| {
            ui.heading(&star.name);
            ui.separator();

            // Distance and light delay from player
            if let Ok(stationed) = player_q.single() {
                if let Ok(player_pos) = positions.get(stationed.system) {
                    let dist = physics::distance_ly(player_pos, star_pos);
                    let delay_sd = physics::light_delay_hexadies(dist);
                    let delay_yr = physics::light_delay_years(dist);
                    ui.label(format!("Distance: {:.1} ly", dist));
                    ui.label(format!(
                        "Light delay: {} sd ({:.1} yr)",
                        delay_sd, delay_yr
                    ));
                }
            }

            // Survey status
            if star.surveyed {
                ui.label("Status: Surveyed");
            } else {
                ui.label("Status: Unsurveyed");
                ui.label("Approximate position only. Survey required.");
            }

            // Knowledge age
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
                ui.label(format!(
                    "Info age: {} sd ({:.1} yr) [{}]",
                    age, years, freshness
                ));
            }

            // Attributes (if surveyed)
            if star.surveyed {
                if let Some(attrs) = attrs {
                    ui.separator();
                    ui.label(format!("Habitability: {:?}", attrs.habitability));
                    ui.label(format!("Minerals: {:?}", attrs.mineral_richness));
                    ui.label(format!("Energy: {:?}", attrs.energy_potential));
                    ui.label(format!("Research: {:?}", attrs.research_potential));
                    ui.label(format!("Building slots: {}", attrs.max_building_slots));
                }
            }

            // Colony info
            if star.colonized {
                ui.separator();
                ui.label(
                    egui::RichText::new("Colony")
                        .strong()
                        .color(egui::Color32::from_rgb(100, 200, 100)),
                );

                // We need to iterate colonies to find the one matching this system.
                // Because we have a mutable query, we iterate once.
                for (_colony_entity, colony, production, stockpile, build_queue) in
                    colonies.iter_mut()
                {
                    if colony.system != sel_entity {
                        continue;
                    }

                    ui.label(format!("Population: {:.0}", colony.population));

                    if let Some(prod) = production {
                        ui.label(format!(
                            "Production: M {:.1} | E {:.1} | R {:.1} /sd",
                            prod.minerals_per_hexadies,
                            prod.energy_per_hexadies,
                            prod.research_per_hexadies,
                        ));
                    }

                    if let Some(stockpile) = stockpile {
                        ui.label(format!(
                            "Stockpile: M {:.0} | E {:.0} | R {:.0}",
                            stockpile.minerals, stockpile.energy, stockpile.research,
                        ));
                    }

                    // Build queue
                    if let Some(ref bq) = build_queue {
                        ui.separator();
                        ui.label(egui::RichText::new("Build Queue").strong());

                        if bq.queue.is_empty() {
                            ui.label("[empty]");
                        } else {
                            for order in &bq.queue {
                                let m_pct = if order.minerals_cost > 0.0 {
                                    (order.minerals_invested / order.minerals_cost * 100.0)
                                        .min(100.0)
                                } else {
                                    100.0
                                };
                                let e_pct = if order.energy_cost > 0.0 {
                                    (order.energy_invested / order.energy_cost * 100.0).min(100.0)
                                } else {
                                    100.0
                                };
                                let pct = m_pct.min(e_pct);
                                ui.horizontal(|ui| {
                                    ui.label(&order.ship_type_name);
                                    let bar = egui::ProgressBar::new(pct as f32 / 100.0)
                                        .desired_width(100.0);
                                    ui.add(bar);
                                });
                            }
                        }

                        // Build buttons
                        ui.separator();
                        ui.label(egui::RichText::new("Build Ship").strong());
                    }

                    // Build buttons - add orders to the queue
                    if let Some(mut bq) = build_queue {
                        let mut build_request: Option<(&str, f64, f64)> = None;
                        ui.horizontal(|ui| {
                            if ui.button("Explorer").on_hover_text("M:200 E:100").clicked() {
                                build_request = Some(("Explorer", 200.0, 100.0));
                            }
                            if ui
                                .button("Colony Ship")
                                .on_hover_text("M:500 E:300")
                                .clicked()
                            {
                                build_request = Some(("Colony Ship", 500.0, 300.0));
                            }
                            if ui.button("Courier").on_hover_text("M:100 E:50").clicked() {
                                build_request = Some(("Courier", 100.0, 50.0));
                            }
                        });
                        if let Some((name, minerals_cost, energy_cost)) = build_request {
                            bq.queue.push(BuildOrder {
                                ship_type_name: name.to_string(),
                                minerals_cost,
                                minerals_invested: 0.0,
                                energy_cost,
                                energy_invested: 0.0,
                                build_time_total: 0,
                                build_time_remaining: 0,
                            });
                            info!("Build order added: {}", name);
                        }
                    }

                    break;
                }
            }

            // Docked ships
            ui.separator();
            let docked_ships = ships_docked_at(sel_entity, ships_query);
            if !docked_ships.is_empty() {
                ui.label(egui::RichText::new("Docked Ships").strong());
                for (entity, name, ship_type) in &docked_ships {
                    let is_selected = selected_ship.0 == Some(*entity);
                    let label = format!(
                        "{} ({:?}){}",
                        name,
                        ship_type,
                        if is_selected { " [selected]" } else { "" }
                    );
                    if ui.selectable_label(is_selected, &label).clicked() {
                        selected_ship.0 = Some(*entity);
                    }
                }
            }
        });
}

/// Draws the floating ship details panel when a ship is selected.
#[allow(clippy::too_many_arguments)]
pub fn draw_ship_panel(
    ctx: &egui::Context,
    selected_system: &SelectedSystem,
    selected_ship: &mut SelectedShip,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    positions: &Query<&Position>,
    clock: &GameClock,
    colonies: &mut Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
    )>,
) {
    // Collect ship data into locals first, then draw UI, then apply mutations
    let ship_data = selected_ship.0.and_then(|ship_entity| {
        let (_, ship, state, cargo) = ships_query.get(ship_entity).ok()?;
        let docked_system = if let ShipState::Docked { system } = &*state {
            Some(*system)
        } else {
            None
        };
        let cargo_data = cargo.map(|c| (c.minerals, c.energy));
        Some((
            ship_entity,
            ship.name.clone(),
            ship.ship_type,
            ship.hp,
            ship.max_hp,
            ship.ftl_range,
            ship.sublight_speed,
            match &*state {
                ShipState::Docked { .. } => "Docked",
                ShipState::SubLight { .. } => "Sub-light travel",
                ShipState::InFTL { .. } => "FTL travel",
                ShipState::Surveying { .. } => "Surveying",
                ShipState::Settling { .. } => "Settling",
            }
            .to_string(),
            docked_system,
            cargo_data,
        ))
    });

    let Some((
        ship_entity,
        name,
        ship_type,
        hp,
        max_hp,
        ftl_range,
        sublight_speed,
        status,
        docked_system,
        cargo_data,
    )) = ship_data
    else {
        return;
    };

    let sel_entity = selected_system.0;

    let mut command: Option<ShipState> = None;
    let mut deselect_ship = false;

    // Cargo load/unload actions to apply after UI drawing
    #[derive(Default)]
    struct CargoAction {
        load_minerals: f64,
        load_energy: f64,
        unload_minerals: f64,
        unload_energy: f64,
    }
    let mut cargo_action = CargoAction::default();
    // Entity of the colony at the docked system (for cargo transfers)
    let colony_entity_at_dock: Option<Entity> = docked_system.and_then(|dock_sys| {
        colonies.iter().find_map(|(e, col, _, _, _)| {
            if col.system == dock_sys { Some(e) } else { None }
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
            ui.label(format!("Type: {:?}", ship_type));
            ui.label(format!("HP: {:.0}/{:.0}", hp, max_hp));
            ui.label(format!("Status: {}", status));
            if ftl_range > 0.0 {
                ui.label(format!("FTL range: {:.1} ly", ftl_range));
            }
            ui.label(format!(
                "Sub-light speed: {:.0}% c",
                sublight_speed * 100.0
            ));

            if let Some(docked_system) = docked_system {
                ui.separator();
                ui.label(egui::RichText::new("Commands").strong());

                let target_info = sel_entity.and_then(|sel| {
                    if sel != docked_system {
                        if let Ok((_, target_star, target_pos, _)) = stars.get(sel) {
                            if let Ok(dock_pos) = positions.get(docked_system) {
                                let dist = physics::distance_ly(dock_pos, target_pos);
                                ui.label(format!(
                                    "Target: {} ({:.1} ly)",
                                    target_star.name, dist
                                ));
                                return Some((
                                    sel,
                                    dist,
                                    target_star.name.clone(),
                                    target_star.surveyed,
                                    dock_pos.as_array(),
                                    target_pos.as_array(),
                                ));
                            }
                        }
                    }
                    None
                });

                // FTL button
                if ftl_range > 0.0 {
                    let can_ftl = target_info
                        .as_ref()
                        .is_some_and(|(_, dist, _, surveyed, _, _)| {
                            *dist <= ftl_range && *surveyed
                        });
                    if ui
                        .add_enabled(can_ftl, egui::Button::new("FTL Jump"))
                        .on_disabled_hover_text("Select a surveyed system within FTL range")
                        .clicked()
                    {
                        if let Some((target, dist, _, _, _, _)) = &target_info {
                            let travel_time =
                                physics::sublight_travel_hexadies(*dist, 10.0).max(1);
                            command = Some(ShipState::InFTL {
                                origin_system: docked_system,
                                destination_system: *target,
                                departed_at: clock.elapsed,
                                arrival_at: clock.elapsed + travel_time,
                            });
                        }
                    }
                }

                // Sub-light button
                let can_move = target_info.is_some();
                if ui
                    .add_enabled(can_move, egui::Button::new("Move (Sub-light)"))
                    .on_disabled_hover_text("Select a different system as target")
                    .clicked()
                {
                    if let Some((target, dist, _, _, origin, dest)) = &target_info {
                        let travel_time =
                            physics::sublight_travel_hexadies(*dist, sublight_speed);
                        command = Some(ShipState::SubLight {
                            origin: *origin,
                            destination: *dest,
                            target_system: Some(*target),
                            departed_at: clock.elapsed,
                            arrival_at: clock.elapsed + travel_time,
                        });
                    }
                }

                // Survey button
                if ship_type == ShipType::Explorer {
                    let can_survey = target_info
                        .as_ref()
                        .is_some_and(|(_, _, _, surveyed, _, _)| !surveyed);
                    if ui
                        .add_enabled(can_survey, egui::Button::new("Survey"))
                        .on_disabled_hover_text("Select an unsurveyed system as target")
                        .clicked()
                    {
                        if let Some((target, dist, _, _, _, _)) = &target_info {
                            let survey_time =
                                physics::light_delay_hexadies(*dist) * 2 + 5;
                            command = Some(ShipState::Surveying {
                                target_system: *target,
                                started_at: clock.elapsed,
                                completes_at: clock.elapsed + survey_time,
                            });
                        }
                    }
                }

                // Cargo section for Courier ships docked at a colony
                if ship_type == ShipType::Courier {
                    if let Some((cargo_m, cargo_e)) = cargo_data {
                        ui.separator();
                        ui.label(egui::RichText::new("Cargo").strong());
                        ui.label(format!("Minerals: {:.0}", cargo_m));
                        ui.label(format!("Energy: {:.0}", cargo_e));

                        if colony_entity_at_dock.is_some() {
                            ui.horizontal(|ui| {
                                if ui.button("Load M +100").clicked() {
                                    cargo_action.load_minerals = 100.0;
                                }
                                if ui.button("Load E +100").clicked() {
                                    cargo_action.load_energy = 100.0;
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

                if ui.button("Deselect ship").clicked() {
                    deselect_ship = true;
                }
            }
        });

    // Apply deselect
    if deselect_ship {
        selected_ship.0 = None;
    }

    // Apply command mutation AFTER UI drawing (no borrow conflict)
    if let Some(new_state) = command {
        if let Ok((_, _, mut state, _)) = ships_query.get_mut(ship_entity) {
            *state = new_state;
            selected_ship.0 = None;
        }
    }

    // Apply cargo load/unload actions
    let has_cargo_action = cargo_action.load_minerals > 0.0
        || cargo_action.load_energy > 0.0
        || cargo_action.unload_minerals > 0.0
        || cargo_action.unload_energy > 0.0;
    if has_cargo_action {
        if let Some(colony_e) = colony_entity_at_dock {
            // Get mutable access to the colony stockpile
            if let Ok((_, _, _, Some(mut stockpile), _)) = colonies.get_mut(colony_e) {
                if let Ok((_, _, _, Some(mut cargo))) = ships_query.get_mut(ship_entity) {
                    // Load minerals
                    if cargo_action.load_minerals > 0.0 {
                        let transfer = cargo_action.load_minerals.min(stockpile.minerals);
                        stockpile.minerals -= transfer;
                        cargo.minerals += transfer;
                    }
                    // Load energy
                    if cargo_action.load_energy > 0.0 {
                        let transfer = cargo_action.load_energy.min(stockpile.energy);
                        stockpile.energy -= transfer;
                        cargo.energy += transfer;
                    }
                    // Unload minerals
                    if cargo_action.unload_minerals > 0.0 {
                        let transfer = cargo_action.unload_minerals.min(cargo.minerals);
                        cargo.minerals -= transfer;
                        stockpile.minerals += transfer;
                    }
                    // Unload energy
                    if cargo_action.unload_energy > 0.0 {
                        let transfer = cargo_action.unload_energy.min(cargo.energy);
                        cargo.energy -= transfer;
                        stockpile.energy += transfer;
                    }
                }
            }
        }
    }
}

/// Helper to collect ships docked at a given system.
fn ships_docked_at(
    system: Entity,
    ships: &Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
) -> Vec<(Entity, String, ShipType)> {
    let mut result: Vec<(Entity, String, ShipType)> = ships
        .iter()
        .filter_map(|(e, ship, state, _)| {
            if let ShipState::Docked { system: s } = &*state {
                if *s == system {
                    return Some((e, ship.name.clone(), ship.ship_type));
                }
            }
            None
        })
        .collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    result
}
