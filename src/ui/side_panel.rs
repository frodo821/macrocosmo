use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::colony::{BuildOrder, BuildQueue, Colony, Production, ResourceStockpile};
use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::ship::{Ship, ShipState, ShipType};
use crate::time_system::{GameClock, SEXADIES_PER_YEAR};
use crate::visualization::{SelectedShip, SelectedSystem};

/// System that draws the right-side info panel when a star system is selected.
pub fn draw_side_panel(
    mut contexts: EguiContexts,
    selected_system: Res<SelectedSystem>,
    mut selected_ship: ResMut<SelectedShip>,
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: Query<&StationedAt, With<Player>>,
    mut colonies: Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&ResourceStockpile>,
        Option<&mut BuildQueue>,
    )>,
    ships_query: Query<(Entity, &Ship, &ShipState)>,
    mut ship_writer: Query<(&mut Ship, &mut ShipState)>,
    positions: Query<&Position>,
    knowledge: Res<KnowledgeStore>,
    clock: Res<GameClock>,
) {
    let Some(sel_entity) = selected_system.0 else {
        return;
    };

    let Ok((_, star, star_pos, attrs)) = stars.get(sel_entity) else {
        return;
    };

    let Ok(ctx) = contexts.ctx_mut() else { return };

    egui::SidePanel::right("info_panel")
        .min_width(260.0)
        .show(ctx, |ui| {
            ui.heading(&star.name);
            ui.separator();

            // Distance and light delay from player
            if let Ok(stationed) = player_q.single() {
                if let Ok(player_pos) = positions.get(stationed.system) {
                    let dist = physics::distance_ly(player_pos, star_pos);
                    let delay_sd = physics::light_delay_sexadies(dist);
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
                let years = age as f64 / SEXADIES_PER_YEAR as f64;
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
                let mut colony_found = false;
                for (_colony_entity, colony, production, stockpile, build_queue) in &mut colonies {
                    if colony.system != sel_entity {
                        continue;
                    }
                    colony_found = true;

                    ui.label(format!("Population: {:.0}", colony.population));

                    if let Some(prod) = production {
                        ui.label(format!(
                            "Production: M {:.1} | E {:.1} | R {:.1} /sd",
                            prod.minerals_per_sexadie,
                            prod.energy_per_sexadie,
                            prod.research_per_sexadie,
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
                        // We need to mutate the build queue, so we use the mutable reference
                        // through the Option<Mut<BuildQueue>> which we already have.
                    }

                    // Build buttons - we need to add orders to the queue
                    // Using a separate block to handle the mutation
                    break;
                }

                // Build buttons - we do a second pass to actually mutate
                // (The borrow checker requires we drop the immutable refs first)
                for (_colony_entity, colony, _production, _stockpile, build_queue) in &mut colonies {
                    if colony.system != sel_entity {
                        continue;
                    }
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
            let docked_ships = ships_docked_at(sel_entity, &ships_query);
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

            // Selected ship details
            if let Some(ship_entity) = selected_ship.0 {
                if let Ok((_, ship, state)) = ships_query.get(ship_entity) {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("Ship: {}", ship.name))
                            .strong()
                            .color(egui::Color32::from_rgb(100, 200, 255)),
                    );
                    ui.label(format!("Type: {:?}", ship.ship_type));
                    ui.label(format!("HP: {:.0}/{:.0}", ship.hp, ship.max_hp));

                    let status = match state {
                        ShipState::Docked { .. } => "Docked".to_string(),
                        ShipState::SubLight { .. } => "Sub-light travel".to_string(),
                        ShipState::InFTL { .. } => "FTL travel".to_string(),
                        ShipState::Surveying { .. } => "Surveying".to_string(),
                        ShipState::Settling { .. } => "Settling".to_string(),
                    };
                    ui.label(format!("Status: {}", status));

                    if ship.ftl_range > 0.0 {
                        ui.label(format!("FTL range: {:.1} ly", ship.ftl_range));
                    }
                    ui.label(format!(
                        "Sub-light speed: {:.0}% c",
                        ship.sublight_speed * 100.0
                    ));

                    // Commands (only when docked)
                    if let ShipState::Docked { system: docked_system } = state {
                        ui.separator();
                        ui.label(egui::RichText::new("Commands").strong());

                        let docked_system = *docked_system;

                        // We need to know what the selected system is for targeting
                        let target_system = if sel_entity != docked_system {
                            // Show target info
                            if let Ok((_, target_star, target_pos, _)) = stars.get(sel_entity) {
                                if let Ok(dock_pos) = positions.get(docked_system) {
                                    let dist = physics::distance_ly(dock_pos, target_pos);
                                    ui.label(format!(
                                        "Target: {} ({:.1} ly)",
                                        target_star.name, dist
                                    ));
                                    Some((sel_entity, dist))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        // FTL button
                        if ship.ftl_range > 0.0 {
                            let can_ftl = target_system.is_some_and(|(target, dist)| {
                                dist <= ship.ftl_range
                                    && stars
                                        .get(target)
                                        .map(|(_, s, _, _)| s.surveyed)
                                        .unwrap_or(false)
                            });
                            if ui
                                .add_enabled(can_ftl, egui::Button::new("FTL Jump"))
                                .on_disabled_hover_text(
                                    "Select a surveyed system within FTL range",
                                )
                                .clicked()
                            {
                                if let Some((target, dist)) = target_system {
                                    let travel_time =
                                        physics::sublight_travel_sexadies(dist, 10.0).max(1);
                                    if let Ok((ref _s, ref mut state_mut)) =
                                        ship_writer.get_mut(ship_entity)
                                    {
                                        **state_mut = ShipState::InFTL {
                                            origin_system: docked_system,
                                            destination_system: target,
                                            departed_at: clock.elapsed,
                                            arrival_at: clock.elapsed + travel_time,
                                        };
                                        info!("Ship {} jumping via FTL (ETA: {} sd)", ship.name, travel_time);
                                        selected_ship.0 = None;
                                    }
                                }
                            }
                        }

                        // Sub-light move button
                        let can_move = target_system.is_some();
                        if ui
                            .add_enabled(can_move, egui::Button::new("Move (Sub-light)"))
                            .on_disabled_hover_text("Select a different system as target")
                            .clicked()
                        {
                            if let Some((target, dist)) = target_system {
                                let travel_time =
                                    physics::sublight_travel_sexadies(dist, ship.sublight_speed);
                                if let Ok((_, target_star, target_pos, _)) = stars.get(target) {
                                    if let Ok(dock_pos) = positions.get(docked_system) {
                                        if let Ok((ref _s, ref mut state_mut)) =
                                            ship_writer.get_mut(ship_entity)
                                        {
                                            **state_mut = ShipState::SubLight {
                                                origin: dock_pos.as_array(),
                                                destination: target_pos.as_array(),
                                                target_system: Some(target),
                                                departed_at: clock.elapsed,
                                                arrival_at: clock.elapsed + travel_time,
                                            };
                                            info!(
                                                "Ship {} departing for {} (ETA: {} sd)",
                                                ship.name, target_star.name, travel_time
                                            );
                                            selected_ship.0 = None;
                                        }
                                    }
                                }
                            }
                        }

                        // Survey button (Explorer only)
                        if ship.ship_type == ShipType::Explorer {
                            let can_survey = target_system.is_some_and(|(target, _dist)| {
                                stars
                                    .get(target)
                                    .map(|(_, s, _, _)| !s.surveyed)
                                    .unwrap_or(false)
                            });
                            if ui
                                .add_enabled(can_survey, egui::Button::new("Survey"))
                                .on_disabled_hover_text(
                                    "Select an unsurveyed system as target",
                                )
                                .clicked()
                            {
                                if let Some((target, dist)) = target_system {
                                    let survey_time =
                                        physics::light_delay_sexadies(dist) * 2 + 5;
                                    if let Ok((ref _s, ref mut state_mut)) =
                                        ship_writer.get_mut(ship_entity)
                                    {
                                        **state_mut = ShipState::Surveying {
                                            target_system: target,
                                            started_at: clock.elapsed,
                                            completes_at: clock.elapsed + survey_time,
                                        };
                                        info!(
                                            "Ship {} surveying (ETA: {} sd)",
                                            ship.name, survey_time
                                        );
                                        selected_ship.0 = None;
                                    }
                                }
                            }
                        }

                        // Deselect ship button
                        if ui.button("Deselect ship").clicked() {
                            selected_ship.0 = None;
                        }
                    }
                }
            }
        });
}

/// Helper to collect ships docked at a given system.
fn ships_docked_at(
    system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState)>,
) -> Vec<(Entity, String, ShipType)> {
    let mut result: Vec<(Entity, String, ShipType)> = ships
        .iter()
        .filter_map(|(e, ship, state)| {
            if let ShipState::Docked { system: s } = state {
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
