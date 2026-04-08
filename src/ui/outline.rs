use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildQueue, Colony, Production, ResourceStockpile};
use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::ship::{Ship, ShipState, ShipType};
use crate::visualization::{SelectedShip, SelectedSystem};

/// Draws the left-side outline panel showing owned systems and ships.
#[allow(clippy::too_many_arguments)]
pub fn draw_outline(
    ctx: &egui::Context,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    colonies: &Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&ResourceStockpile>,
        Option<&mut BuildQueue>,
    )>,
    ships: &Query<(Entity, &mut Ship, &mut ShipState)>,
    selected_system: &mut SelectedSystem,
    selected_ship: &mut SelectedShip,
) {
    egui::SidePanel::left("outline_panel")
        .min_width(180.0)
        .max_width(220.0)
        .show(ctx, |ui| {
            ui.heading("Empire");
            ui.separator();

            // Collect systems that have colonies (owned systems)
            let mut owned_systems: Vec<(Entity, String, bool)> = Vec::new();
            for (_, colony, _, _, _) in colonies.iter() {
                if let Ok((entity, star, _, _)) = stars.get(colony.system) {
                    // Avoid duplicates if multiple colonies on same system
                    if !owned_systems.iter().any(|(e, _, _)| *e == entity) {
                        owned_systems.push((entity, star.name.clone(), star.is_capital));
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
                        for (ship_entity, name, ship_type) in &docked {
                            let label = format!("  {} ({:?})", name, ship_type);
                            let is_selected = selected_ship.0 == Some(*ship_entity);
                            if ui.selectable_label(is_selected, &label).clicked() {
                                selected_ship.0 = Some(*ship_entity);
                                selected_system.0 = Some(*system_entity);
                            }
                        }
                    }
                });

                // Click on the header to select the system
                if header_response.header_response.clicked() {
                    selected_system.0 = Some(*system_entity);
                    selected_ship.0 = None;
                }
            }

            // "In Transit" section for ships not docked
            let mut in_transit: Vec<(Entity, String, ShipType, &str)> = Vec::new();
            for (entity, ship, state) in ships.iter() {
                let status = match &*state {
                    ShipState::Docked { .. } => continue,
                    ShipState::SubLight { .. } => "Moving",
                    ShipState::InFTL { .. } => "FTL",
                    ShipState::Surveying { .. } => "Surveying",
                    ShipState::Settling { .. } => "Settling",
                };
                in_transit.push((entity, ship.name.clone(), ship.ship_type, status));
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
                            if ui.selectable_label(is_selected, &label).clicked() {
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
    ships: &Query<(Entity, &mut Ship, &mut ShipState)>,
) -> Vec<(Entity, String, ShipType)> {
    let mut result: Vec<(Entity, String, ShipType)> = ships
        .iter()
        .filter_map(|(e, ship, state)| {
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
