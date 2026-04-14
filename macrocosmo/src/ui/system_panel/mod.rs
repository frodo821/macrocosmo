mod colony_detail;
mod planet_window;

use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildOrder, BuildQueue, BuildingQueue, Buildings, Colony, ColonizationQueue, ConstructionParams, DeliverableStockpile, FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile, SystemBuildings, SystemBuildingQueue};
use crate::condition::{EvalContext, ScopeData};
use crate::deep_space::{ConstructionPlatform, DeepSpaceStructure, Scrapyard, StructureRegistry};
use crate::scripting::building_api::{BuildingId, BuildingRegistry};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes, habitability_label, is_colonizable};
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

/// #229: Action produced by the system panel that must be applied by the
/// outer `draw_main_panels_system` (where exclusive world access and
/// `Commands` are available). Unlike `ShipPanelActions`, these target
/// world-state entities (structures, stockpiles) rather than ship state.
#[derive(Default)]
pub struct SystemPanelActions {
    /// A `deliverable_ops::dismantle_structure` should be queued on this
    /// structure entity. Requires `&mut World`, so the caller wraps it in
    /// `commands.queue(...)`.
    pub dismantle: Option<Entity>,
    /// #229: Player clicked "Load" on a DeliverableStockpile row while a
    /// ship was docked at the system. Push a `LoadDeliverable` command onto
    /// that ship's CommandQueue. Payload: (ship, system, stockpile_index).
    pub load_deliverable: Option<(Entity, Entity, usize)>,
}

/// #229: Information that the shipyard "Deliverables" button list needs
/// to decide whether a definition is buildable right now. Pre-computed in
/// `draw_main_panels_system` and passed down through the system panel
/// signature. Using `HashSet<String>` lets us re-use `EvalContext::flat`.
pub struct DeliverableAvailabilityCtx<'a> {
    pub researched_techs: &'a std::collections::HashSet<String>,
    pub active_modifiers: &'a std::collections::HashSet<String>,
    pub empire_flags: &'a std::collections::HashSet<String>,
    pub empire_buildings: &'a std::collections::HashSet<String>,
}

impl<'a> DeliverableAvailabilityCtx<'a> {
    fn as_eval(&self) -> EvalContext<'a> {
        EvalContext {
            researched_techs: self.researched_techs,
            active_modifiers: self.active_modifiers,
            empire: Some(ScopeData {
                flags: self.empire_flags,
                buildings: self.empire_buildings,
            }),
            system: None,
            planet: None,
            ship: None,
        }
    }
}

/// #229: Filter the structure/deliverable registry to shipyard-buildable
/// items whose `prerequisites` evaluate to `true` in the given context.
/// Returns `(definition_id, display_name)` pairs sorted alphabetically so
/// the UI has a stable order across frames.
pub fn available_shipyard_deliverables<'a>(
    registry: &'a StructureRegistry,
    avail: &DeliverableAvailabilityCtx<'_>,
) -> Vec<(&'a str, &'a str)> {
    let ctx = avail.as_eval();
    let mut out: Vec<(&str, &str)> = registry
        .definitions
        .values()
        .filter(|def| def.deliverable.is_some())
        .filter(|def| match &def.prerequisites {
            Some(cond) => cond.evaluate(&ctx).is_satisfied(),
            None => true,
        })
        .map(|def| (def.id.as_str(), def.name.as_str()))
        .collect();
    out.sort_by(|a, b| a.1.cmp(b.1));
    out
}

