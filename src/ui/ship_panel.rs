use bevy::prelude::*;
use bevy_egui::egui;

use crate::amount::Amt;
use crate::colony::{BuildQueue, BuildingQueue, Buildings, Colony, ConstructionParams, FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile, SystemBuildings, SystemBuildingQueue};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::physics;
use crate::player::{AboardShip, Player, StationedAt};
use crate::ship::{
    Cargo, CommandQueue, CourierMode, CourierRoute, PendingShipCommand, QueuedCommand,
    RulesOfEngagement, Ship, ShipHitpoints, ShipState, SurveyData,
};
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;
use crate::visualization::{SelectedShip};

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
    /// #117: Add the currently-selected system as a waypoint to the ship's
    /// CourierRoute (creating one if absent).
    pub courier_add_waypoint: Option<(Entity, Entity, CourierMode)>,
    /// #117: Toggle the paused flag on the courier's route.
    pub courier_toggle_pause: Option<Entity>,
    /// #117: Remove the entire CourierRoute from the ship.
    pub courier_clear_route: Option<Entity>,
    /// #117: Change the route's mode.
    pub courier_set_mode: Option<(Entity, CourierMode)>,
}

/// Resolve an Entity to a star system name, falling back to "Unknown".
pub(super) fn system_name(
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

/// Helper to collect ships docked at a given system.
pub(super) fn ships_docked_at(
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
    design_registry: &ShipDesignRegistry,
    clock_elapsed: i64,
    roe_query: &Query<&RulesOfEngagement>,
    positions: &Query<&Position>,
    player_stationed: Option<Entity>,
    player_aboard_ship: Option<Entity>,
    courier_routes: &Query<&CourierRoute>,
    selected_system: Option<Entity>,
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
        let maintenance_cost = design_registry.maintenance(&ship.design_id);
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
            let design_display_name = design_registry.get(&design_id).map(|d| d.name.as_str()).unwrap_or(&design_id);
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

            // #117: Courier route automation panel (couriers only)
            if design_id == "courier_mk1" {
                ui.separator();
                ui.label(egui::RichText::new("Courier Route").strong());
                let route_opt = courier_routes.get(ship_entity).ok();
                let current_mode = route_opt.map(|r| r.mode).unwrap_or(CourierMode::ResourceTransport);

                // Mode selector
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    for mode in [
                        CourierMode::ResourceTransport,
                        CourierMode::KnowledgeRelay,
                        CourierMode::MessageDelivery,
                    ] {
                        let selected = current_mode == mode;
                        if ui.selectable_label(selected, mode.label()).clicked() && !selected {
                            actions.courier_set_mode = Some((ship_entity, mode));
                        }
                    }
                });

                // Waypoints list
                if let Some(route) = route_opt {
                    if route.waypoints.is_empty() {
                        ui.label("(no waypoints)");
                    } else {
                        for (i, wp) in route.waypoints.iter().enumerate() {
                            let name = system_name(*wp, stars);
                            let marker = if i == route.current_index { "->" } else { "  " };
                            ui.label(format!("{} {}. {}", marker, i + 1, name));
                        }
                    }
                    ui.horizontal(|ui| {
                        let label = if route.paused { "Resume Route" } else { "Pause Route" };
                        if ui.button(label).clicked() {
                            actions.courier_toggle_pause = Some(ship_entity);
                        }
                        if ui.button("Stop Route").clicked() {
                            actions.courier_clear_route = Some(ship_entity);
                        }
                    });
                } else {
                    ui.label("(no active route)");
                }

                // Add waypoint button (uses current selection)
                if let Some(sel_sys) = selected_system {
                    let sel_name = system_name(sel_sys, stars);
                    let label = format!("Add waypoint: {}", sel_name);
                    if ui.button(label).clicked() {
                        actions.courier_add_waypoint = Some((ship_entity, sel_sys, current_mode));
                    }
                } else {
                    ui.label(
                        egui::RichText::new("Select a star to add it as a waypoint")
                            .small()
                            .weak(),
                    );
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
                let (refund_m, refund_e) = design_registry.scrap_refund(&design_id, &ship_modules, module_registry);
                let scrap_label = format!("Scrap Ship (+{} M, +{} E)", refund_m, refund_e);
                let response = ui.button(&scrap_label)
                    .on_hover_text("Dismantle this ship and recover 50% of total value (hull + modules)");
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
