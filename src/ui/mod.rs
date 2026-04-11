pub mod bottom_bar;
pub mod context_menu;
pub mod outline;
pub mod overlays;
pub mod params;
pub mod ship_panel;
pub mod system_panel;
pub mod top_bar;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::amount::{Amt, SignedAmt};
use crate::colony::{
    AuthorityParams, BuildQueue, BuildingQueue, Buildings, Colony, ConstructionParams,
    FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile,
    SystemBuildingQueue, SystemBuildings,
};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::player::{AboardShip, Player, PlayerEmpire, StationedAt};
use crate::ship::{
    Cargo, CommandQueue, PendingShipCommand, RulesOfEngagement, Ship, ShipHitpoints, ShipState,
    SurveyData,
};
use crate::ship_design::{HullRegistry, ModuleRegistry, ShipDesignRegistry};
use crate::scripting::building_api::BuildingRegistry;
use crate::technology::{GlobalParams, ResearchPool, ResearchQueue, TechTree};
use crate::time_system::{GameClock, GameSpeed};
use crate::visualization::{
    ContextMenu, EguiWantsPointer, OutlineExpandedSystems, SelectedPlanet, SelectedShip,
    SelectedSystem,
};

use params::{MainPanelRegistries, MainPanelSelection, MainPanelWorldQueries};

/// Resource tracking whether the research overlay is open.
#[derive(Resource, Default)]
pub struct ResearchPanelOpen(pub bool);

/// Intermediate resource holding pre-computed UI data shared across systems.
/// Written by `compute_ui_state`, read by drawing systems.
#[derive(Resource, Default)]
pub struct UiState {
    pub player_system: Option<Entity>,
    pub player_entity: Option<Entity>,
    pub player_aboard_ship: Option<Entity>,
    pub total_minerals: Amt,
    pub total_energy: Amt,
    pub total_food: Amt,
    pub total_authority: Amt,
    pub net_minerals: SignedAmt,
    pub net_energy: SignedAmt,
    pub net_food: SignedAmt,
    pub net_authority: SignedAmt,
    pub capital_stockpile: Option<(Amt, Amt)>,
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<ResearchPanelOpen>()
            .init_resource::<overlays::ShipDesignerState>()
            .init_resource::<EguiWantsPointer>()
            .init_resource::<UiState>()
            .add_systems(
                EguiPrimaryContextPass,
                (
                    compute_ui_state,
                    draw_top_bar_system,
                    draw_outline_and_tooltips_system,
                    draw_main_panels_system,
                    draw_overlays_system,
                    draw_bottom_bar_system,
                )
                    .chain(),
            );
    }
}