/// Draws the full-screen system detail view when a star system is selected.
/// Layout: left info panel | central system map | right actions panel.
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
    colony_pop_view: &Query<(
        Entity,
        Option<&crate::species::ColonyPopulation>,
        Option<&crate::species::ColonyJobs>,
        Option<&crate::colony::ColonyJobRates>,
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
    job_registry: &crate::species::JobRegistry,
    colony_panel_tab: &mut crate::ui::ColonyPanelTab,
    anomalies_q: &Query<&crate::galaxy::Anomalies>,
    deliverable_stockpiles: &Query<&DeliverableStockpile, With<StarSystem>>,
    deep_space_structures: &Query<(
        Entity,
        &DeepSpaceStructure,
        &Position,
        Option<&ConstructionPlatform>,
        Option<&Scrapyard>,
    )>,
    structure_registry: &StructureRegistry,
    deliverable_avail: &DeliverableAvailabilityCtx<'_>,
    system_actions_out: &mut SystemPanelActions,
    dispatches: &mut crate::communication::PendingColonyDispatches,
    remote_commands: &Query<&crate::communication::PendingCommand>,
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

    // #176: Survey status from knowledge for remote systems
    let effective_surveyed = if is_local_system {
        star.surveyed
    } else {
        k_data.map(|k| k.data.surveyed).unwrap_or(false)
    };

    // Collect data for docked ships before drawing (to avoid borrow issues)
    let docked_ships = ships_docked_at(sel_entity, ships_query);
    // #229: Is the currently-selected ship docked at this system? Enables
    // the "Load" button on DeliverableStockpile rows.
    let selected_ship_docked_here: Option<Entity> =
        selected_ship.0.filter(|e| docked_ships.iter().any(|(se, _, _)| se == e));

    // Collect system stockpile info for display
    let stockpile_info: Option<(Amt, Amt, Amt, Amt)> = system_stockpiles.get(sel_entity).ok()
        .map(|(s, _)| (s.minerals, s.energy, s.food, s.authority));

    let screen = ctx.screen_rect();
    let mut close_system_view = false;

    // Full-screen window with three-column layout
    egui::Window::new(format!("{} ({})", star.name, format_star_type(&star.star_type)))
        .id(egui::Id::new("system_detail_view"))
        .fixed_pos(egui::pos2(0.0, 28.0))
        .fixed_size(egui::vec2(screen.width(), screen.height() - 28.0))
        .title_bar(false)
        .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(10, 10, 20)))
        .show(ctx, |ui| {
            // === Top bar with system name and close button ===
            ui.horizontal(|ui| {
                if ui.button("\u{2190} Back to Galaxy").clicked() {
                    close_system_view = true;
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(&star.name)
                        .heading()
                        .strong()
                        .color(egui::Color32::from_rgb(220, 220, 255)),
                );
                ui.label(
                    egui::RichText::new(format!("({})", format_star_type(&star.star_type)))
                        .color(egui::Color32::from_rgb(160, 160, 200)),
                );

                if let Ok((_, stationed, _)) = player_q.single() {
                    if let Ok(player_pos) = positions.get(stationed.system) {
                        let dist = physics::distance_ly(player_pos, star_pos);
                        let delay_sd = physics::light_delay_hexadies(dist);
                        ui.separator();
                        ui.label(format!("{:.1} ly | {} hd delay", dist, delay_sd));
                    }
                }

                if effective_surveyed {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Surveyed")
                            .color(egui::Color32::from_rgb(100, 200, 100)),
                    );
                } else {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Unsurveyed")
                            .color(egui::Color32::from_rgb(200, 150, 100)),
                    );
                }
            });
            ui.separator();

            // === Three-column layout ===
            let available_width = ui.available_width();
            let available_height = ui.available_height();
            let left_width = (available_width * 0.22).clamp(180.0, 320.0);
            let right_width = (available_width * 0.25).clamp(200.0, 380.0);
            let center_width = (available_width - left_width - right_width - 16.0).max(200.0);

            ui.horizontal_top(|ui| {
                // === LEFT PANEL: System info ===
                ui.allocate_ui_with_layout(
                    egui::vec2(left_width, available_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(15, 15, 28))
                        .inner_margin(6.0)
                        .rounding(4.0)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("system_panel_left")
                                .max_height(available_height - 8.0)
                                .show(ui, |ui| {
                                    draw_left_panel(
                                        ui,
                                        sel_entity,
                                        star,
                                        star_pos,
                                        is_local_system,
                                        effective_surveyed,
                                        k_data,
                                        knowledge,
                                        clock,
                                        player_q,
                                        positions,
                                        &system_planets,
                                        selected_planet,
                                        &stockpile_info,
                                        anomalies_q,
                                        deliverable_stockpiles,
                                        selected_ship_docked_here,
                                        system_actions_out,
                                    );
                                });
                        });
                });

                // === CENTER: System map ===
                ui.allocate_ui_with_layout(
                    egui::vec2(center_width, available_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(6, 6, 14))
                        .inner_margin(4.0)
                        .rounding(4.0)
                        .show(ui, |ui| {
                            draw_system_map(
                                ui,
                                &star.star_type,
                                &system_planets,
                                selected_planet,
                                &docked_ships,
                                design_registry,
                            );
                        });
                });

                // === RIGHT PANEL: Actions ===
                ui.allocate_ui_with_layout(
                    egui::vec2(right_width, available_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(15, 15, 28))
                        .inner_margin(6.0)
                        .rounding(4.0)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("system_panel_right")
                                .max_height(available_height - 8.0)
                                .show(ui, |ui| {
                                    draw_right_panel(
                                        ui,
                                        sel_entity,
                                        star_pos,
                                        selected_ship,
                                        &docked_ships,
                                        hull_registry,
                                        module_registry,
                                        design_registry,
                                        system_buildings_q,
                                        construction_params,
                                        building_registry,
                                        &colonized_planets,
                                        planet_entities,
                                        planets,
                                        colonies,
                                        colonization_queues,
                                        colonization_actions_out,
                                        deep_space_structures,
                                        structure_registry,
                                        deliverable_avail,
                                        system_actions_out,
                                        dispatches,
                                        remote_commands,
                                        clock.elapsed,
                                    );
                                });
                        });
                });
            });
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
        colony_pop_view,
        system_stockpiles,
        ships_query,
        construction_params,
        planets,
        planet_entities,
        hull_registry,
        module_registry,
        design_registry,
        building_registry,
        job_registry,
        colony_panel_tab,
        dispatches,
    );
}

