use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildOrder, BuildQueue, BuildingOrder, BuildingQueue, BuildingType, Buildings, Colony, ConstructionParams, Production, ResourceStockpile};
use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::ship::{Cargo, CommandQueue, QueuedCommand, Ship, ShipState, ShipType};
use crate::technology::GlobalParams;
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
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
    )>,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    positions: &Query<&Position>,
    knowledge: &KnowledgeStore,
    clock: &GameClock,
    construction_params: &ConstructionParams,
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
                for (_colony_entity, colony, production, stockpile, build_queue, buildings, building_queue) in
                    colonies.iter_mut()
                {
                    if colony.system != sel_entity {
                        continue;
                    }

                    // #69: Show population with carrying capacity
                    let carrying_cap = {
                        use crate::amount::Amt;
                        use crate::galaxy::{BASE_CARRYING_CAPACITY, FOOD_PER_POP_PER_HEXADIES};
                        let hab_score = attrs.map(|a| a.habitability.base_score()).unwrap_or(0.5);
                        let k_habitat = BASE_CARRYING_CAPACITY * hab_score;
                        // Food production including building modifiers
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
                        ui.label(format!(
                            "Production: M {} | E {} | R {} | F {} /hd",
                            prod.minerals_per_hexadies.final_value(),
                            prod.energy_per_hexadies.final_value(),
                            prod.research_per_hexadies.final_value(),
                            prod.food_per_hexadies.final_value(),
                        ));
                    }

                    if let Some(stockpile) = stockpile {
                        ui.label(format!(
                            "Stockpile: F {} | E {} | M {} | A {}",
                            stockpile.food, stockpile.energy, stockpile.minerals, stockpile.authority,
                        ));
                    }

                    // #51/#64: Maintenance cost summary (ships charged via home_port)
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
                            if ship.home_port == colony.system {
                                ship_maintenance = ship_maintenance.add(ship.ship_type.maintenance_cost());
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
                                    ui.label(&order.ship_type_name);
                                    let bar = egui::ProgressBar::new(pct)
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
                        use crate::amount::Amt;
                        let ship_mod = construction_params.ship_cost_modifier.final_value();
                        let ship_time_mod = construction_params.ship_build_time_modifier.final_value();
                        // Base costs per ship type
                        let ships_data: [(&str, Amt, Amt, i64); 3] = [
                            ("Explorer", Amt::units(200), Amt::units(100), 60),
                            ("Colony Ship", Amt::units(500), Amt::units(300), 120),
                            ("Courier", Amt::units(100), Amt::units(50), 30),
                        ];
                        let mut build_request: Option<(&str, Amt, Amt, i64)> = None;
                        ui.horizontal(|ui| {
                            for &(name, base_m, base_e, base_time) in &ships_data {
                                let eff_m = base_m.mul_amt(ship_mod);
                                let eff_e = base_e.mul_amt(ship_mod);
                                let eff_time = (base_time as f64 * ship_time_mod.to_f64()).ceil() as i64;
                                let tooltip = format!("M:{} E:{} | {} hd", eff_m, eff_e, eff_time);
                                if ui.button(name).on_hover_text(tooltip).clicked() {
                                    build_request = Some((name, eff_m, eff_e, eff_time));
                                }
                            }
                        });
                        if let Some((name, minerals_cost, energy_cost, build_time)) = build_request {
                            bq.queue.push(BuildOrder {
                                ship_type_name: name.to_string(),
                                minerals_cost,
                                minerals_invested: Amt::ZERO,
                                energy_cost,
                                energy_invested: Amt::ZERO,
                                build_time_total: build_time,
                                build_time_remaining: build_time,
                            });
                            info!("Build order added: {}", name);
                        }
                    }

                    // #46: Buildings display and construction UI
                    if let Some(buildings) = buildings {
                        ui.separator();
                        ui.label(egui::RichText::new("Buildings").strong());
                        for (i, slot) in buildings.slots.iter().enumerate() {
                            match slot {
                                Some(bt) => {
                                    ui.label(format!("  [{}] {}", i, bt.name()));
                                }
                                None => {
                                    ui.label(format!("  [{}] (empty)", i));
                                }
                            }
                        }

                        // Find first empty slot for building construction
                        let empty_slot = buildings.slots.iter().position(|s| s.is_none());

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
    )>,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    command_queues: &Query<&mut CommandQueue>,
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
        let maintenance_cost = ship.ship_type.maintenance_cost();
        // Check if docked at a system that has a colony (for "Set Home Port" button)
        let docked_at_colony = docked_system.and_then(|dock_sys| {
            colonies.iter().find_map(|(_, col, _, _, _, _, _)| {
                if col.system == dock_sys { Some(dock_sys) } else { None }
            })
        });
        Some((
            ship_entity,
            ship.name.clone(),
            ship.ship_type,
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
        ship_type,
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
        return;
    };

    let mut deselect_ship = false;
    let mut set_home_port: Option<Entity> = None;

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
        colonies.iter().find_map(|(e, col, _, _, _, _, _)| {
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
                if ship_type == ShipType::Courier {
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
    use crate::amount::Amt;
    let has_cargo_action = cargo_action.load_minerals > Amt::ZERO
        || cargo_action.load_energy > Amt::ZERO
        || cargo_action.unload_minerals > Amt::ZERO
        || cargo_action.unload_energy > Amt::ZERO;
    if has_cargo_action {
        if let Some(colony_e) = colony_entity_at_dock {
            if let Ok((_, _, _, Some(mut stockpile), _, _, _)) = colonies.get_mut(colony_e) {
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
}

/// Draws the RTS-style context menu when a ship is selected and a star is clicked.
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
            ship.ship_type,
            ship.ftl_range,
            ship.sublight_speed,
            docked_system,
            current_destination_system,
        )
    };

    let (ship_name, ship_type, ftl_range, sublight_speed, docked_system, current_destination_system) = ship_data;

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
    let target_colonized = target_star.colonized;
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
    let can_survey = is_docked && ship_type == ShipType::Explorer && !target_surveyed;
    let can_colonize = is_docked && ship_type == ShipType::ColonyShip && target_habitable && !target_colonized && target_surveyed && same_system;

    let origin_pos_arr = origin_pos.as_array();
    let target_pos_arr = target_pos.as_array();

    let mut command: Option<ShipState> = None;
    let mut queued_command: Option<QueuedCommand> = None;
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
                command = Some(ShipState::Surveying {
                    target_system: target_entity,
                    started_at: clock.elapsed,
                    completes_at: clock.elapsed + crate::ship::SURVEY_DURATION_HEXADIES,
                });
            } else if can_colonize {
                command = Some(ShipState::Settling {
                    system: target_entity,
                    started_at: clock.elapsed,
                    completes_at: clock.elapsed + crate::ship::SETTLING_DURATION_HEXADIES,
                });
            }
            context_menu.open = false;
            context_menu.target_system = None;
            context_menu.execute_default = false;
            if let Some(new_state) = command {
                if let Ok((_, _, mut state, _)) = ships_query.get_mut(ship_entity) {
                    *state = new_state;
                }
            }
            return;
        } else if is_docked {
            if can_ftl {
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
                let travel_time = physics::sublight_travel_hexadies(dist, sublight_speed);
                command = Some(ShipState::SubLight {
                    origin: origin_pos_arr,
                    destination: target_pos_arr,
                    target_system: Some(target_entity),
                    departed_at: clock.elapsed,
                    arrival_at: clock.elapsed + travel_time,
                });
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
                    let travel_time = physics::sublight_travel_hexadies(dist, sublight_speed);
                    command = Some(ShipState::SubLight {
                        origin: origin_pos_arr,
                        destination: target_pos_arr,
                        target_system: Some(target_entity),
                        departed_at: clock.elapsed,
                        arrival_at: clock.elapsed + travel_time,
                    });
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
                    let survey_time = physics::light_delay_hexadies(dist) * 2 + 5;
                    command = Some(ShipState::Surveying {
                        target_system: target_entity,
                        started_at: clock.elapsed,
                        completes_at: clock.elapsed + survey_time,
                    });
                    close_menu = true;
                }
            }

            // Colonize -- if ColonyShip + target habitable + uncolonized (docked only)
            if can_colonize {
                if ui.button("Colonize").clicked() {
                    command = Some(ShipState::Settling {
                        system: target_entity,
                        started_at: clock.elapsed,
                        completes_at: clock.elapsed + crate::ship::SETTLING_DURATION_HEXADIES,
                    });
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

    // Apply immediate command (docked ships)
    if let Some(new_state) = command {
        if let Ok((_, _, mut state, _)) = ships_query.get_mut(ship_entity) {
            *state = new_state;
            selected_ship.0 = None;
        }
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
