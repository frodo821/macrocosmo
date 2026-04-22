use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{
    BuildQueue, BuildingQueue, Buildings, Colony, ConstructionParams, FoodConsumption,
    MaintenanceCost, Production, ResourceCapacity, ResourceStockpile, SlotAssignment,
};
use crate::communication::PendingColonyDispatches;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::scripting::building_api::BuildingRegistry;
use crate::ship::{Cargo, Ship, ShipHitpoints, ShipState, SurveyData};
use crate::visualization::SelectedPlanet;

use crate::faction::FactionOwner;

use super::colony_detail::draw_colony_detail;
use super::format_planet_type;

/// Draws the floating planet info window when a planet is selected.
/// Shows planet attributes, colony detail, buildings, and build queue.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_planet_window(
    ctx: &egui::Context,
    system_entity: Entity,
    selected_planet: &mut SelectedPlanet,
    colonized_planets: &std::collections::HashSet<Entity>,
    stars: &Query<(
        Entity,
        &StarSystem,
        &crate::components::Position,
        Option<&SystemAttributes>,
    )>,
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
    colony_pop_view: &Query<(
        Entity,
        Option<&crate::species::ColonyPopulation>,
        Option<&crate::species::ColonyJobs>,
        Option<&crate::colony::ColonyJobRates>,
    )>,
    system_stockpiles: &mut Query<
        (&mut ResourceStockpile, Option<&ResourceCapacity>),
        With<StarSystem>,
    >,
    ships_query: &mut Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    ), Without<SlotAssignment>>,
    construction_params: &ConstructionParams,
    planets: &Query<&Planet>,
    planet_entities: &Query<(Entity, &Planet, Option<&SystemAttributes>)>,
    hull_registry: &crate::ship_design::HullRegistry,
    module_registry: &crate::ship_design::ModuleRegistry,
    design_registry: &crate::ship_design::ShipDesignRegistry,
    building_registry: &BuildingRegistry,
    job_registry: &crate::species::JobRegistry,
    colony_panel_tab: &mut crate::ui::ColonyPanelTab,
    dispatches: &mut PendingColonyDispatches,
    is_local_system: bool,
    k_data: Option<&crate::knowledge::SystemKnowledge>,
    clock_elapsed: i64,
    // #432: Active empire entity for colony ownership checks.
    viewed_empire: Entity,
    // #432: Observer mode flag.
    is_observer: bool,
    // #432: FactionOwner lookup for colony ownership.
    faction_owners: &Query<&FactionOwner>,
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

    let is_surveyed = stars
        .get(system_entity)
        .map(|(_, s, _, _)| s.surveyed)
        .unwrap_or(false);
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
                    ui.label(format!(
                        "Habitability: {} ({:.0}%)",
                        crate::galaxy::habitability_label(attrs.habitability),
                        attrs.habitability * 100.0
                    ));
                    ui.label(format!(
                        "Minerals: {} ({:.0}%)",
                        crate::galaxy::resource_label(attrs.mineral_richness),
                        attrs.mineral_richness * 100.0
                    ));
                    ui.label(format!(
                        "Energy: {} ({:.0}%)",
                        crate::galaxy::resource_label(attrs.energy_potential),
                        attrs.energy_potential * 100.0
                    ));
                    ui.label(format!(
                        "Research: {} ({:.0}%)",
                        crate::galaxy::resource_label(attrs.research_potential),
                        attrs.research_potential * 100.0
                    ));
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
                let planet_attrs = planet_entities
                    .get(sel_planet_entity)
                    .ok()
                    .and_then(|(_, _, a)| a);

                // #432: Determine if the colony on this planet belongs to the viewed empire.
                let is_own_colony = if is_observer {
                    true
                } else {
                    colonies.iter().find_map(|(ce, c, _, _, _, _, _, _)| {
                        if c.planet == sel_planet_entity { Some(ce) } else { None }
                    }).map_or(false, |ce| {
                        faction_owners.get(ce).map(|fo| fo.0 == viewed_empire).unwrap_or(false)
                    })
                };

                egui::ScrollArea::vertical()
                    .max_height(500.0)
                    .show(ui, |ui| {
                        draw_colony_detail(
                            ui,
                            sel_planet_entity,
                            system_entity,
                            planet_attrs,
                            colonies,
                            colony_pop_view,
                            system_stockpiles,
                            ships_query,
                            construction_params,
                            planets,
                            hull_registry,
                            module_registry,
                            design_registry,
                            building_registry,
                            job_registry,
                            colony_panel_tab,
                            dispatches,
                            is_local_system,
                            k_data,
                            clock_elapsed,
                            is_own_colony,
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