// ---------------------------------------------------------------------------
// Left panel: system information
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_left_panel(
    ui: &mut egui::Ui,
    sel_entity: Entity,
    star: &StarSystem,
    star_pos: &Position,
    is_local_system: bool,
    effective_surveyed: bool,
    k_data: Option<&crate::knowledge::SystemKnowledge>,
    knowledge: &KnowledgeStore,
    clock: &GameClock,
    player_q: &Query<(Entity, &StationedAt, Option<&AboardShip>), With<Player>>,
    positions: &Query<&Position>,
    system_planets: &[(Entity, String, String, bool, Option<f64>)],
    selected_planet: &mut SelectedPlanet,
    stockpile_info: &Option<(Amt, Amt, Amt, Amt)>,
    anomalies_q: &Query<&crate::galaxy::Anomalies>,
    deliverable_stockpiles: &Query<&DeliverableStockpile, With<StarSystem>>,
    selected_ship_docked_here: Option<Entity>,
    system_actions_out: &mut SystemPanelActions,
) {
    // --- Survey & Distance ---
    ui.label(egui::RichText::new("System Info").strong().color(egui::Color32::from_rgb(180, 180, 220)));
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

    if !effective_surveyed {
        ui.label("Approximate position only. Survey required.");
    }

    if let Some(perceived) = crate::knowledge::perceived_system(knowledge, sel_entity, clock.elapsed) {
        let age = perceived.age(clock.elapsed);
        let years = age as f64 / HEXADIES_PER_YEAR as f64;
        let freshness = if age < 60 {
            "FRESH"
        } else if age < 300 {
            "AGING"
        } else if age < crate::knowledge::STALE_THRESHOLD_HEXADIES {
            "OLD"
        } else {
            "VERY OLD"
        };
        let source_tag = observation_source_tag(perceived.source);
        let color = observation_freshness_color(age);
        ui.label(
            egui::RichText::new(format!(
                "Info age: {} hd ({:.1} yr) [{}] {}",
                age, years, freshness, source_tag
            ))
            .color(color),
        );
    }

    // --- Resource stockpile ---
    if let Some((minerals, energy, food, authority)) = stockpile_info {
        ui.separator();
        ui.label(egui::RichText::new("System Stockpile").strong().color(egui::Color32::from_rgb(180, 180, 220)));
        ui.label(format!("Minerals: {}", minerals.display_compact()));
        ui.label(format!("Energy:   {}", energy.display_compact()));
        ui.label(format!("Food:     {}", food.display_compact()));
        ui.label(format!("Authority:{}", authority.display_compact()));
    }

    // #229: Deliverable Stockpile — shipyard-built deliverables waiting to
    // be loaded onto a ship. Only shown when non-empty. When a ship is
    // selected and docked at THIS system, each row shows a [Load] button
    // that queues a `LoadDeliverable` on the ship.
    if let Ok(stockpile) = deliverable_stockpiles.get(sel_entity) {
        if !stockpile.items.is_empty() {
            ui.separator();
            ui.label(
                egui::RichText::new("Deliverable Stockpile")
                    .strong()
                    .color(egui::Color32::from_rgb(180, 220, 180)),
            );
            for (i, item) in stockpile.items.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("  #{}: {}", i, item.definition_id()));
                    if let Some(ship_e) = selected_ship_docked_here {
                        if ui.small_button("Load").clicked() {
                            system_actions_out.load_deliverable =
                                Some((ship_e, sel_entity, i));
                        }
                    }
                });
            }
            if selected_ship_docked_here.is_none() {
                ui.label(
                    egui::RichText::new(
                        "(Select a ship docked here to Load a deliverable.)",
                    )
                    .small()
                    .italics()
                    .weak(),
                );
            }
        }
    }

    // #176: Remote system knowledge summary
    if !is_local_system {
        if let Some(k) = k_data {
            let snap = &k.data;
            ui.separator();
            ui.label(egui::RichText::new("Remote Intelligence").strong()
                .color(egui::Color32::from_rgb(200, 180, 100)));
            ui.label(egui::RichText::new("(light-speed delayed)").weak().small());
            if snap.colonized {
                ui.label(format!("M {} | E {} | F {} | A {}",
                    snap.minerals.display_compact(), snap.energy.display_compact(),
                    snap.food.display_compact(), snap.authority.display_compact()));
                if snap.production_minerals > Amt::ZERO || snap.production_energy > Amt::ZERO
                    || snap.production_food > Amt::ZERO || snap.production_research > Amt::ZERO
                {
                    ui.label(egui::RichText::new("Production/hd:").strong());
                    ui.label(format!("M {} | E {} | F {} | R {}",
                        snap.production_minerals.display_compact(), snap.production_energy.display_compact(),
                        snap.production_food.display_compact(), snap.production_research.display_compact()));
                }
                if snap.maintenance_energy > Amt::ZERO {
                    ui.label(format!("Maintenance: E {}/hd", snap.maintenance_energy.display_compact()));
                }
            }
            if snap.has_hostile {
                // #215: tag hostile observation with source + freshness colouring
                // so the player can judge how trustworthy the reading is.
                let age = clock.elapsed - k.observed_at;
                let overlay_source = if age >= crate::knowledge::STALE_THRESHOLD_HEXADIES {
                    crate::knowledge::ObservationSource::Stale
                } else {
                    k.source
                };
                let source_tag = observation_source_tag(overlay_source);
                // Tint the hostile red toward grey as the observation ages so
                // fresh threats pop while stale sightings are visibly dimmed.
                let color = if age < 60 {
                    egui::Color32::from_rgb(255, 100, 100)
                } else if age < 300 {
                    egui::Color32::from_rgb(220, 120, 120)
                } else if age < crate::knowledge::STALE_THRESHOLD_HEXADIES {
                    egui::Color32::from_rgb(180, 110, 110)
                } else {
                    egui::Color32::from_rgb(140, 80, 70)
                };
                ui.label(
                    egui::RichText::new(format!(
                        "Hostile presence (str: {:.1}) {}",
                        snap.hostile_strength, source_tag
                    ))
                    .color(color),
                );
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

    // --- Anomalies ---
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

    // --- Planet list ---
    if !system_planets.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("Planets").strong().color(egui::Color32::from_rgb(180, 180, 220)));
        for (planet_entity, name, planet_type, is_colonized, hab) in system_planets {
            let is_selected = selected_planet.0 == Some(*planet_entity);
            let label_text = format!(
                "{} ({}){}",
                name,
                format_planet_type(planet_type),
                if *is_colonized { " [col]" } else { "" },
            );
            let mut label = egui::RichText::new(&label_text);
            if *is_colonized {
                label = label.color(egui::Color32::from_rgb(100, 200, 100));
            }
            if ui.selectable_label(is_selected, label).clicked() {
                selected_planet.0 = Some(*planet_entity);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Right panel: actions, ships, buildings, colonization
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_right_panel(
    ui: &mut egui::Ui,
    sel_entity: Entity,
    star_pos: &Position,
    selected_ship: &mut SelectedShip,
    docked_ships: &[(Entity, String, String)],
    hull_registry: &crate::ship_design::HullRegistry,
    module_registry: &crate::ship_design::ModuleRegistry,
    design_registry: &crate::ship_design::ShipDesignRegistry,
    system_buildings_q: &mut Query<(Option<&mut SystemBuildings>, Option<&mut SystemBuildingQueue>)>,
    construction_params: &ConstructionParams,
    building_registry: &BuildingRegistry,
    colonized_planets: &std::collections::HashSet<Entity>,
    planet_entities: &Query<(Entity, &Planet, Option<&SystemAttributes>)>,
    planets: &Query<&Planet>,
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
    colonization_queues: &Query<&ColonizationQueue>,
    colonization_actions_out: &mut Vec<ColonizationAction>,
    deep_space_structures: &Query<(
        Entity,
        &DeepSpaceStructure,
        &Position,
        Option<&ConstructionPlatform>,
        Option<&Scrapyard>,
    )>,
    structure_registry: &StructureRegistry,
    deliverable_avail: &DeliverableAvailabilityCtx<'_>,
    system_actions_out: &mut SystemPanelActions,
    dispatches: &mut crate::communication::PendingColonyDispatches,
    remote_commands: &Query<&crate::communication::PendingCommand>,
    clock_elapsed: i64,
) {
    draw_in_flight_commands_section(ui, sel_entity, remote_commands, clock_elapsed);

    // === Docked Ships ===
    ui.label(egui::RichText::new("Docked Ships").strong().color(egui::Color32::from_rgb(180, 180, 220)));
    ui.separator();
    if docked_ships.is_empty() {
        ui.label(egui::RichText::new("No ships docked").weak().italics());
    } else {
        for (entity, name, design_id) in docked_ships {
            let is_selected = selected_ship.0 == Some(*entity);
            let design_name = design_registry.get(design_id).map(|d| d.name.as_str()).unwrap_or(design_id);
            let label = format!(
                "{} ({}){}",
                name,
                design_name,
                if is_selected { " [sel]" } else { "" }
            );
            if ui.selectable_label(is_selected, &label).clicked() {
                selected_ship.0 = Some(*entity);
            }
        }
    }

    // === System Buildings ===
    if let Ok((Some(sys_bldgs), sys_bldg_queue)) = system_buildings_q.get_mut(sel_entity) {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("System Buildings").strong().color(egui::Color32::from_rgb(180, 180, 220)));
        ui.separator();

        let mut sys_demolish_request: Option<(usize, BuildingId)> = None;
        let mut sys_upgrade_request: Option<(usize, String, Amt, Amt, i64)> = None;
        let sys_bldg_cost_mod = construction_params.building_cost_modifier.final_value();
        let sys_bldg_time_mod = construction_params.building_build_time_modifier.final_value();

        // #260: Collect pending system-building construction orders so empty
        // slots can show their in-progress target + progress bar. Mirrors the
        // planet-side logic in `colony_detail::draw_colony_detail`. Kept inline
        // (no shared helper yet) to minimise churn — see issue for the
        // follow-up on extracting a common row renderer.
        let sys_pending_orders: Vec<(usize, String, f32)> = sys_bldg_queue
            .as_ref()
            .map(|bq| {
                bq.queue
                    .iter()
                    .map(|order| {
                        let def = building_registry.get(order.building_id.as_str());
                        let (total_m, total_e) =
                            def.map(|d| d.build_cost()).unwrap_or((Amt::ZERO, Amt::ZERO));
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
                        let name = def
                            .map(|d| d.name.clone())
                            .unwrap_or_else(|| order.building_id.0.clone());
                        (order.target_slot, name, pct)
                    })
                    .collect()
            })
            .unwrap_or_default();

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
                        "[{}] {} — Demolishing ({} hd)",
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
                        "[{}] {} -> {} ({} hd)",
                        i, name, target_name, remaining
                    ));
                }
                Some(bid) => {
                    let def = building_registry.get(bid.as_str());
                    let name = def.map(|d| d.name.as_str()).unwrap_or(bid.as_str());
                    let (m_refund, e_refund) = def.map(|d| d.demolition_refund()).unwrap_or((Amt::ZERO, Amt::ZERO));
                    let demo_time = def.map(|d| d.demolition_time()).unwrap_or(0);
                    ui.horizontal(|ui| {
                        ui.label(format!("[{}] {}", i, name));
                        let tooltip = format!(
                            "Demolish: {} hd | Refund M:{} E:{}",
                            demo_time, m_refund.display_compact(), e_refund.display_compact()
                        );
                        // #260: `"Demolish"` matches the planet-side label so
                        // the button is discoverable. The previous `"X"` was
                        // effectively hidden.
                        if ui
                            .small_button("Demolish")
                            .on_hover_text(tooltip)
                            .clicked()
                        {
                            sys_demolish_request = Some((i, bid.clone()));
                        }
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
                                    target_name, eff_m.display_compact(), eff_e.display_compact(), eff_time
                                );
                                if ui
                                    .small_button(format!("-> {}", target_name))
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
                    // #260: Show the in-progress order on an empty slot, same
                    // way `colony_detail` does for planet buildings. Without
                    // this the player has no feedback after queueing a
                    // shipyard/port — the slot stays blank until completion.
                    if let Some((_, name, pct)) =
                        sys_pending_orders.iter().find(|(s, _, _)| *s == i)
                    {
                        ui.horizontal(|ui| {
                            ui.label(format!("[{}] (Building: {})", i, name));
                            let bar = egui::ProgressBar::new(*pct).desired_width(80.0);
                            ui.add(bar);
                        });
                    } else {
                        ui.label(format!("[{}] (empty)", i));
                    }
                }
            }
        }

        if let Some((slot_idx, _bid)) = sys_demolish_request {
            dispatches.queue.push(crate::communication::PendingColonyDispatch {
                target_system: sel_entity,
                command: crate::communication::ColonyCommand {
                    target_planet: None,
                    kind: crate::communication::ColonyCommandKind::DemolishBuilding {
                        target_slot: slot_idx,
                    },
                },
            });
        }
        if let Some((slot_idx, target_id, _, _, _)) = sys_upgrade_request {
            dispatches.queue.push(crate::communication::PendingColonyDispatch {
                target_system: sel_entity,
                command: crate::communication::ColonyCommand {
                    target_planet: None,
                    kind: crate::communication::ColonyCommandKind::UpgradeBuilding {
                        slot_index: slot_idx,
                        target_id,
                    },
                },
            });
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
                    let tooltip = format!("M:{} E:{} | {} hexadies", eff_m.display_compact(), eff_e.display_compact(), eff_time);
                    if ui.button(&def.name).on_hover_text(tooltip).clicked() {
                        build_sys_building_request = Some(BuildingId::new(&def.id));
                    }
                }
                if let Some(bid) = build_sys_building_request {
                    dispatches.queue.push(crate::communication::PendingColonyDispatch {
                        target_system: sel_entity,
                        command: crate::communication::ColonyCommand {
                            target_planet: None,
                            kind: crate::communication::ColonyCommandKind::QueueBuilding {
                                building_id: bid.0,
                                target_slot: slot_idx,
                            },
                        },
                    });
                }
            }
        }
    }

    // === #134: Ship Build Queue + Build Ship (system-level) ===
    {
        // Determine shipyard availability via capability check on system buildings.
        let has_shipyard = system_buildings_q
            .get(sel_entity)
            .ok()
            .and_then(|(sb, _)| sb.map(|sb| sb.has_shipyard(building_registry)))
            .unwrap_or(false);

        // Collect colonies in this system along with a snapshot of their build queues.
        // Also remember the first colony entity, which we will use as the host for
        // newly enqueued ship orders (BuildQueue is per-colony but ship construction
        // is gated by system-level shipyard).
        let mut host_colony: Option<Entity> = None;
        let mut queue_snapshots: Vec<(String, Amt, Amt, Amt, Amt, i64, i64)> = Vec::new();
        for (colony_entity, colony, _prod, build_queue, _b, _bq, _m, _f) in colonies.iter() {
            if colony.system(planets) != Some(sel_entity) {
                continue;
            }
            if host_colony.is_none() {
                host_colony = Some(colony_entity);
            }
            if let Some(bq) = build_queue {
                for order in &bq.queue {
                    queue_snapshots.push((
                        order.display_name.clone(),
                        order.minerals_invested,
                        order.minerals_cost,
                        order.energy_invested,
                        order.energy_cost,
                        order.build_time_remaining,
                        order.build_time_total,
                    ));
                }
            }
        }

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Ship Construction")
                .strong()
                .color(egui::Color32::from_rgb(180, 180, 220)),
        );
        ui.separator();

        if host_colony.is_none() {
            ui.label(
                egui::RichText::new("(No colony in this system)")
                    .weak()
                    .italics(),
            );
        } else if !has_shipyard {
            ui.label(
                egui::RichText::new("Shipyard required to build ships")
                    .color(egui::Color32::from_rgb(220, 160, 100)),
            );
        }

        // --- Build Queue (combined across colonies in this system) ---
        ui.label(egui::RichText::new("Build Queue").strong());
        if queue_snapshots.is_empty() {
            ui.label("[empty]");
        } else {
            for (name, m_inv, m_cost, e_inv, e_cost, time_rem, time_total) in &queue_snapshots {
                let m_pct = if m_cost.raw() > 0 {
                    (m_inv.raw() as f32 / m_cost.raw() as f32).min(1.0)
                } else {
                    1.0
                };
                let e_pct = if e_cost.raw() > 0 {
                    (e_inv.raw() as f32 / e_cost.raw() as f32).min(1.0)
                } else {
                    1.0
                };
                let time_pct = if *time_total > 0 {
                    ((*time_total - *time_rem) as f32 / *time_total as f32).min(1.0)
                } else {
                    1.0
                };
                let pct = m_pct.min(e_pct).min(time_pct);
                ui.horizontal(|ui| {
                    ui.label(name);
                    let bar = egui::ProgressBar::new(pct).desired_width(100.0);
                    ui.add(bar);
                    if m_pct < 1.0 || e_pct < 1.0 {
                        ui.label(egui::RichText::new("(awaiting resources)").weak().small());
                    } else if time_pct < 1.0 {
                        ui.label(egui::RichText::new(format!("{} hd", time_rem)).weak().small());
                    }
                });
            }
        }

        // --- Build buttons (only if a shipyard is present and a host colony exists) ---
        if has_shipyard {
            if let Some(host) = host_colony {
                let ship_mod = construction_params.ship_cost_modifier.final_value();
                let ship_time_mod = construction_params.ship_build_time_modifier.final_value();
                let mut build_request: Option<(String, String, Amt, Amt, i64)> = None;

                let design_ids = design_registry.all_design_ids();
                if !design_ids.is_empty() {
                    ui.label(egui::RichText::new("Build Ship").strong());
                    egui::ScrollArea::horizontal()
                        .id_salt("system_panel_build_ship")
                        .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for design_id in &design_ids {
                                let design = &design_registry.designs[design_id];
                                let hull = hull_registry.get(&design.hull_id);
                                let (base_m, base_e, base_time) = if let Some(hull) = hull {
                                    let mods: Vec<_> = design
                                        .modules
                                        .iter()
                                        .filter_map(|a| module_registry.get(&a.module_id))
                                        .collect();
                                    let (m, e, t, _maint) =
                                        crate::ship_design::design_cost(hull, &mods);
                                    (m, e, t)
                                } else {
                                    (
                                        design.build_cost_minerals,
                                        design.build_cost_energy,
                                        design.build_time,
                                    )
                                };
                                let eff_m = base_m.mul_amt(ship_mod);
                                let eff_e = base_e.mul_amt(ship_mod);
                                let eff_time =
                                    (base_time as f64 * ship_time_mod.to_f64()).ceil() as i64;
                                let tooltip =
                                    format!("M:{} E:{} | {} hd", eff_m.display_compact(), eff_e.display_compact(), eff_time);
                                if ui.button(&design.name).on_hover_text(tooltip).clicked() {
                                    build_request = Some((
                                        design_id.clone(),
                                        design.name.clone(),
                                        eff_m,
                                        eff_e,
                                        eff_time,
                                    ));
                                }
                            }
                        });
                    });
                }

                if let Some((design_id, _display_name, _minerals_cost, _energy_cost, _build_time)) =
                    build_request
                {
                    dispatches.queue.push(crate::communication::PendingColonyDispatch {
                        target_system: sel_entity,
                        command: crate::communication::ColonyCommand {
                            target_planet: None,
                            kind: crate::communication::ColonyCommandKind::QueueShipBuild {
                                host_colony: host,
                                design_id,
                                build_kind: crate::colony::BuildKind::Ship,
                            },
                        },
                    });
                }

                // #229: Deliverables — shipyard-buildable deep-space structures.
                // Filtered by prerequisites against the current empire state so
                // e.g. the Sensor Buoy only appears once its gating tech
                // (if any) is researched. Successful clicks push a BuildOrder
                // with `kind = BuildKind::Deliverable { cargo_size }` so the
                // building_queue tick routes the completed item to the
                // DeliverableStockpile instead of spawning a Ship entity.
                let available = available_shipyard_deliverables(
                    structure_registry,
                    deliverable_avail,
                );
                if !available.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("Deliverables").strong());
                    let mut deliverable_request: Option<(String, String, u32, Amt, Amt, i64)> =
                        None;
                    egui::ScrollArea::horizontal()
                        .id_salt("system_panel_build_deliverable")
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (def_id, def_name) in &available {
                                    let Some(def) = structure_registry.get(def_id) else {
                                        continue;
                                    };
                                    let Some(meta) = def.deliverable.as_ref() else {
                                        continue;
                                    };
                                    let eff_m = meta.cost.minerals.mul_amt(ship_mod);
                                    let eff_e = meta.cost.energy.mul_amt(ship_mod);
                                    let eff_time = (meta.build_time as f64
                                        * ship_time_mod.to_f64())
                                    .ceil() as i64;
                                    let tooltip = format!(
                                        "M:{} E:{} | {} hd | cargo {}",
                                        eff_m.display_compact(),
                                        eff_e.display_compact(),
                                        eff_time,
                                        meta.cargo_size,
                                    );
                                    if ui.button(*def_name).on_hover_text(tooltip).clicked() {
                                        deliverable_request = Some((
                                            def_id.to_string(),
                                            def_name.to_string(),
                                            meta.cargo_size,
                                            eff_m,
                                            eff_e,
                                            eff_time,
                                        ));
                                    }
                                }
                            });
                        });

                    if let Some((def_id, display_name, cargo_size, m_cost, e_cost, build_time)) =
                        deliverable_request
                    {
                        dispatches.queue.push(crate::communication::PendingColonyDispatch {
                            target_system: sel_entity,
                            command: crate::communication::ColonyCommand {
                                target_planet: None,
                                kind: crate::communication::ColonyCommandKind::QueueDeliverableBuild {
                                    host_colony: host,
                                    def_id,
                                    display_name,
                                    cargo_size,
                                    minerals_cost: m_cost,
                                    energy_cost: e_cost,
                                    build_time,
                                },
                            },
                        });
                    }
                }
            }
        }
    }

    // #229: Deep-Space Structures subsection — lists every structure owned
    // by the player within this system's rough coordinate neighbourhood
    // and exposes a Dismantle button per row. The neighbourhood threshold
    // matches the sublight "same-system" feeling used throughout the UI:
    // structures deep in interstellar space show up in their nearest
    // system view. A future issue may introduce proper system-membership
    // tagging for structures.
    {
        const STRUCTURE_SYSTEM_RADIUS_LY: f64 = 2.0;
        let mut in_system: Vec<(Entity, String, bool, bool)> = Vec::new();
        for (entity, ds, pos, platform, scrap) in deep_space_structures.iter() {
            if pos.distance_to(star_pos) > STRUCTURE_SYSTEM_RADIUS_LY {
                continue;
            }
            in_system.push((
                entity,
                ds.name.clone(),
                platform.is_some(),
                scrap.is_some(),
            ));
        }
        if !in_system.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Deep-Space Structures")
                    .strong()
                    .color(egui::Color32::from_rgb(180, 180, 220)),
            );
            ui.separator();
            for (entity, name, is_platform, is_scrap) in &in_system {
                let state_label = if *is_platform {
                    "[assembling]"
                } else if *is_scrap {
                    "[scrapyard]"
                } else {
                    "[active]"
                };
                ui.horizontal(|ui| {
                    ui.label(format!("{} {}", name, state_label));
                    // Scrapyards are already dismantled — no sense
                    // dismantling them again. ConstructionPlatforms can be
                    // dismantled (player may want to cancel construction).
                    let can_dismantle = !*is_scrap;
                    if ui
                        .add_enabled(can_dismantle, egui::Button::new("Dismantle"))
                        .on_disabled_hover_text("already a Scrapyard")
                        .clicked()
                    {
                        system_actions_out.dismantle = Some(*entity);
                    }
                });
            }
        }
    }

    // === #114: Same-system colonization ===
    if !colonized_planets.is_empty() {
        let mut colonizable: Vec<(Entity, String, String)> = Vec::new();
        for (pe, planet, attrs) in planet_entities.iter() {
            if planet.system == sel_entity
                && !colonized_planets.contains(&pe)
                && attrs.map(|a| {
                    is_colonizable(a.habitability)
                }).unwrap_or(false)
            {
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
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Colonize Planet").strong().color(egui::Color32::from_rgb(180, 180, 220)));
            ui.separator();
            ui.label(egui::RichText::new(format!(
                "Cost: {} M, {} E, {} hd",
                crate::colony::COLONIZATION_MINERAL_COST,
                crate::colony::COLONIZATION_ENERGY_COST,
                crate::colony::COLONIZATION_BUILD_TIME,
            )).small());

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
}

// ---------------------------------------------------------------------------
// Central panel: system map schematic
// ---------------------------------------------------------------------------

fn draw_system_map(
    ui: &mut egui::Ui,
    star_type: &str,
    system_planets: &[(Entity, String, String, bool, Option<f64>)],
    selected_planet: &mut SelectedPlanet,
    docked_ships: &[(Entity, String, String)],
    design_registry: &crate::ship_design::ShipDesignRegistry,
) {
    // Use the entire available area for the map
    let available = ui.available_size();
    let (response, painter) = ui.allocate_painter(
        egui::vec2(available.x, available.y),
        egui::Sense::click(),
    );
    let rect = response.rect;
    let center = rect.center();

    // Dark space background
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(6, 6, 14));

    // Star color based on type
    let star_color = match star_type {
        t if t.contains("red") => egui::Color32::from_rgb(255, 100, 80),
        t if t.contains("blue") => egui::Color32::from_rgb(130, 180, 255),
        t if t.contains("white") => egui::Color32::from_rgb(240, 240, 255),
        t if t.contains("orange") => egui::Color32::from_rgb(255, 180, 80),
        t if t.contains("brown") || t.contains("dwarf") => egui::Color32::from_rgb(180, 120, 80),
        t if t.contains("neutron") || t.contains("pulsar") => egui::Color32::from_rgb(200, 200, 255),
        _ => egui::Color32::from_rgb(255, 220, 80), // Yellow default
    };

    // Draw star glow
    let star_radius = 20.0;
    painter.circle_filled(center, star_radius + 8.0, egui::Color32::from_rgba_premultiplied(
        star_color.r(), star_color.g(), star_color.b(), 30,
    ));
    painter.circle_filled(center, star_radius + 4.0, egui::Color32::from_rgba_premultiplied(
        star_color.r(), star_color.g(), star_color.b(), 60,
    ));
    painter.circle_filled(center, star_radius, star_color);
    painter.circle_stroke(center, star_radius, egui::Stroke::new(1.0, egui::Color32::from_rgb(
        (star_color.r() as u16 * 3 / 4) as u8,
        (star_color.g() as u16 * 3 / 4) as u8,
        (star_color.b() as u16 * 3 / 4) as u8,
    )));

    // Star label
    painter.text(
        egui::pos2(center.x, center.y + star_radius + 6.0),
        egui::Align2::CENTER_TOP,
        format_star_type(star_type),
        egui::FontId::proportional(11.0),
        egui::Color32::from_rgb(200, 200, 200),
    );

    // Scale orbits to fit
    let max_orbit_radius = (rect.width().min(rect.height()) / 2.0 - 50.0).max(60.0);
    let planet_count = system_planets.len();
    let orbit_spacing = if planet_count > 1 {
        (max_orbit_radius - 50.0) / (planet_count as f32)
    } else {
        max_orbit_radius * 0.5
    };

    // Draw planets on orbital rings
    for (i, (planet_entity, name, _planet_type, is_colonized, hab)) in system_planets.iter().enumerate() {
        let orbit_r = 50.0 + (i as f32) * orbit_spacing;

        // Orbit ring
        painter.circle_stroke(
            center,
            orbit_r,
            egui::Stroke::new(0.5, egui::Color32::from_rgba_premultiplied(80, 80, 120, 50)),
        );

        // Planet position (spread around orbit)
        let angle = (i as f32) * 2.1 + 0.5;
        let px = center.x + orbit_r * angle.cos();
        let py = center.y + orbit_r * angle.sin();
        let planet_pos = egui::pos2(px, py);

        // Planet color based on habitability
        let planet_color = match hab {
            Some(h) if *h >= 0.9 => egui::Color32::from_rgb(50, 200, 50),
            Some(h) if *h >= 0.6 => egui::Color32::from_rgb(150, 200, 50),
            Some(h) if *h >= 0.3 => egui::Color32::from_rgb(200, 150, 50),
            Some(h) if *h > 0.0 => egui::Color32::from_rgb(130, 130, 130),
            Some(_) => egui::Color32::from_rgb(200, 130, 80),
            None => egui::Color32::from_rgb(100, 100, 100),
        };

        let planet_radius = match hab {
            Some(h) if *h == 0.0 => 14.0,
            _ => 9.0,
        };

        // Selected planet highlight
        let is_selected = selected_planet.0 == Some(*planet_entity);
        if is_selected {
            painter.circle_filled(planet_pos, planet_radius + 5.0, egui::Color32::from_rgba_premultiplied(255, 255, 100, 40));
            painter.circle_stroke(planet_pos, planet_radius + 5.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 255, 100)));
        }

        // Planet body
        painter.circle_filled(planet_pos, planet_radius, planet_color);

        // Colonized indicator ring
        if *is_colonized {
            painter.circle_stroke(planet_pos, planet_radius + 2.5, egui::Stroke::new(2.0, egui::Color32::from_rgb(50, 130, 255)));
        }

        // Planet name label
        painter.text(
            egui::pos2(px, py + planet_radius + 5.0),
            egui::Align2::CENTER_TOP,
            name,
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(200, 200, 200),
        );

        // Habitability label below name
        if let Some(hab) = hab {
            let hab_str = match hab {
                _ if *hab >= 0.9 => "Ideal",
                _ if *hab >= 0.6 => "Adequate",
                _ if *hab >= 0.3 => "Marginal",
                _ if *hab > 0.0 => "Barren",
                _ => "Uninhabitable",
            };
            painter.text(
                egui::pos2(px, py + planet_radius + 18.0),
                egui::Align2::CENTER_TOP,
                hab_str,
                egui::FontId::proportional(9.0),
                egui::Color32::from_rgb(140, 140, 160),
            );
        }

        // Click detection on this planet
        let click_radius = planet_radius + 8.0;
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

    // Draw docked ships near the star
    if !docked_ships.is_empty() {
        let ship_area_y = center.y - star_radius - 30.0;
        let ship_count = docked_ships.len();
        let ship_spacing = 60.0_f32.min(rect.width() * 0.6 / ship_count.max(1) as f32);
        let start_x = center.x - (ship_count as f32 - 1.0) * ship_spacing / 2.0;

        for (i, (_entity, name, design_id)) in docked_ships.iter().enumerate() {
            let sx = start_x + i as f32 * ship_spacing;
            let sy = ship_area_y;

            // Ship triangle
            let ship_size = 6.0;
            let points = vec![
                egui::pos2(sx, sy - ship_size),
                egui::pos2(sx - ship_size * 0.6, sy + ship_size * 0.5),
                egui::pos2(sx + ship_size * 0.6, sy + ship_size * 0.5),
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                egui::Color32::from_rgb(100, 180, 255),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 120, 200)),
            ));

            // Ship name
            let design_name = design_registry.get(design_id).map(|d| d.name.as_str()).unwrap_or(design_id);
            painter.text(
                egui::pos2(sx, sy + ship_size + 3.0),
                egui::Align2::CENTER_TOP,
                format!("{}\n({})", name, design_name),
                egui::FontId::proportional(9.0),
                egui::Color32::from_rgb(150, 180, 220),
            );
        }
    }

    // Instructions at bottom
    let hint_pos = egui::pos2(rect.center().x, rect.max.y - 16.0);
    painter.text(
        hint_pos,
        egui::Align2::CENTER_BOTTOM,
        "Click a planet to view details",
        egui::FontId::proportional(11.0),
        egui::Color32::from_rgb(120, 120, 140),
    );
}