// ---------------------------------------------------------------------------
// System 1: compute_ui_state — pre-compute player info and resource totals
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn compute_ui_state(
    mut ui_state: ResMut<UiState>,
    player_q: Query<(Entity, &StationedAt, Option<&AboardShip>), With<Player>>,
    colonies: Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&BuildQueue>,
        Option<&Buildings>,
        Option<&BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    system_stockpiles: Query<
        (&ResourceStockpile, Option<&ResourceCapacity>),
        With<StarSystem>,
    >,
    empire_q: Query<(&KnowledgeStore, &AuthorityParams), With<PlayerEmpire>>,
    planets: Query<&Planet>,
) {
    let player_info = player_q
        .iter()
        .next()
        .map(|(e, s, a)| (e, s.system, a.map(|ab| ab.ship)));
    ui_state.player_system = player_info.map(|(_, sys, _)| sys);
    ui_state.player_entity = player_info.map(|(e, _, _)| e);
    ui_state.player_aboard_ship = player_info.and_then(|(_, _, aboard)| aboard);

    let Ok((knowledge, authority_params)) = empire_q.single() else {
        return;
    };

    // Collect resource totals using KnowledgeStore (light-speed delayed) + real-time for local system
    let mut m = Amt::ZERO;
    let mut e = Amt::ZERO;
    let mut f = Amt::ZERO;
    let mut a = Amt::ZERO;

    // Remote systems: use delayed data from KnowledgeStore
    for (_entity, k) in knowledge.iter() {
        if ui_state.player_system == Some(k.system) {
            continue;
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
    if let Some(local_sys) = ui_state.player_system {
        if let Ok((stockpile, _)) = system_stockpiles.get(local_sys) {
            m = m.add(stockpile.minerals);
            e = e.add(stockpile.energy);
            f = f.add(stockpile.food);
            a = a.add(stockpile.authority);
        }
    }

    ui_state.total_minerals = m;
    ui_state.total_energy = e;
    ui_state.total_food = f;
    ui_state.total_authority = a;

    // Net income calculations
    let mut net_m = SignedAmt::ZERO;
    let mut net_e = SignedAmt::ZERO;
    let mut net_f = SignedAmt::ZERO;
    let mut colony_count: u64 = 0;
    let mut has_capital = false;
    for (_, colony, production, _, _, _, maintenance, food_consumption) in colonies.iter() {
        if let Some(prod) = production {
            net_m = net_m.add(SignedAmt::from_amt(prod.minerals_per_hexadies.final_value()));
            let energy_prod = SignedAmt::from_amt(prod.energy_per_hexadies.final_value());
            let maint = maintenance
                .map(|mc| SignedAmt::from_amt(mc.energy_per_hexadies.final_value()))
                .unwrap_or(SignedAmt::ZERO);
            net_e = net_e.add(energy_prod.add(SignedAmt(0 - maint.raw())));
            let food_prod = SignedAmt::from_amt(prod.food_per_hexadies.final_value());
            let food_cons = food_consumption
                .map(|fc| SignedAmt::from_amt(fc.food_per_hexadies.final_value()))
                .unwrap_or(SignedAmt::ZERO);
            net_f = net_f.add(food_prod.add(SignedAmt(0 - food_cons.raw())));
        }
        colony_count += 1;
        if let Some(sys) = colony.system(&planets) {
            if let Ok((_, star, _, _)) = stars.get(sys) {
                if star.is_capital {
                    has_capital = true;
                }
            }
        }
    }
    let non_capital_count = if has_capital {
        colony_count.saturating_sub(1)
    } else {
        colony_count
    };
    let auth_prod = SignedAmt::from_amt(authority_params.production.final_value());
    let auth_cost = SignedAmt::from_amt(
        authority_params
            .cost_per_colony
            .final_value()
            .mul_u64(non_capital_count),
    );
    let net_a = auth_prod.add(SignedAmt(0 - auth_cost.raw()));

    ui_state.net_minerals = net_m;
    ui_state.net_energy = net_e;
    ui_state.net_food = net_f;
    ui_state.net_authority = net_a;

    // Capital stockpile for upfront cost checks (research)
    ui_state.capital_stockpile = None;
    for (sys_entity, star, _, _) in stars.iter() {
        if star.is_capital {
            if let Ok((s, _)) = system_stockpiles.get(sys_entity) {
                ui_state.capital_stockpile = Some((s.minerals, s.energy));
            }
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// System 2: draw_top_bar_system
// ---------------------------------------------------------------------------

fn draw_top_bar_system(
    mut contexts: EguiContexts,
    ui_state: Res<UiState>,
    clock: Res<GameClock>,
    mut speed: ResMut<GameSpeed>,
    mut research_open: ResMut<ResearchPanelOpen>,
    mut designer_state: ResMut<overlays::ShipDesignerState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    top_bar::draw_top_bar(
        ctx,
        &clock,
        &mut speed,
        ui_state.total_minerals,
        ui_state.total_energy,
        ui_state.total_food,
        ui_state.total_authority,
        ui_state.net_food,
        ui_state.net_energy,
        ui_state.net_minerals,
        ui_state.net_authority,
        &mut research_open,
        &mut designer_state,
    );
}

// ---------------------------------------------------------------------------
// System 3: draw_outline_and_tooltips_system
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_outline_and_tooltips_system(
    mut contexts: EguiContexts,
    clock: Res<GameClock>,
    ui_state: Res<UiState>,
    mut selected_system: ResMut<SelectedSystem>,
    mut selected_ship: ResMut<SelectedShip>,
    mut egui_wants_pointer: ResMut<EguiWantsPointer>,
    mut outline_expanded: ResMut<OutlineExpandedSystems>,
    galaxy_view: Res<crate::visualization::GalaxyView>,
    design_registry: Res<ShipDesignRegistry>,
    empire_q: Query<&KnowledgeStore, With<PlayerEmpire>>,
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    colonies: Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    ships_query: Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    planets: Query<&Planet>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<Camera2d>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let knowledge = empire_q.single().ok();
    let player_system = ui_state.player_system;

    egui_wants_pointer.0 = ctx.wants_pointer_input();

    outline::draw_outline(
        ctx,
        &stars,
        &colonies,
        &ships_query,
        &mut selected_system,
        &mut selected_ship,
        &planets,
        &mut outline_expanded,
        &design_registry,
    );

    draw_map_tooltips(
        ctx,
        &windows,
        &camera_q,
        &stars,
        &ships_query,
        &planets,
        &colonies,
        &clock,
        &galaxy_view,
        &design_registry,
        knowledge,
        player_system,
    );
}

// ---------------------------------------------------------------------------
// System 4: draw_main_panels_system — system panel, ship panel, context menu
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_main_panels_system(
    mut commands: Commands,
    mut contexts: EguiContexts,
    clock: Res<GameClock>,
    ui_state: Res<UiState>,
    mut selection: MainPanelSelection,
    registries: MainPanelRegistries,
    building_registry: Res<BuildingRegistry>,
    mut world: MainPanelWorldQueries,
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
    mut ships_query: Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    mut command_queues: Query<&mut CommandQueue>,
    empire_q: Query<
        (
            &KnowledgeStore,
            &CommandLog,
            &GlobalParams,
            &ConstructionParams,
            &TechTree,
            &ResearchPool,
            &ResearchQueue,
            &AuthorityParams,
        ),
        With<PlayerEmpire>,
    >,
    mut game_events: MessageWriter<GameEvent>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Ok((knowledge, _command_log, global_params, construction_params, _tech_tree, _research_pool, _research_queue, _authority_params)) =
        empire_q.single()
    else {
        return;
    };

    let player_system = ui_state.player_system;
    let player_aboard_ship = ui_state.player_aboard_ship;
    let player_info = player_q
        .iter()
        .next()
        .map(|(e, s, a)| (e, s.system, a.map(|ab| ab.ship)));

    // --- System panel ---
    let mut colonization_actions = Vec::new();
    system_panel::draw_system_panel(
        ctx,
        &mut selection.selected_system,
        &mut selection.selected_ship,
        &mut selection.selected_planet,
        &stars,
        &player_q,
        &mut colonies,
        &mut world.stockpiles,
        &mut ships_query,
        &world.positions,
        knowledge,
        &clock,
        construction_params,
        &world.planets,
        &world.planet_entities,
        &mut world.system_buildings,
        &registries.hull_registry,
        &registries.module_registry,
        &registries.design_registry,
        &world.colonization_queues,
        &mut colonization_actions,
        &building_registry,
        &world.anomalies,
    );

    for action in colonization_actions {
        commands.spawn(crate::colony::PendingColonizationOrder {
            system_entity: action.system_entity,
            target_planet: action.target_planet,
            source_colony: action.source_colony,
        });
    }

    // --- Ship panel ---
    let ship_panel_actions = ship_panel::draw_ship_panel(
        ctx,
        &mut selection.selected_ship,
        &mut ships_query,
        &clock,
        &mut colonies,
        &mut world.stockpiles,
        &stars,
        &command_queues,
        &world.planets,
        &world.pending_commands,
        &registries.hull_registry,
        &registries.module_registry,
        &registries.design_registry,
        clock.elapsed,
        &world.roe,
        &world.positions,
        player_system,
        player_aboard_ship,
    );

    // Handle cancel current action
    if ship_panel_actions.cancel_current {
        if let Some(ship_entity) = selection.selected_ship.0 {
            if let Ok((_, _, mut state, _, _, _)) = ships_query.get_mut(ship_entity) {
                let dock_system = match &*state {
                    ShipState::Surveying {
                        target_system, ..
                    } => Some(*target_system),
                    ShipState::Settling { system, .. } => Some(*system),
                    _ => None,
                };
                if let Some(sys) = dock_system {
                    *state = ShipState::Docked { system: sys };
                }
            }
        }
    }

    // Handle cancel individual command from queue
    if let Some(index) = ship_panel_actions.cancel_command_index {
        if let Some(ship_entity) = selection.selected_ship.0 {
            if let Ok(mut queue) = command_queues.get_mut(ship_entity) {
                if index < queue.commands.len() {
                    queue.commands.remove(index);
                }
            }
        }
    }

    // Handle clear all commands from queue
    if ship_panel_actions.clear_commands {
        if let Some(ship_entity) = selection.selected_ship.0 {
            if let Ok(mut queue) = command_queues.get_mut(ship_entity) {
                queue.commands.clear();
            }
        }
    }

    // Handle ship scrapping
    if let Some(scrap) = ship_panel_actions.scrap {
        if let Ok((mut stockpile, _)) = world.stockpiles.get_mut(scrap.colony_entity) {
            stockpile.minerals = stockpile.minerals.add(scrap.minerals_refund);
            stockpile.energy = stockpile.energy.add(scrap.energy_refund);
        }
        commands.entity(scrap.ship_entity).despawn();
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

    // Handle ship refit
    if let Some(refit) = ship_panel_actions.refit {
        if let Ok((mut stockpile, _)) = world.stockpiles.get_mut(refit.system_entity) {
            stockpile.minerals = stockpile.minerals.sub(refit.cost_minerals);
            stockpile.energy = stockpile.energy.sub(refit.cost_energy);
        }
        if let Ok((_, _, mut state, _, _, _)) = ships_query.get_mut(refit.ship_entity) {
            *state = ShipState::Refitting {
                system: refit.system_entity,
                started_at: clock.elapsed,
                completes_at: clock.elapsed + refit.refit_time,
                new_modules: refit.new_modules,
            };
        }
    }

    // Handle ROE change
    if let Some((ship_entity, new_roe, delay)) = ship_panel_actions.set_roe {
        if delay == 0 {
            commands.entity(ship_entity).insert(new_roe);
        } else {
            commands.spawn(PendingShipCommand {
                ship: ship_entity,
                command: crate::ship::ShipCommand::SetROE { roe: new_roe },
                arrives_at: clock.elapsed + delay,
            });
        }
    }

    // Handle board ship
    if let Some(ship_entity) = ship_panel_actions.board_ship {
        if let Some((player_entity, _, _)) = player_info {
            if let Ok((_, mut ship, _, _, _, _)) = ships_query.get_mut(ship_entity) {
                ship.player_aboard = true;
            }
            commands
                .entity(player_entity)
                .insert(AboardShip { ship: ship_entity });
        }
    }

    // Handle disembark
    if ship_panel_actions.disembark {
        if let Some((player_entity, _, _)) = player_info {
            if let Some(ship_entity) = selection.selected_ship.0 {
                if let Ok((_, mut ship, _state, _, _, _)) = ships_query.get_mut(ship_entity) {
                    ship.player_aboard = false;
                }
            }
            commands.entity(player_entity).remove::<AboardShip>();
        }
    }

    // --- Context menu ---
    let mut pending_ship_commands = Vec::new();
    let colony_ro: Vec<Colony> = colonies
        .iter()
        .map(|(_, c, _, _, _, _, _, _)| Colony {
            planet: c.planet,
            population: c.population,
            growth_rate: c.growth_rate,
        })
        .collect();
    // #176: Build hostile_systems using real-time for local, KnowledgeStore for remote
    let hostile_systems: std::collections::HashSet<Entity> = {
        let mut set: std::collections::HashSet<Entity> = std::collections::HashSet::new();
        // Local system: real-time HostilePresence
        for h in world.hostile_presence.iter() {
            if Some(h.system) == player_system {
                set.insert(h.system);
            }
        }
        // Remote systems: from KnowledgeStore
        for (_entity, k) in knowledge.iter() {
            if Some(k.system) == player_system {
                continue;
            }
            if k.data.has_hostile {
                set.insert(k.system);
            }
        }
        set
    };
    context_menu::draw_context_menu(
        ctx,
        &mut selection.context_menu,
        &mut selection.selected_ship,
        &stars,
        &mut ships_query,
        &mut command_queues,
        &world.positions,
        &clock,
        global_params,
        &player_q,
        &mut pending_ship_commands,
        &colony_ro,
        &world.planets,
        &world.planet_entities,
        &hostile_systems,
        &registries.design_registry,
    );
    for pending_cmd in pending_ship_commands {
        commands.spawn(pending_cmd);
    }
}

// ---------------------------------------------------------------------------
// System 5: draw_overlays_system — research panel, ship designer
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw_overlays_system(
    mut contexts: EguiContexts,
    ui_state: Res<UiState>,
    clock: Res<GameClock>,
    mut research_open: ResMut<ResearchPanelOpen>,
    mut designer_state: ResMut<overlays::ShipDesignerState>,
    hull_registry: Res<HullRegistry>,
    module_registry: Res<ModuleRegistry>,
    mut design_registry: ResMut<ShipDesignRegistry>,
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    mut system_stockpiles: Query<
        (&mut ResourceStockpile, Option<&ResourceCapacity>),
        With<StarSystem>,
    >,
    mut empire_q: Query<
        (&TechTree, &ResearchPool, &mut ResearchQueue),
        With<PlayerEmpire>,
    >,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Ok((tech_tree, research_pool, mut research_queue)) = empire_q.single_mut() else {
        return;
    };

    let capital_refs = ui_state
        .capital_stockpile
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

    match research_action {
        overlays::ResearchAction::StartResearch(tech_id) => {
            if let Some(tech) = tech_tree.get(&tech_id) {
                let mineral_cost = tech.cost.minerals;
                let energy_cost = tech.cost.energy;

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
            designer_state.open = false;
            designer_state.selected_hull = None;
            designer_state.selected_modules.clear();
            designer_state.design_name.clear();
        }
        overlays::ShipDesignerAction::None => {}
    }
}

// ---------------------------------------------------------------------------
// System 6: draw_bottom_bar_system
// ---------------------------------------------------------------------------

fn draw_bottom_bar_system(
    mut contexts: EguiContexts,
    clock: Res<GameClock>,
    empire_q: Query<&CommandLog, With<PlayerEmpire>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Ok(command_log) = empire_q.single() else {
        return;
    };
    bottom_bar::draw_bottom_bar(ctx, command_log, &clock);
}

// ---------------------------------------------------------------------------
// Helper: draw_map_tooltips (plain function, not a Bevy system)
// ---------------------------------------------------------------------------

/// Draw tooltips when hovering over objects on the galaxy map.
#[allow(clippy::too_many_arguments)]
fn draw_map_tooltips(
    ctx: &egui::Context,
    windows: &Query<&Window>,
    camera_q: &Query<(&Camera, &GlobalTransform), With<Camera2d>>,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
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
    design_registry: &ShipDesignRegistry,
    knowledge: Option<&KnowledgeStore>,
    player_system: Option<Entity>,
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
        let star_px =
            bevy::math::Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale);
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
                let t = if total > 0.0 {
                    (elapsed / total).clamp(0.0, 1.0)
                } else {
                    1.0
                };
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
                let origin_pos = stars
                    .iter()
                    .find(|(e, _, _, _)| *e == *origin_system)
                    .map(|(_, _, p, _)| p);
                let dest_pos = stars
                    .iter()
                    .find(|(e, _, _, _)| *e == *destination_system)
                    .map(|(_, _, p, _)| p);
                if let (Some(op), Some(dp)) = (origin_pos, dest_pos) {
                    let total = (*arrival_at - *departed_at) as f64;
                    let elapsed = (clock.elapsed - *departed_at) as f64;
                    let t = if total > 0.0 {
                        (elapsed / total).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
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
                let design_name = design_registry.get(&ship.design_id)
                    .map(|d| d.name.as_str())
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

    // Star tooltip — #176: use KnowledgeStore for remote systems
    if let Some((star_entity, _)) = best_star {
        if let Ok((_, star, _, attrs)) = stars.get(star_entity) {
            let is_local = player_system == Some(star_entity);
            let k_data = if is_local { None } else { knowledge.and_then(|k| k.get(star_entity)) };

            // For remote systems, derive info from KnowledgeStore
            let effective_surveyed = if is_local {
                star.surveyed
            } else {
                k_data.map(|k| k.data.surveyed).unwrap_or(false)
            };

            let has_colony = if is_local {
                colonies.iter().any(|(_, c, _, _, _, _, _, _)| {
                    c.system(planets).is_some_and(|sys| sys == star_entity)
                })
            } else {
                k_data.map(|k| k.data.colonized).unwrap_or(false)
            };

            let effective_hab = if is_local {
                attrs.map(|a| a.habitability)
            } else {
                k_data.and_then(|k| k.data.habitability)
            };

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
                if effective_surveyed {
                    // Local: show actual planet count. Remote: planet count not in snapshot, skip.
                    if is_local {
                        let planet_count = planets.iter().filter(|p| p.system == star_entity).count();
                        ui.label(format!("Planets: {}", planet_count));
                    }
                    if let Some(hab) = effective_hab {
                        ui.label(format!("Habitability: {}", crate::galaxy::habitability_label(hab)));
                    }
                } else {
                    ui.label(egui::RichText::new("Unsurveyed").weak().italics());
                }
                if !is_local {
                    if let Some(k) = k_data {
                        let age = clock.elapsed - k.observed_at;
                        let years = age as f64 / crate::time_system::HEXADIES_PER_YEAR as f64;
                        ui.label(egui::RichText::new(format!("Info age: {:.1} yr", years)).weak().small());
                    } else if !star.is_capital {
                        ui.label(egui::RichText::new("No intelligence").weak().italics());
                    }
                }
                if has_colony {
                    ui.label(
                        egui::RichText::new("Colonized")
                            .color(egui::Color32::from_rgb(100, 255, 100)),
                    );
                }
            });
        }
    }
}
