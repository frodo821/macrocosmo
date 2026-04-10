pub mod bottom_bar;
pub mod outline;
pub mod overlays;
pub mod side_panel;
pub mod top_bar;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::colony::{AuthorityParams, BuildQueue, BuildingQueue, Buildings, Colony, ColonizationQueue, ConstructionParams, FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile, SystemBuildings, SystemBuildingQueue};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::player::{AboardShip, Player, PlayerEmpire, StationedAt};
use crate::ship::{Cargo, CommandQueue, PendingShipCommand, RulesOfEngagement, Ship, ShipHitpoints, ShipState, SurveyData};
use crate::ship_design::{HullRegistry, ModuleRegistry, ShipDesignRegistry};
use crate::technology::{GlobalParams, ResearchPool, ResearchQueue, TechTree};
use crate::time_system::{GameClock, GameSpeed};
use crate::visualization::{ContextMenu, EguiWantsPointer, SelectedPlanet, SelectedShip, SelectedSystem};

/// Resource tracking whether the research overlay is open.
#[derive(Resource, Default)]
pub struct ResearchPanelOpen(pub bool);

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<ResearchPanelOpen>()
            .init_resource::<overlays::ShipDesignerState>()
            .init_resource::<EguiWantsPointer>()
            .add_systems(EguiPrimaryContextPass, draw_all_ui);
    }
}

