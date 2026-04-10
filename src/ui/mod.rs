pub mod bottom_bar;
pub mod outline;
pub mod overlays;
pub mod side_panel;
pub mod top_bar;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};

use crate::colony::{AuthorityParams, BuildQueue, BuildingQueue, Buildings, Colony, ConstructionParams, FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::player::{Player, PlayerEmpire, StationedAt};
use crate::ship::{Cargo, CommandQueue, PendingShipCommand, Ship, ShipHitpoints, ShipState, SurveyData};
use crate::technology::{GlobalParams, ResearchPool, ResearchQueue, TechTree};
use crate::time_system::{GameClock, GameSpeed};
use crate::visualization::{ContextMenu, SelectedPlanet, SelectedShip, SelectedSystem};

/// Resource tracking whether the research overlay is open.
#[derive(Resource, Default)]
pub struct ResearchPanelOpen(pub bool);

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<ResearchPanelOpen>()
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
    mut research_open: ResMut<ResearchPanelOpen>,
    mut selected_system: ResMut<SelectedSystem>,
    selection_state: (ResMut<SelectedShip>, ResMut<ContextMenu>, ResMut<SelectedPlanet>),
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: Query<&StationedAt, With<Player>>,
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
    positions_planets_stockpiles: (Query<&Position>, Query<&Planet>, Query<(Entity, &Planet, Option<&SystemAttributes>)>, Query<(&mut ResourceStockpile, Option<&ResourceCapacity>), With<StarSystem>>),
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
    let (mut selected_ship, mut context_menu, mut selected_planet) = selection_state;
    let (positions, planets, planet_entities, mut system_stockpiles) = positions_planets_stockpiles;
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Ok((knowledge, command_log, global_params, construction_params, tech_tree, research_pool, mut research_queue, authority_params)) =
        empire_q.single_mut()
    else {
        return;
    };

    // Collect resource totals and net income before passing colonies around
    let (total_minerals, total_energy, total_food, total_authority,
         net_minerals, net_energy, net_food, net_authority) = {
        use crate::amount::{Amt, SignedAmt};
        let mut m = Amt::ZERO;
        let mut e = Amt::ZERO;
        let mut f = Amt::ZERO;
        let mut a = Amt::ZERO;
        // Aggregate stockpiles from star systems
        for (stockpile, _) in system_stockpiles.iter() {
            m = m.add(stockpile.minerals);
            e = e.add(stockpile.energy);
            f = f.add(stockpile.food);
            a = a.add(stockpile.authority);
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
    top_bar::draw_top_bar(ctx, &clock, &mut speed, total_minerals, total_energy, total_food, total_authority, net_food, net_energy, net_minerals, net_authority, &mut research_open);

    outline::draw_outline(
        ctx,
        &stars,
        &colonies,
        &ships_query,
        &mut selected_system,
        &mut selected_ship,
        &planets,
    );

    side_panel::draw_system_panel(
        ctx,
        &selected_system,
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
    );

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

    // #76: Collect pending ship commands from context menu (light-speed delay)
    let mut pending_ship_commands = Vec::new();
    // Need a read-only Colony query for context menu colonization check
    let colony_ro: Vec<Colony> = colonies.iter().map(|(_, c, _, _, _, _, _, _)| Colony { planet: c.planet, population: c.population, growth_rate: c.growth_rate }).collect();
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
            if let Some(tech) = tech_tree.get(tech_id) {
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
}
