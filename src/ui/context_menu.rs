use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::Colony;
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::physics;
use crate::player::{AboardShip, Player, StationedAt};
use crate::ship::{Cargo, CommandQueue, QueuedCommand, Ship, ShipHitpoints, ShipState, SurveyData};
use crate::technology::GlobalParams;
use crate::time_system::GameClock;
use crate::visualization::SelectedShip;

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

    // #76: Calculate light-speed delay from player to ship's location.
    // For in-transit ships, the command must also wait for the ship to arrive
    // at its destination (it can't receive commands mid-FTL).
    let command_delay: i64 = {
        let light_delay: i64 = player_q
            .single()
            .ok()
            .and_then(|(_, stationed, _)| {
                let player_pos = positions.get(stationed.system).ok()?;
                let ship_pos = positions.get(origin_system).ok()?;
                let dist = physics::distance_ly(player_pos, ship_pos);
                Some(physics::light_delay_hexadies(dist))
            })
            .unwrap_or(0);

        // For non-docked ships, also account for remaining travel time
        let remaining_travel: i64 = if !is_docked {
            if let Ok((_, _, state, _, _, _)) = ships_query.get(ship_entity) {
                match &*state {
                    ShipState::InFTL { arrival_at, .. } => (*arrival_at - clock.elapsed).max(0),
                    ShipState::SubLight { arrival_at, .. } => (*arrival_at - clock.elapsed).max(0),
                    ShipState::Surveying { completes_at, .. } => (*completes_at - clock.elapsed).max(0),
                    ShipState::Settling { completes_at, .. } => (*completes_at - clock.elapsed).max(0),
                    ShipState::Refitting { completes_at, .. } => (*completes_at - clock.elapsed).max(0),
                    _ => 0,
                }
            } else {
                0
            }
        } else {
            0
        };

        light_delay.max(remaining_travel)
    };

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
            // Non-docked: queue the default action (with delay if remote)
            let qc = QueuedCommand::MoveTo { system: target_entity };
            if command_delay > 0 {
                delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(qc));
            } else {
                queued_command = Some(qc);
            }
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
                let qc = QueuedCommand::MoveTo { system: target_entity };
                if is_docked {
                    if command_delay == 0 {
                        queued_command = Some(qc);
                    } else {
                        delayed_command = Some(crate::ship::ShipCommand::MoveTo { destination: target_entity });
                    }
                } else if command_delay > 0 {
                    // In-transit + remote: delay until command reaches the ship
                    delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(qc));
                } else {
                    queued_command = Some(qc);
                }
                close_menu = true;
            }

            // Survey -- if Explorer + target unsurveyed
            if can_survey {
                let survey_label = if !is_docked || !same_system { format!("{}Survey", queue_prefix) } else { "Survey".to_string() };
                if ui.button(survey_label).clicked() {
                    let qc = QueuedCommand::Survey { system: target_entity };
                    if !is_docked {
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(qc));
                        } else {
                            queued_command = Some(qc);
                        }
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
                        // Docked at different system: queue survey (auto-inserts move)
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(
                                QueuedCommand::Survey { system: target_entity },
                            ));
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
                    let qc = QueuedCommand::Colonize { system: target_entity, planet: None };
                    if !is_docked {
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(qc));
                        } else {
                            queued_command = Some(qc);
                        }
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
                        // Docked at different system: queue colonize (auto-inserts move)
                        if command_delay > 0 {
                            delayed_command = Some(crate::ship::ShipCommand::EnqueueCommand(
                                QueuedCommand::Colonize { system: target_entity, planet: None },
                            ));
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