/// Single unified UI system. All egui panels must be drawn from the same
/// system to avoid the "available_rect() before Context::run()" panic
/// that occurs when multiple systems try to access EguiContexts concurrently.
#[allow(clippy::too_many_arguments)]
pub fn draw_all_ui(
    mut commands: Commands,
    mut contexts: EguiContexts,
    clock: Res<GameClock>,
    mut speed: ResMut<GameSpeed>,
    overlay_state: (ResMut<ResearchPanelOpen>, ResMut<overlays::ShipDesignerState>, Res<HullRegistry>, Res<ModuleRegistry>, ResMut<ShipDesignRegistry>),
    mut selected_system: ResMut<SelectedSystem>,
    selection_state: (ResMut<SelectedShip>, ResMut<ContextMenu>, ResMut<SelectedPlanet>, ResMut<EguiWantsPointer>, Res<crate::visualization::GalaxyView>, Query<&Window>, Query<(&Camera, &GlobalTransform), With<Camera2d>>),
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: Query<(Entity, &StationedAt, Option<&AboardShip>), With<Player>>,
    mut colonies: Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    mut ships_query: Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    mut command_queues: Query<&mut CommandQueue>,
    pending_commands: Query<&PendingShipCommand>,
    positions_planets_stockpiles: (Query<&Position>, Query<&Planet>, Query<(Entity, &Planet, Option<&SystemAttributes>)>, Query<(&mut ResourceStockpile, Option<&ResourceCapacity>), With<StarSystem>>, Query<(Option<&mut SystemBuildings>, Option<&mut SystemBuildingQueue>)>, Query<&ColonizationQueue>, Query<&RulesOfEngagement>, Query<&crate::galaxy::HostilePresence>),
    mut empire_q: Query<
        (
            &KnowledgeStore,
            &CommandLog,
            &GlobalParams,
            &ConstructionParams,
            &TechTree,
            &ResearchPool,
            &mut ResearchQueue,
            &AuthorityParams,
        ),
        With<PlayerEmpire>,
    >,
    mut game_events: MessageWriter<GameEvent>,
) {
    let (mut selected_ship, mut context_menu, mut selected_planet, mut egui_wants_pointer, galaxy_view, windows, camera_q) = selection_state;
    let (mut research_open, mut designer_state, hull_registry, module_registry, mut design_registry) = overlay_state;
    let (positions, planets, planet_entities, mut system_stockpiles, mut system_buildings_q, colonization_queues, roe_query, hostiles_query) = positions_planets_stockpiles;
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // Tell camera_controls whether egui is consuming pointer input this frame
    egui_wants_pointer.0 = ctx.wants_pointer_input();
    let Ok((knowledge, command_log, global_params, construction_params, tech_tree, research_pool, mut research_queue, authority_params)) =
        empire_q.single_mut()
    else {
        return;
    };

    // Collect resource totals using KnowledgeStore (light-speed delayed) + real-time for local system
    // #59: Extract player info for UI
    let player_info = player_q.iter().next().map(|(e, s, a)| (e, s.system, a.map(|ab| ab.ship)));
    let player_system = player_info.map(|(_, sys, _)| sys);
    let player_aboard_ship = player_info.and_then(|(_, _, aboard)| aboard);
    let (total_minerals, total_energy, total_food, total_authority,
         net_minerals, net_energy, net_food, net_authority) = {
        use crate::amount::{Amt, SignedAmt};
        let mut m = Amt::ZERO;
        let mut e = Amt::ZERO;
        let mut f = Amt::ZERO;
        let mut a = Amt::ZERO;

        // Remote systems: use delayed data from KnowledgeStore
        for (_entity, k) in knowledge.iter() {
            if player_system == Some(k.system) {
                continue; // local system added below with real-time data
            }
            let snap = &k.data;
            if snap.colonized {
                m = m.add(snap.minerals);
                e = e.add(snap.energy);
                f = f.add(snap.food);
                a = a.add(snap.authority);
            }
        }

        // Local system: use real-time stockpile
        if let Some(local_sys) = player_system {
            if let Ok((stockpile, _)) = system_stockpiles.get(local_sys) {
                m = m.add(stockpile.minerals);
                e = e.add(stockpile.energy);
                f = f.add(stockpile.food);
                a = a.add(stockpile.authority);
            }
        }
        let mut net_m = SignedAmt::ZERO;
        let mut net_e = SignedAmt::ZERO;
        let mut net_f = SignedAmt::ZERO;
        let mut colony_count: u64 = 0;
        let mut has_capital = false;
        for (_, colony, production, _, _, _, maintenance, food_consumption) in colonies.iter() {
            // Net income calculations
            if let Some(prod) = production {
                // Minerals: just production (no per-tick consumption to subtract)
                net_m = net_m.add(SignedAmt::from_amt(prod.minerals_per_hexadies.final_value()));
                // Energy: production - maintenance
                let energy_prod = SignedAmt::from_amt(prod.energy_per_hexadies.final_value());
                let maint = maintenance.map(|mc| SignedAmt::from_amt(mc.energy_per_hexadies.final_value())).unwrap_or(SignedAmt::ZERO);
                net_e = net_e.add(energy_prod.add(SignedAmt(0 - maint.raw())));
                // Food: production - consumption
                let food_prod = SignedAmt::from_amt(prod.food_per_hexadies.final_value());
                let food_cons = food_consumption.map(|fc| SignedAmt::from_amt(fc.food_per_hexadies.final_value())).unwrap_or(SignedAmt::ZERO);
                net_f = net_f.add(food_prod.add(SignedAmt(0 - food_cons.raw())));
            }
            colony_count += 1;
            // Check if capital
            if let Some(sys) = colony.system(&planets) {
                if let Ok((_, star, _, _)) = stars.get(sys) {
                    if star.is_capital {
                        has_capital = true;
                    }
                }
            }
        }
        // Authority net: production - cost_per_colony * non_capital_count
        let non_capital_count = if has_capital { colony_count.saturating_sub(1) } else { colony_count };
        let auth_prod = SignedAmt::from_amt(authority_params.production.final_value());
        let auth_cost = SignedAmt::from_amt(authority_params.cost_per_colony.final_value().mul_u64(non_capital_count));
        let net_a = auth_prod.add(SignedAmt(0 - auth_cost.raw()));
        (m, e, f, a, net_m, net_e, net_f, net_a)
    };
    top_bar::draw_top_bar(ctx, &clock, &mut speed, total_minerals, total_energy, total_food, total_authority, net_food, net_energy, net_minerals, net_authority, &mut research_open, &mut designer_state);

    outline::draw_outline(
        ctx,
        &stars,
        &colonies,
        &ships_query,
        &mut selected_system,
        &mut selected_ship,
        &planets,
    );

    // Galaxy map tooltips: show info when hovering over stars/ships on the map
    draw_map_tooltips(ctx, &windows, &camera_q, &stars, &ships_query, &planets, &colonies, &clock, &galaxy_view);

    let mut colonization_actions = Vec::new();
    side_panel::draw_system_panel(
        ctx,
        &mut selected_system,
        &mut selected_ship,
        &mut selected_planet,
        &stars,
        &player_q,
        &mut colonies,
        &mut system_stockpiles,
        &mut ships_query,
        &positions,
        knowledge,
        &clock,
        construction_params,
        &planets,
        &planet_entities,
        &mut system_buildings_q,
        &hull_registry,
        &module_registry,
        &design_registry,
        &colonization_queues,
        &mut colonization_actions,
    );

    // #114: Process colonization actions from system panel
    for action in colonization_actions {
        commands.spawn(crate::colony::PendingColonizationOrder {
            system_entity: action.system_entity,
            target_planet: action.target_planet,
            source_colony: action.source_colony,
        });
    }

    let ship_panel_actions = side_panel::draw_ship_panel(
        ctx,
        &mut selected_ship,
        &mut ships_query,
        &clock,
        &mut colonies,
        &mut system_stockpiles,
        &stars,
        &command_queues,
        &planets,
        &pending_commands,
        &hull_registry,
        &module_registry,
        clock.elapsed,
        &roe_query,
        &positions,
        player_system,
        player_aboard_ship,
    );

    // #99: Handle cancel current action (surveying/settling -> docked)
    if ship_panel_actions.cancel_current {
        if let Some(ship_entity) = selected_ship.0 {
            if let Ok((_, _, mut state, _, _, _)) = ships_query.get_mut(ship_entity) {
                let dock_system = match &*state {
                    ShipState::Surveying { target_system, .. } => Some(*target_system),
                    ShipState::Settling { system, .. } => Some(*system),
                    _ => None,
                };
                if let Some(sys) = dock_system {
                    *state = ShipState::Docked { system: sys };
                }
            }
        }
    }

    // #99: Handle cancel individual command from queue
    if let Some(index) = ship_panel_actions.cancel_command_index {
        if let Some(ship_entity) = selected_ship.0 {
            if let Ok(mut queue) = command_queues.get_mut(ship_entity) {
                if index < queue.commands.len() {
                    queue.commands.remove(index);
                }
            }
        }
    }

    // #99: Handle clear all commands from queue
    if ship_panel_actions.clear_commands {
        if let Some(ship_entity) = selected_ship.0 {
            if let Ok(mut queue) = command_queues.get_mut(ship_entity) {
                queue.commands.clear();
            }
        }
    }

    // #79: Handle ship scrapping — despawn entity, refund resources, fire events
    if let Some(scrap) = ship_panel_actions.scrap {
        // Add refund to system stockpile (scrap.colony_entity is now the system entity)
        if let Ok((mut stockpile, _)) = system_stockpiles.get_mut(scrap.colony_entity) {
            stockpile.minerals = stockpile.minerals.add(scrap.minerals_refund);
            stockpile.energy = stockpile.energy.add(scrap.energy_refund);
        }
        // Despawn the ship entity
        commands.entity(scrap.ship_entity).despawn();
        // Fire GameEvent for the event log
        let description = format!(
            "{} scrapped at {} (+{} M, +{} E)",
            scrap.ship_name, scrap.system_name, scrap.minerals_refund, scrap.energy_refund
        );
        game_events.write(GameEvent {
            timestamp: clock.elapsed,
            kind: GameEventKind::ShipScrapped,
            description,
            related_system: None,
        });
    }

    // #98: Handle ship refit — deduct resources, set state to Refitting
    if let Some(refit) = ship_panel_actions.refit {
        // Deduct resources from system stockpile
        if let Ok((mut stockpile, _)) = system_stockpiles.get_mut(refit.system_entity) {
            stockpile.minerals = stockpile.minerals.sub(refit.cost_minerals);
            stockpile.energy = stockpile.energy.sub(refit.cost_energy);
        }
        // Set ship state to Refitting
        if let Ok((_, _, mut state, _, _, _)) = ships_query.get_mut(refit.ship_entity) {
            *state = ShipState::Refitting {
                system: refit.system_entity,
                started_at: clock.elapsed,
                completes_at: clock.elapsed + refit.refit_time,
                new_modules: refit.new_modules,
            };
        }
    }

    // #57: Handle ROE change — immediate if local, delayed if remote
    if let Some((ship_entity, new_roe, delay)) = ship_panel_actions.set_roe {
        if delay == 0 {
            // Local ship: apply immediately
            commands.entity(ship_entity).insert(new_roe);
        } else {
            // Remote ship: send as pending command with light-speed delay
            commands.spawn(PendingShipCommand {
                ship: ship_entity,
                command: crate::ship::ShipCommand::SetROE { roe: new_roe },
                arrives_at: clock.elapsed + delay,
            });
        }
    }

    // #59: Handle board ship
    if let Some(ship_entity) = ship_panel_actions.board_ship {
        if let Some((player_entity, _, _)) = player_info {
            if let Ok((_, mut ship, _, _, _, _)) = ships_query.get_mut(ship_entity) {
                ship.player_aboard = true;
            }
            commands.entity(player_entity).insert(AboardShip { ship: ship_entity });
        }
    }

    // #59: Handle disembark
    if ship_panel_actions.disembark {
        if let Some((player_entity, _, _)) = player_info {
            if let Some(ship_entity) = selected_ship.0 {
                if let Ok((_, mut ship, state, _, _, _)) = ships_query.get_mut(ship_entity) {
                    ship.player_aboard = false;
                    // StationedAt is already tracking the docked system via update_player_location
                }
            }
            commands.entity(player_entity).remove::<AboardShip>();
        }
    }

    // #76: Collect pending ship commands from context menu (light-speed delay)
    let mut pending_ship_commands = Vec::new();
    // Need a read-only Colony query for context menu colonization check
    let colony_ro: Vec<Colony> = colonies.iter().map(|(_, c, _, _, _, _, _, _)| Colony { planet: c.planet, population: c.population, growth_rate: c.growth_rate }).collect();
    let hostile_systems: std::collections::HashSet<Entity> = hostiles_query.iter().map(|h| h.system).collect();
    side_panel::draw_context_menu(
        ctx,
        &mut context_menu,
        &mut selected_ship,
        &stars,
        &mut ships_query,
        &mut command_queues,
        &positions,
        &clock,
        global_params,
        &player_q,
        &mut pending_ship_commands,
        &colony_ro,
        &planets,
        &planet_entities,
        &hostile_systems,
    );
    // Spawn any delayed commands as entities
    for pending_cmd in pending_ship_commands {
        commands.spawn(pending_cmd);
    }

    bottom_bar::draw_bottom_bar(ctx, command_log, &clock);

    // Find capital system stockpile for upfront cost checks
    let capital_stockpile: Option<(crate::amount::Amt, crate::amount::Amt)> = {
        let mut result = None;
        for (_, star, _, _) in stars.iter() {
            if star.is_capital {
                // Find the star system entity
                for (sys_entity, sys_star, _, _) in stars.iter() {
                    if sys_star.is_capital {
                        if let Ok((s, _)) = system_stockpiles.get(sys_entity) {
                            result = Some((s.minerals, s.energy));
                        }
                        break;
                    }
                }
                break;
            }
        }
        result
    };

    let capital_refs = capital_stockpile
        .as_ref()
        .map(|(m, e)| (m, e));

    let research_action = overlays::draw_overlays(
        ctx,
        &mut research_open,
        tech_tree,
        &research_queue,
        research_pool,
        capital_refs,
        clock.elapsed,
    );

    // Handle research actions that require mutable colony access
    match research_action {
        overlays::ResearchAction::StartResearch(tech_id) => {
            // Deduct upfront costs from capital system stockpile
            if let Some(tech) = tech_tree.get(&tech_id) {
                let mineral_cost = tech.cost.minerals;
                let energy_cost = tech.cost.energy;

                // Find and deduct from capital system
                for (sys_entity, star, _, _) in stars.iter() {
                    if star.is_capital {
                        if let Ok((mut s, _)) = system_stockpiles.get_mut(sys_entity) {
                            s.minerals = s.minerals.sub(mineral_cost);
                            s.energy = s.energy.sub(energy_cost);
                        }
                        break;
                    }
                }

                research_queue.start_research(tech_id);
            }
        }
        overlays::ResearchAction::CancelResearch => {
            research_queue.cancel_research();
        }
        overlays::ResearchAction::None => {}
    }

    // #98: Ship designer overlay
    let designer_action = overlays::draw_ship_designer(
        ctx,
        &mut designer_state,
        &hull_registry,
        &module_registry,
        &design_registry,
    );

    match designer_action {
        overlays::ShipDesignerAction::SaveDesign(design) => {
            info!("Ship design saved: {} ({})", design.name, design.id);
            design_registry.insert(design);
            // Close designer after saving
            designer_state.open = false;
            // Reset state for next design
            designer_state.selected_hull = None;
            designer_state.selected_modules.clear();
            designer_state.design_name.clear();
        }
        overlays::ShipDesignerAction::None => {}
    }
}

