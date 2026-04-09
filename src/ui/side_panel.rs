use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildOrder, BuildQueue, BuildingOrder, BuildingQueue, BuildingType, Buildings, Colony, Production, ResourceStockpile};
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

                    // #51: Maintenance cost summary
                    {
                        let mut building_maintenance = 0.0;
                        if let Some(b) = buildings {
                            for slot in &b.slots {
                                if let Some(bt) = slot {
                                    building_maintenance += bt.maintenance_cost();
                                }
                            }
                        }
                        let mut ship_maintenance = 0.0;
                        for (_, ship, state, _) in ships_query.iter() {
                            if let ShipState::Docked { system } = &*state {
                                if *system == colony.system {
                                    ship_maintenance += ship.ship_type.maintenance_cost();
                                }
                            }
                        }
                        let total_maintenance = building_maintenance + ship_maintenance;
                        if total_maintenance > 0.0 {
                            ui.label(format!("Maintenance: {:.1} E/hd", total_maintenance));
                            ui.label(format!("  Ships: {:.1} E/hd", ship_maintenance));
                            ui.label(format!("  Buildings: {:.1} E/hd", building_maintenance));
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
                            ];
                            let mut build_building_request: Option<BuildingType> = None;
                            for bt in &building_types {
                                let (m_cost, e_cost) = bt.build_cost();
                                let time = bt.build_time();
                                let tooltip = format!("M:{:.0} E:{:.0} | {} hexadies", m_cost, e_cost, time);
                                if ui.button(bt.name()).on_hover_text(tooltip).clicked() {
                                    build_building_request = Some(*bt);
                                }
                            }
                            if let Some(bt) = build_building_request {
                                if let Some(mut bq) = building_queue {
                                    let (m_cost, e_cost) = bt.build_cost();
                                    bq.queue.push(BuildingOrder {
                                        building_type: bt,
                                        target_slot: slot_idx,
                                        minerals_remaining: m_cost,
                                        energy_remaining: e_cost,
                                        build_time_remaining: bt.build_time(),
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

/// Draws the floating ship details panel when a ship is selected.
/// #53: Simplified - command buttons moved to context menu
#[allow(clippy::too_many_arguments)]
pub fn draw_ship_panel(
    ctx: &egui::Context,
    selected_ship: &mut SelectedShip,
    ships_query: &mut Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    _clock: &GameClock,
    colonies: &mut Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
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
            ui.label(format!("Status: {}", status));
            if ftl_range > 0.0 {
                ui.label(format!("FTL range: {:.1} ly", ftl_range));
            } else {
                ui.label("No FTL capability");
            }
            ui.label(format!(
                "Sub-light speed: {:.0}% c",
                sublight_speed * 100.0
            ));

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
            }

            if ui.button("Deselect ship").clicked() {
                deselect_ship = true;
            }
        });

    // Apply deselect
    if deselect_ship {
        selected_ship.0 = None;
    }

    // Apply cargo load/unload actions
    let has_cargo_action = cargo_action.load_minerals > 0.0
        || cargo_action.load_energy > 0.0
        || cargo_action.unload_minerals > 0.0
        || cargo_action.unload_energy > 0.0;
    if has_cargo_action {
        if let Some(colony_e) = colony_entity_at_dock {
            // Get mutable access to the colony stockpile
            if let Ok((_, _, _, Some(mut stockpile), _, _, _)) = colonies.get_mut(colony_e) {
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