fn draw_in_flight_commands_section(
    ui: &mut egui::Ui,
    sel_entity: Entity,
    remote_commands: &Query<&crate::communication::PendingCommand>,
    clock_elapsed: i64,
) {
    let mut items: Vec<&crate::communication::PendingCommand> = remote_commands
        .iter()
        .filter(|cmd| cmd.target_system == sel_entity)
        .collect();
    if items.is_empty() {
        return;
    }
    items.sort_by_key(|c| c.arrives_at);

    ui.label(
        egui::RichText::new("In-flight Commands")
            .strong()
            .color(egui::Color32::from_rgb(240, 200, 120)),
    );
    ui.separator();
    for cmd in items {
        let remaining = (cmd.arrives_at - clock_elapsed).max(0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("• {}", cmd.command.describe())).weak());
            ui.label(
                egui::RichText::new(format!("arrives in {} hd", remaining))
                    .italics()
                    .color(egui::Color32::from_rgb(180, 180, 180)),
            );
        });
    }
    ui.add_space(4.0);
    ui.separator();
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

/// #134: Determine whether the given system can build ships from the system panel.
/// Returns the colony entity that should host any new ship build orders, or None
/// when the system cannot build ships (no shipyard or no colony in the system).
///
/// This mirrors the logic used by the right-pane Build Ship UI and is exposed so
/// regression tests can verify it without needing an egui context.
pub fn ship_build_host_colony(
    system_entity: Entity,
    system_buildings: &SystemBuildings,
    building_registry: &BuildingRegistry,
    colonies: &[(Entity, Entity)], // (colony_entity, system_entity) pairs
) -> Option<Entity> {
    if !system_buildings.has_shipyard(building_registry) {
        return None;
    }
    colonies
        .iter()
        .find(|(_, sys)| *sys == system_entity)
        .map(|(colony, _)| *colony)
}