/// Draw tooltips when hovering over objects on the galaxy map.
/// Called from draw_all_ui since it has EguiContexts access.
#[allow(clippy::too_many_arguments)]
fn draw_map_tooltips(
    ctx: &egui::Context,
    windows: &Query<&Window>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    ships: &Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>, &ShipHitpoints, Option<&SurveyData>)>,
    planets: &Query<&Planet>,
    colonies: &Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    clock: &GameClock,
    view: &crate::visualization::GalaxyView,
) {
    // Don't show map tooltips if pointer is over an egui area (panel, overlay, etc.)
    if ctx.is_pointer_over_area() {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, global_transform)) = camera_q.single() else {
        return;
    };
    let Ok(world_pos) = camera.viewport_to_world_2d(global_transform, cursor_pos) else {
        return;
    };

    let hover_radius = 15.0_f32;

    // Check for nearest star under cursor
    let mut best_star: Option<(Entity, f32)> = None;
    for (entity, _star, pos, _) in stars.iter() {
        let star_px = bevy::math::Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale);
        let dist = world_pos.distance(star_px);
        if dist < hover_radius {
            if best_star.is_none() || dist < best_star.unwrap().1 {
                best_star = Some((entity, dist));
            }
        }
    }

    // Check for nearest in-transit ship under cursor
    let ship_hover_radius = 12.0_f32;
    let mut best_ship: Option<(Entity, f32)> = None;
    for (entity, _ship, state, _, _, _) in ships.iter() {
        let ship_px = match &*state {
            ShipState::SubLight {
                origin,
                destination,
                departed_at,
                arrival_at,
                ..
            } => {
                let total = (*arrival_at - *departed_at) as f64;
                let elapsed = (clock.elapsed - *departed_at) as f64;
                let t = if total > 0.0 { (elapsed / total).clamp(0.0, 1.0) } else { 1.0 };
                let cx = (origin[0] + (destination[0] - origin[0]) * t) as f32 * view.scale;
                let cy = (origin[1] + (destination[1] - origin[1]) * t) as f32 * view.scale;
                Some(bevy::math::Vec2::new(cx, cy))
            }
            ShipState::InFTL {
                origin_system,
                destination_system,
                departed_at,
                arrival_at,
            } => {
                // Need star positions to interpolate FTL position
                let origin_pos = stars.iter().find(|(e, _, _, _)| *e == *origin_system).map(|(_, _, p, _)| p);
                let dest_pos = stars.iter().find(|(e, _, _, _)| *e == *destination_system).map(|(_, _, p, _)| p);
                if let (Some(op), Some(dp)) = (origin_pos, dest_pos) {
                    let total = (*arrival_at - *departed_at) as f64;
                    let elapsed = (clock.elapsed - *departed_at) as f64;
                    let t = if total > 0.0 { (elapsed / total).clamp(0.0, 1.0) } else { 1.0 };
                    let cx = (op.x + (dp.x - op.x) * t) as f32 * view.scale;
                    let cy = (op.y + (dp.y - op.y) * t) as f32 * view.scale;
                    Some(bevy::math::Vec2::new(cx, cy))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(px) = ship_px {
            let dist = world_pos.distance(px);
            if dist < ship_hover_radius {
                if best_ship.is_none() || dist < best_ship.unwrap().1 {
                    best_ship = Some((entity, dist));
                }
            }
        }
    }

    // Prefer ship tooltip if ship is closer
    if let Some((ship_entity, ship_dist)) = best_ship {
        let star_closer = best_star.is_some_and(|(_, d)| d < ship_dist);
        if !star_closer {
            if let Ok((_, ship, state, _, hp, _)) = ships.get(ship_entity) {
                let design_name = crate::ship::design_preset(&ship.design_id)
                    .map(|p| p.design_name)
                    .unwrap_or(&ship.design_id);
                let status = match &*state {
                    ShipState::Docked { .. } => "Docked",
                    ShipState::SubLight { .. } => "Sub-light",
                    ShipState::InFTL { .. } => "In FTL",
                    ShipState::Surveying { .. } => "Surveying",
                    ShipState::Settling { .. } => "Settling",
                    ShipState::Refitting { .. } => "Refitting",
                };
                egui::Tooltip::always_open(
                    ctx.clone(),
                    egui::LayerId::background(),
                    egui::Id::new("map_ship_tooltip"),
                    egui::PopupAnchor::Pointer,
                )
                .gap(12.0)
                .show(|ui: &mut egui::Ui| {
                    ui.label(egui::RichText::new(&ship.name).strong());
                    ui.label(format!("Design: {}", design_name));
                    ui.label(format!("Status: {}", status));
                    ui.label(format!("HP: {:.0}/{:.0}", hp.hull, hp.hull_max));
                });
            }
            return;
        }
    }

    // Star tooltip
    if let Some((star_entity, _)) = best_star {
        if let Ok((_, star, _, attrs)) = stars.get(star_entity) {
            let planet_count = planets.iter().filter(|p| p.system == star_entity).count();
            let has_colony = colonies.iter().any(|(_, c, _, _, _, _, _, _)| {
                c.system(planets).is_some_and(|sys| sys == star_entity)
            });

            egui::Tooltip::always_open(
                ctx.clone(),
                egui::LayerId::background(),
                egui::Id::new("map_star_tooltip"),
                egui::PopupAnchor::Pointer,
            )
            .gap(12.0)
            .show(|ui: &mut egui::Ui| {
                ui.label(egui::RichText::new(&star.name).strong());
                if star.is_capital {
                    ui.label("Capital system");
                }
                if star.surveyed {
                    ui.label(format!("Planets: {}", planet_count));
                    if let Some(attr) = attrs {
                        ui.label(format!("Habitability: {:?}", attr.habitability));
                    }
                } else {
                    ui.label(egui::RichText::new("Unsurveyed").weak().italics());
                }
                if has_colony {
                    ui.label(egui::RichText::new("Colonized").color(egui::Color32::from_rgb(100, 255, 100)));
                }
            });
        }
    }
}
