use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildQueue, BuildingQueue, Buildings, Colony, Production};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::ship::{Cargo, Ship, ShipHitpoints, ShipState, SurveyData};
use crate::visualization::{SelectedShip, SelectedSystem};

/// Helper: format a ship status string from ShipState.
fn ship_status_label(state: &ShipState) -> &'static str {
    match state {
        ShipState::Docked { .. } => "Docked",
        ShipState::SubLight { .. } => "Sub-light",
        ShipState::InFTL { .. } => "In FTL",
        ShipState::Surveying { .. } => "Surveying",
        ShipState::Settling { .. } => "Settling",
        ShipState::Refitting { .. } => "Refitting",
    }
}

/// Draws the left-side outline panel showing owned systems and ships.
#[allow(clippy::too_many_arguments)]
pub fn draw_outline(
    ctx: &egui::Context,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    colonies: &Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&crate::colony::MaintenanceCost>,
        Option<&crate::colony::FoodConsumption>,
    )>,
    ships: &Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    selected_system: &mut SelectedSystem,
    selected_ship: &mut SelectedShip,
    planets: &Query<&Planet>,
) {
    egui::SidePanel::left("outline_panel")
        .min_width(180.0)
        .max_width(220.0)
        .show(ctx, |ui| {
            ui.heading("Empire");
            ui.separator();

            // Collect systems that have colonies (owned systems)
            let mut owned_systems: Vec<(Entity, String, bool)> = Vec::new();
            for (_, colony, _, _, _, _, _, _) in colonies.iter() {
                if let Some(sys) = colony.system(planets) {
                    if let Ok((entity, star, _, _)) = stars.get(sys) {
                        // Avoid duplicates if multiple colonies on same system
                        if !owned_systems.iter().any(|(e, _, _)| *e == entity) {
                            owned_systems.push((entity, star.name.clone(), star.is_capital));
                        }
                    }
                }
            }

            // Sort: capital first, then alphabetical
            owned_systems.sort_by(|a, b| {
                b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1))
            });

            for (system_entity, system_name, is_capital) in &owned_systems {
                let header_text = if *is_capital {
                    format!("{} \u{2605}", system_name)
                } else {
                    system_name.clone()
                };

                let is_system_selected = selected_system.0 == Some(*system_entity);

                let id = ui.make_persistent_id(format!("outline_sys_{:?}", system_entity));
                let header_response = egui::CollapsingHeader::new(
                    egui::RichText::new(&header_text).color(if is_system_selected {
                        egui::Color32::from_rgb(0, 255, 255)
                    } else {
                        egui::Color32::from_rgb(200, 200, 200)
                    }),
                )
                .id_salt(id)
                .default_open(*is_capital)
                .show(ui, |ui| {
                    // List docked ships at this system
                    let docked = ships_docked_at(*system_entity, ships);
                    if docked.is_empty() {
                        ui.label(
                            egui::RichText::new("  (no ships)")
                                .weak()
                                .italics(),
                        );
                    } else {
                        for (ship_entity, name, design_id) in &docked {
                            let design_name = crate::ship::design_preset(design_id).map(|p| p.design_name).unwrap_or(design_id);
                            let label = format!("  {} ({})", name, design_name);
                            let is_selected = selected_ship.0 == Some(*ship_entity);
                            let mut response = ui.selectable_label(is_selected, &label);
                            // Ship tooltip
                            if let Ok((_, ship, state, _, hp, _)) = ships.get(*ship_entity) {
                                response = response.on_hover_ui(|ui| {
                                    ui.label(egui::RichText::new(&ship.name).strong());
                                    ui.label(format!("Design: {}", design_name));
                                    ui.label(format!("Status: {}", ship_status_label(&state)));
                                    ui.label(format!("HP: {:.0}/{:.0}", hp.hull, hp.hull_max));
                                    if hp.armor_max > 0.0 {
                                        ui.label(format!("Armor: {:.0}/{:.0}", hp.armor, hp.armor_max));
                                    }
                                    if hp.shield_max > 0.0 {
                                        ui.label(format!("Shield: {:.0}/{:.0}", hp.shield, hp.shield_max));
                                    }
                                });
                            }
                            if response.clicked() {
                                selected_ship.0 = Some(*ship_entity);
                                // Don't touch selected_system — selections are independent
                            }
                        }
                    }
                });

                // Tooltip for system header + click handling
                let hr = header_response.header_response.on_hover_ui(|ui| {
                    ui.label(egui::RichText::new(system_name).strong());
                    if *is_capital {
                        ui.label("Capital system");
                    }
                    let planet_count = planets.iter().filter(|p| p.system == *system_entity).count();
                    ui.label(format!("Planets: {}", planet_count));
                    ui.label("Colonized");
                });
                if hr.clicked() {
                    selected_system.0 = Some(*system_entity);
                    // Don't touch selected_ship — selections are independent
                }
            }

            // Collect owned system entities for lookup
            let owned_system_entities: Vec<Entity> =
                owned_systems.iter().map(|(e, _, _)| *e).collect();

            // "Stationed Elsewhere" section for ships docked at unowned systems
            let mut unowned_system_ships: Vec<(Entity, String, Vec<(Entity, String, String)>)> =
                Vec::new();
            for (entity, ship, state, _, _, _) in ships.iter() {
                if let ShipState::Docked { system } = &*state {
                    if !owned_system_entities.contains(system) {
                        // Find or create entry for this system
                        if let Ok((_, star, _, _)) = stars.get(*system) {
                            if let Some(entry) = unowned_system_ships
                                .iter_mut()
                                .find(|(e, _, _)| *e == *system)
                            {
                                entry.2.push((entity, ship.name.clone(), ship.design_id.clone()));
                            } else {
                                unowned_system_ships.push((
                                    *system,
                                    star.name.clone(),
                                    vec![(entity, ship.name.clone(), ship.design_id.clone())],
                                ));
                            }
                        }
                    }
                }
            }
            unowned_system_ships.sort_by(|a, b| a.1.cmp(&b.1));
            for entry in &mut unowned_system_ships {
                entry.2.sort_by(|a, b| a.1.cmp(&b.1));
            }

            if !unowned_system_ships.is_empty() {
                ui.separator();
                egui::CollapsingHeader::new("Stationed Elsewhere")
                    .default_open(true)
                    .show(ui, |ui| {
                        for (system_entity, system_name, docked) in &unowned_system_ships {
                            let is_system_selected =
                                selected_system.0 == Some(*system_entity);
                            let id = ui.make_persistent_id(format!(
                                "outline_unowned_{:?}",
                                system_entity
                            ));
                            let header_response = egui::CollapsingHeader::new(
                                egui::RichText::new(system_name).color(
                                    if is_system_selected {
                                        egui::Color32::from_rgb(0, 255, 255)
                                    } else {
                                        egui::Color32::from_rgb(160, 160, 160)
                                    },
                                ),
                            )
                            .id_salt(id)
                            .default_open(true)
                            .show(ui, |ui| {
                                for (ship_entity, name, design_id) in docked {
                                    let design_name = crate::ship::design_preset(design_id).map(|p| p.design_name).unwrap_or(design_id);
                                    let label =
                                        format!("  {} ({})", name, design_name);
                                    let is_selected =
                                        selected_ship.0 == Some(*ship_entity);
                                    let mut response = ui.selectable_label(is_selected, &label);
                                    if let Ok((_, ship, state, _, hp, _)) = ships.get(*ship_entity) {
                                        response = response.on_hover_ui(|ui| {
                                            ui.label(egui::RichText::new(&ship.name).strong());
                                            ui.label(format!("Design: {}", design_name));
                                            ui.label(format!("Status: {}", ship_status_label(&state)));
                                            ui.label(format!("HP: {:.0}/{:.0}", hp.hull, hp.hull_max));
                                            if hp.armor_max > 0.0 {
                                                ui.label(format!("Armor: {:.0}/{:.0}", hp.armor, hp.armor_max));
                                            }
                                            if hp.shield_max > 0.0 {
                                                ui.label(format!("Shield: {:.0}/{:.0}", hp.shield, hp.shield_max));
                                            }
                                        });
                                    }
                                    if response.clicked()
                                    {
                                        selected_ship.0 = Some(*ship_entity);
                                        // Don't touch selected_system — selections are independent
                                    }
                                }
                            });
                            // Tooltip for unowned system header + click handling
                            let hr = header_response.header_response.on_hover_ui(|ui| {
                                ui.label(egui::RichText::new(system_name).strong());
                                let planet_count = planets.iter().filter(|p| p.system == *system_entity).count();
                                ui.label(format!("Planets: {}", planet_count));
                            });
                            if hr.clicked() {
                                selected_system.0 = Some(*system_entity);
                                // Don't touch selected_ship — selections are independent
                            }
                        }
                    });
            }

            // "In Transit" section for ships not docked
            let mut in_transit: Vec<(Entity, String, String, &str)> = Vec::new();
            for (entity, ship, state, _, _, _) in ships.iter() {
                let status = match &*state {
                    ShipState::Docked { .. } => continue,
                    ShipState::SubLight { .. } => "Moving",
                    ShipState::InFTL { .. } => "FTL",
                    ShipState::Surveying { .. } => "Surveying",
                    ShipState::Settling { .. } => "Settling",
                    ShipState::Refitting { .. } => continue,
                };
                in_transit.push((entity, ship.name.clone(), ship.design_id.clone(), status));
            }
            in_transit.sort_by(|a, b| a.1.cmp(&b.1));

            if !in_transit.is_empty() {
                ui.separator();
                egui::CollapsingHeader::new("In Transit")
                    .default_open(true)
                    .show(ui, |ui| {
                        for (entity, name, _ship_type, status) in &in_transit {
                            let label = format!("{} [{}]", name, status);
                            let is_selected = selected_ship.0 == Some(*entity);
                            let mut response = ui.selectable_label(is_selected, &label);
                            if let Ok((_, ship, _state, _, hp, _)) = ships.get(*entity) {
                                let design_name = crate::ship::design_preset(&ship.design_id).map(|p| p.design_name).unwrap_or(&ship.design_id);
                                response = response.on_hover_ui(|ui| {
                                    ui.label(egui::RichText::new(&ship.name).strong());
                                    ui.label(format!("Design: {}", design_name));
                                    ui.label(format!("Status: {}", status));
                                    ui.label(format!("HP: {:.0}/{:.0}", hp.hull, hp.hull_max));
                                    if hp.armor_max > 0.0 {
                                        ui.label(format!("Armor: {:.0}/{:.0}", hp.armor, hp.armor_max));
                                    }
                                    if hp.shield_max > 0.0 {
                                        ui.label(format!("Shield: {:.0}/{:.0}", hp.shield, hp.shield_max));
                                    }
                                });
                            }
                            if response.clicked() {
                                selected_ship.0 = Some(*entity);
                            }
                        }
                    });
            }
        });
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