// ---------------------------------------------------------------------------
// #215: Observation-source / freshness visuals
// ---------------------------------------------------------------------------

/// Short tag displayed next to an observation, identifying its channel.
fn observation_source_tag(source: crate::knowledge::ObservationSource) -> &'static str {
    use crate::knowledge::ObservationSource;
    match source {
        ObservationSource::Direct => "[DIR]",
        ObservationSource::Relay => "[REL]",
        ObservationSource::Scout => "[SCT]",
        ObservationSource::Stale => "[STALE]",
    }
}

/// Freshness colour for the "info age" line. Fresher observations render in
/// the default text colour; older ones shade toward grey / red-brown as they
/// approach the [`crate::knowledge::STALE_THRESHOLD_HEXADIES`] cutoff.
fn observation_freshness_color(age: i64) -> egui::Color32 {
    if age < 60 {
        egui::Color32::from_rgb(220, 220, 220)
    } else if age < 300 {
        egui::Color32::from_rgb(180, 180, 180)
    } else if age < crate::knowledge::STALE_THRESHOLD_HEXADIES {
        egui::Color32::from_rgb(130, 130, 130)
    } else {
        egui::Color32::from_rgb(160, 90, 70)
    }
}

#[cfg(test)]
mod tests_229 {
    use super::*;
    use crate::condition::{Condition, ConditionAtom};
    use crate::deep_space::{
        DeliverableDefinition, DeliverableMetadata, ResourceCost, StructureRegistry,
    };
    use std::collections::HashSet;

