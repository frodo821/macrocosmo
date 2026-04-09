pub mod bottom_bar;
pub mod outline;
pub mod overlays;
pub mod side_panel;
pub mod top_bar;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};

use bevy::ecs::system::SystemParam;

use crate::colony::{BuildQueue, BuildingQueue, Buildings, Colony, ConstructionParams, Production, ResourceStockpile};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::player::{Player, StationedAt};
use crate::ship::{Cargo, CommandQueue, Ship, ShipState};
use crate::technology::{GlobalParams, ResearchPool, ResearchQueue, TechTree};
use crate::technology::EmpireModifiers;
use crate::time_system::{GameClock, GameSpeed};
use crate::visualization::{ContextMenu, SelectedShip, SelectedSystem};

/// Grouped read-only resources for the UI system to stay within Bevy's
/// 16-parameter limit.
#[derive(SystemParam)]
pub struct UiResources<'w> {
    pub knowledge: Res<'w, KnowledgeStore>,
    pub command_log: Res<'w, CommandLog>,
    pub global_params: Res<'w, GlobalParams>,
    pub construction_params: Res<'w, ConstructionParams>,
    pub tech_tree: Res<'w, TechTree>,
    pub research_pool: Res<'w, ResearchPool>,
}

/// Grouped mutable resources for the research system.
#[derive(SystemParam)]
pub struct UiResMut<'w> {
    pub research_queue: ResMut<'w, ResearchQueue>,
}

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
    mut selected_ship: ResMut<SelectedShip>,
    mut context_menu: ResMut<ContextMenu>,
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: Query<&StationedAt, With<Player>>,
    mut colonies: Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
    )>,
    mut ships_query: Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    mut command_queues: Query<&mut CommandQueue>,
    positions: Query<&Position>,
    ui_res: UiResources,
    mut ui_res_mut: UiResMut,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // Collect resource totals before passing colonies around
    let (total_minerals, total_energy, total_food, total_authority) = {
        let mut m = crate::amount::Amt::ZERO;
        let mut e = crate::amount::Amt::ZERO;
        let mut f = crate::amount::Amt::ZERO;
        let mut a = crate::amount::Amt::ZERO;
        for (_, _, _, stockpile, _, _, _) in colonies.iter() {
            if let Some(s) = stockpile {
                m = m.add(s.minerals);
                e = e.add(s.energy);
                f = f.add(s.food);
                a = a.add(s.authority);
            }
        }
        (m, e, f, a)
    };
    top_bar::draw_top_bar(ctx, &clock, &mut speed, total_minerals, total_energy, total_food, total_authority, &mut research_open);

    outline::draw_outline(
        ctx,
        &stars,
        &colonies,
        &ships_query,
        &mut selected_system,
        &mut selected_ship,
    );

    side_panel::draw_system_panel(
        ctx,
        &selected_system,
        &mut selected_ship,
        &stars,
        &player_q,
        &mut colonies,
        &mut ships_query,
        &positions,
        &ui_res.knowledge,
        &clock,
        &ui_res.construction_params,
    );

    side_panel::draw_ship_panel(
        ctx,
        &mut selected_ship,
        &mut ships_query,
        &clock,
        &mut colonies,
        &stars,
        &command_queues,
    );

    // #76: Collect pending ship commands from context menu (light-speed delay)
    let mut pending_ship_commands = Vec::new();
    side_panel::draw_context_menu(
        ctx,
        &mut context_menu,
        &mut selected_ship,
        &stars,
        &mut ships_query,
        &mut command_queues,
        &positions,
        &clock,
        &ui_res.global_params,
        &player_q,
        &mut pending_ship_commands,
    );
    // Spawn any delayed commands as entities
    for pending_cmd in pending_ship_commands {
        commands.spawn(pending_cmd);
    }

    bottom_bar::draw_bottom_bar(ctx, &ui_res.command_log, &clock);

    // Find capital colony stockpile for upfront cost checks
    let capital_stockpile: Option<(crate::amount::Amt, crate::amount::Amt)> = {
        let mut result = None;
        for (_, colony_data, _, stockpile, _, _, _) in colonies.iter() {
            if let Some(s) = stockpile {
                // Check if the colony's system is the capital
                if let Ok((_, star, _, _)) = stars.get(colony_data.system) {
                    if star.is_capital {
                        result = Some((s.minerals, s.energy));
                        break;
                    }
                }
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
        &ui_res.tech_tree,
        &ui_res_mut.research_queue,
        &ui_res.research_pool,
        capital_refs,
        clock.elapsed,
    );

    // Handle research actions that require mutable colony access
    match research_action {
        overlays::ResearchAction::StartResearch(tech_id) => {
            // Deduct upfront costs from capital stockpile
            if let Some(tech) = ui_res.tech_tree.get(tech_id) {
                let mineral_cost = tech.cost.minerals;
                let energy_cost = tech.cost.energy;

                // Find and deduct from capital colony
                for (_, colony_data, _, stockpile, _, _, _) in colonies.iter_mut() {
                    if let Some(mut s) = stockpile {
                        if let Ok((_, star, _, _)) = stars.get(colony_data.system) {
                            if star.is_capital {
                                s.minerals = s.minerals.sub(mineral_cost);
                                s.energy = s.energy.sub(energy_cost);
                                break;
                            }
                        }
                    }
                }

                ui_res_mut.research_queue.start_research(tech_id);
            }
        }
        overlays::ResearchAction::CancelResearch => {
            ui_res_mut.research_queue.cancel_research();
        }
        overlays::ResearchAction::None => {}
    }
}