    fn def(
        id: &str,
        name: &str,
        shipyard_buildable: bool,
        prereq_tech: Option<&str>,
    ) -> DeliverableDefinition {
        DeliverableDefinition {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            max_hp: 100.0,
            energy_drain: Amt::ZERO,
            capabilities: Default::default(),
            prerequisites: prereq_tech
                .map(|t| Condition::Atom(ConditionAtom::has_tech(t))),
            deliverable: if shipyard_buildable {
                Some(DeliverableMetadata {
                    cost: ResourceCost::default(),
                    build_time: 10,
                    cargo_size: 1,
                    scrap_refund: 0.5,
                })
            } else {
                None
            },
            upgrade_to: Vec::new(),
            upgrade_from: None,
        }
    }

    fn ctx_with_techs<'a>(
        techs: &'a HashSet<String>,
        mods: &'a HashSet<String>,
        flags: &'a HashSet<String>,
        bldgs: &'a HashSet<String>,
    ) -> DeliverableAvailabilityCtx<'a> {
        DeliverableAvailabilityCtx {
            researched_techs: techs,
            active_modifiers: mods,
            empire_flags: flags,
            empire_buildings: bldgs,
        }
    }

    #[test]
    fn shipyard_deliverables_filters_non_shipyard_defs() {
        // One shipyard-buildable, one world-only (upgrade target).
        let mut reg = StructureRegistry::default();
        reg.insert(def("sensor_buoy", "Sensor Buoy", true, None));
        reg.insert(def("platform_mk2", "Platform Mk II", false, None));

        let empty = HashSet::new();
        let ctx = ctx_with_techs(&empty, &empty, &empty, &empty);
        let available = available_shipyard_deliverables(&reg, &ctx);

        // Only `sensor_buoy` should be buildable at a shipyard; the
        // world-only `platform_mk2` (no `deliverable` metadata) is filtered
        // out.
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].0, "sensor_buoy");
    }

    #[test]
    fn shipyard_deliverables_enforces_prerequisites() {
        // Deliverable gated by a tech that is NOT researched → filtered out.
        let mut reg = StructureRegistry::default();
        reg.insert(def(
            "defense_platform",
            "Defense Platform",
            true,
            Some("weapons_mk1"),
        ));

        let no_techs = HashSet::new();
        let empty = HashSet::new();
        let ctx = ctx_with_techs(&no_techs, &empty, &empty, &empty);
        assert!(
            available_shipyard_deliverables(&reg, &ctx).is_empty(),
            "prereq not met — should be filtered"
        );

        // Once the tech is researched the deliverable appears.
        let mut techs = HashSet::new();
        techs.insert("weapons_mk1".to_string());
        let ctx2 = ctx_with_techs(&techs, &empty, &empty, &empty);
        let available = available_shipyard_deliverables(&reg, &ctx2);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].0, "defense_platform");
    }

    #[test]
    fn shipyard_deliverables_stable_sort_order() {
        let mut reg = StructureRegistry::default();
        reg.insert(def("zebra", "Zebra Buoy", true, None));
        reg.insert(def("alpha", "Alpha Buoy", true, None));
        reg.insert(def("middle", "Mid Buoy", true, None));

        let empty = HashSet::new();
        let ctx = ctx_with_techs(&empty, &empty, &empty, &empty);
        let available = available_shipyard_deliverables(&reg, &ctx);
        let names: Vec<&str> = available.iter().map(|(_, n)| *n).collect();
        assert_eq!(names, vec!["Alpha Buoy", "Mid Buoy", "Zebra Buoy"]);
    }
}
