pub mod ai_debug;
pub mod bottom_bar;
pub mod context_menu;
pub mod outline;
pub mod overlays;
pub mod params;
pub mod ship_panel;
pub mod situation_center;
pub mod system_panel;
pub mod top_bar;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::amount::{Amt, SignedAmt};
use crate::choice::{PendingChoice, PendingChoiceSelection};
use crate::colony::{
    AuthorityParams, BuildQueue, BuildingQueue, Buildings, Colony, ConstructionParams,
    FoodConsumption, MaintenanceCost, Production, ResourceCapacity, ResourceStockpile,
    SystemBuildingQueue, SystemBuildings,
};
use crate::communication::CommandLog;
use crate::condition::ScopedFlags;
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::notifications::{NotificationPriority, NotificationQueue};
use crate::player::{AboardShip, Player, PlayerEmpire, StationedAt};
use crate::ship::{
    Cargo, CommandQueue, CourierRoute, PendingShipCommand, QueuedCommand, RulesOfEngagement, Ship,
    ShipHitpoints, ShipState, SurveyData,
};
use crate::ship_design::{HullRegistry, ModuleRegistry, ShipDesignRegistry};
use crate::scripting::building_api::BuildingRegistry;
use crate::technology::{GameFlags, GlobalParams, ResearchPool, ResearchQueue, TechTree};
use crate::time_system::{GameClock, GameSpeed};
use crate::visualization::{
    ContextMenu, DeployMode, DeployPending, EguiWantsPointer, OutlineExpandedSystems,
    SelectedPlanet, SelectedShip, SelectedSystem,
};

use params::{
    MainPanelDeliverableRes, MainPanelRegistries, MainPanelSelection, MainPanelWorldQueries,
};

/// Resource tracking whether the research overlay is open.
#[derive(Resource, Default)]
pub struct ResearchPanelOpen(pub bool);

/// #252: Selected tab in the colony detail panel. `Overview` retains the
/// pre-existing income/buildings view; `PopManagement` shows population
/// breakdown, job slot assignments, and per-job production contributions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColonyPanelTab {
    #[default]
    Overview,
    PopManagement,
}

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
    /// #252: Which tab is active in the colony detail window.
    pub colony_panel_tab: ColonyPanelTab,
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            // #344: ESC framework (SituationCenterState, SituationTabRegistry,
            // EscNotificationQueue, F3 toggle system, Notifications tab).
            // Registered before the draw chain is attached so any plugin
            // that calls `register_situation_tab` during `build()` finds the
            // registry already initialised.
            .add_plugins(situation_center::SituationCenterPlugin)
            .init_resource::<ResearchPanelOpen>()
            .init_resource::<overlays::ShipDesignerState>()
            .init_resource::<EguiWantsPointer>()
            .init_resource::<UiState>()
            .init_resource::<ai_debug::AiDebugUi>()
            .add_systems(Update, ai_debug::toggle_ai_debug)
            .add_systems(
                EguiPrimaryContextPass,
                (
                    // #261: Install the bundled CJK font on the first pass.
                    // Must run before any draw system so every widget picks up
                    // the Japanese-capable fallback on frame 1.
                    setup_cjk_font,
                    compute_ui_state,
                    draw_top_bar_system,
                    draw_notifications_system,
                    draw_outline_and_tooltips_system,
                    draw_main_panels_system,
                    draw_overlays_system,
                    // #344: ESC panel sits alongside Research / Ship
                    // Designer in the floating-window slot. Exclusive
                    // system — keeps its own `&World` access for tab
                    // `badge` / `render`, so it cannot be parallelised
                    // with the surrounding UI systems anyway.
                    situation_center::draw_situation_center_system,
                    draw_choice_dialog_system,
                    ai_debug::sample_ai_debug_stream,
                    ai_debug::draw_ai_debug_system,
                    draw_bottom_bar_system,
                )
                    .chain(),
            );
    }
}

/// #261: Raw bytes of the bundled CJK font. Zen Kaku Gothic New Regular from
/// `github.com/googlefonts/zen-kakugothic`, distributed under SIL OFL 1.1 (see
/// `assets/fonts/OFL.txt`). Embedded at compile time so the binary carries its
/// own glyph coverage and doesn't depend on a filesystem asset path at runtime.
const CJK_FONT_BYTES: &[u8] =
    include_bytes!("../../assets/fonts/ZenKakuGothicNew-Regular.ttf");

/// #261: Register the bundled CJK font with egui on the first pass so
/// Japanese glyphs (タブ名、イベントログ、Lua 由来テキスト等) render instead
/// of falling through to tofu. The guard makes this a one-shot system — egui
/// keeps the fonts across subsequent frames once `set_fonts` has been called.
fn setup_cjk_font(mut contexts: EguiContexts, mut initialized: Local<bool>) {
    if *initialized {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "zen_kaku_jp".into(),
        egui::FontData::from_static(CJK_FONT_BYTES).into(),
    );
    // Make the CJK font the first-priority proportional font so ASCII keeps
    // its existing shape where Zen Kaku Gothic covers it, and the fallback
    // chain remains (egui-default Latin + symbol fonts stay behind it).
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "zen_kaku_jp".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("zen_kaku_jp".into());
    ctx.set_fonts(fonts);
    *initialized = true;
}

#[cfg(test)]
mod font_tests {
    use super::CJK_FONT_BYTES;

    /// #261: Verify the font binary is actually embedded and non-trivial.
    /// TTF files start with the magic bytes 0x00 0x01 0x00 0x00 (or `OTTO`
    /// for OpenType CFF); Zen Kaku Gothic New ships as TTF so we check the
    /// former. This guards against a missing / truncated asset at build time.
    #[test]
    fn test_font_bytes_embedded_at_compile_time() {
        assert!(
            CJK_FONT_BYTES.len() > 100_000,
            "embedded font is suspiciously small ({}B)",
            CJK_FONT_BYTES.len()
        );
        assert_eq!(
            &CJK_FONT_BYTES[..4],
            &[0x00, 0x01, 0x00, 0x00],
            "font header does not look like a TTF"
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
    crate::prof_span!("compute_ui_state");
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

#[allow(clippy::too_many_arguments)]
fn draw_top_bar_system(
    mut contexts: EguiContexts,
    ui_state: Res<UiState>,
    clock: Res<GameClock>,
    mut speed: ResMut<GameSpeed>,
    mut research_open: ResMut<ResearchPanelOpen>,
    mut designer_state: ResMut<overlays::ShipDesignerState>,
    observer_mode: Res<crate::observer::ObserverMode>,
    mut observer_view: ResMut<crate::observer::ObserverView>,
    factions_q: Query<(Entity, &crate::player::Faction), With<crate::player::Empire>>,
) {
    crate::prof_span!("draw_top_bar");
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Build sorted (entity, name) list of empire factions for the selector.
    let mut factions: Vec<(Entity, String)> = factions_q
        .iter()
        .map(|(e, f)| (e, f.name.clone()))
        .collect();
    factions.sort_by(|a, b| a.1.cmp(&b.1));

    let mut selected = observer_view.viewing;
    let observer_state = if observer_mode.enabled {
        Some(top_bar::ObserverBarState {
            enabled: true,
            selected: &mut selected,
            factions: &factions,
        })
    } else {
        None
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
        observer_state,
    );

    if selected != observer_view.viewing {
        observer_view.viewing = selected;
    }
}

// ---------------------------------------------------------------------------
// System 2.5: draw_notifications_system — banner stack at the top (#151)
// ---------------------------------------------------------------------------

fn draw_notifications_system(
    mut contexts: EguiContexts,
    mut queue: ResMut<NotificationQueue>,
    mut selected_system: ResMut<SelectedSystem>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Snapshot the items so we can iterate without holding a borrow on the
    // resource while we also mutate it (dismiss, jump).
    let items: Vec<crate::notifications::Notification> = queue.items.clone();
    if items.is_empty() {
        return;
    }

    let mut to_dismiss: Vec<u64> = Vec::new();
    let mut jump_target: Option<Entity> = None;

    egui::Area::new(egui::Id::new("notification_banners"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 48.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(420.0);
                for n in &items {
                    let (border, fill) = match n.priority {
                        NotificationPriority::High => (
                            egui::Color32::from_rgb(220, 80, 80),
                            egui::Color32::from_rgba_premultiplied(60, 14, 14, 230),
                        ),
                        NotificationPriority::Medium => (
                            egui::Color32::from_rgb(230, 200, 90),
                            egui::Color32::from_rgba_premultiplied(40, 36, 14, 220),
                        ),
                        NotificationPriority::Low => (
                            egui::Color32::DARK_GRAY,
                            egui::Color32::from_rgba_premultiplied(20, 20, 20, 200),
                        ),
                    };

                    egui::Frame::group(ui.style())
                        .stroke(egui::Stroke::new(1.5, border))
                        .fill(fill)
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.set_min_width(380.0);
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&n.title)
                                            .strong()
                                            .color(egui::Color32::WHITE),
                                    );
                                    if !n.description.is_empty() {
                                        ui.label(
                                            egui::RichText::new(&n.description)
                                                .color(egui::Color32::LIGHT_GRAY),
                                        );
                                    }
                                    if let Some(remaining) = n.remaining_seconds {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "auto-dismiss in {:.0}s",
                                                remaining.max(0.0),
                                            ))
                                            .small()
                                            .weak(),
                                        );
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        if ui
                                            .button(egui::RichText::new("✕").strong())
                                            .on_hover_text("Dismiss")
                                            .clicked()
                                        {
                                            to_dismiss.push(n.id);
                                        }
                                        if let Some(target) = n.target_system {
                                            if ui
                                                .button("Jump")
                                                .on_hover_text("Select target system")
                                                .clicked()
                                            {
                                                jump_target = Some(target);
                                                to_dismiss.push(n.id);
                                            }
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(4.0);
                }
            });
        });

    for id in to_dismiss {
        queue.dismiss(id);
    }
    if let Some(target) = jump_target {
        selected_system.0 = Some(target);
    }
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
    crate::prof_span!("draw_outline_and_tooltips");
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
    mut ui_state: ResMut<UiState>,
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
    mut deliverables_res: MainPanelDeliverableRes,
    mut game_events: MessageWriter<GameEvent>,
) {
    crate::prof_span!("draw_main_panels");
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let Ok((knowledge, _command_log, global_params, construction_params, tech_tree, _research_pool, _research_queue, _authority_params)) =
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

    // #229: Pre-compute condition evaluation inputs so the system panel can
    // filter the shipyard Deliverables list by `prerequisites`. `TechTree`
    // stores techs as `TechId(String)`; the Condition DSL uses raw strings.
    let researched_techs: std::collections::HashSet<String> = tech_tree
        .technologies
        .iter()
        .filter(|(_, t)| tech_tree.is_researched(&t.id))
        .map(|(id, _)| id.0.clone())
        .collect();
    let active_modifiers: std::collections::HashSet<String> = std::collections::HashSet::new();
    let (empire_flags_union, empire_buildings) = match deliverables_res.empire_flags.single() {
        Ok((game_flags, scoped_flags)) => {
            let mut union: std::collections::HashSet<String> = scoped_flags.flags.clone();
            union.extend(game_flags.flags.iter().cloned());
            (union, std::collections::HashSet::<String>::new())
        }
        Err(_) => (
            std::collections::HashSet::<String>::new(),
            std::collections::HashSet::<String>::new(),
        ),
    };
    let deliverable_avail = system_panel::DeliverableAvailabilityCtx {
        researched_techs: &researched_techs,
        active_modifiers: &active_modifiers,
        empire_flags: &empire_flags_union,
        empire_buildings: &empire_buildings,
    };

    // --- System panel ---
    let mut colonization_actions = Vec::new();
    let mut system_actions = system_panel::SystemPanelActions::default();
    system_panel::draw_system_panel(
        ctx,
        &mut selection.selected_system,
        &mut selection.selected_ship,
        &mut selection.selected_planet,
        &stars,
        &player_q,
        &mut colonies,
        &world.colony_pop_view,
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
        &registries.job_registry,
        &mut ui_state.colony_panel_tab,
        &world.anomalies,
        &world.deliverable_stockpiles,
        &world.deep_space_structures,
        &deliverables_res.structure_registry,
        &deliverable_avail,
        &mut system_actions,
        &mut deliverables_res.colony_dispatches,
        &world.remote_commands,
    );

    for action in colonization_actions {
        commands.spawn(crate::colony::PendingColonizationOrder {
            system_entity: action.system_entity,
            target_planet: action.target_planet,
            source_colony: action.source_colony,
        });
    }

    // #229: Handle dismantle — `dismantle_structure` requires exclusive
    // `&mut World`, so we wrap it in `commands.queue`.
    if let Some(structure_entity) = system_actions.dismantle {
        commands.queue(move |world: &mut World| {
            if let Err(e) =
                crate::ship::deliverable_ops::dismantle_structure(world, structure_entity)
            {
                warn!("Dismantle failed for {:?}: {}", structure_entity, e);
            } else {
                info!("Structure {:?} dismantled", structure_entity);
            }
        });
    }

    // #229: Handle "Load" from DeliverableStockpile row.
    if let Some((ship_e, system_e, idx)) = system_actions.load_deliverable {
        if let Ok(mut queue) = command_queues.get_mut(ship_e) {
            queue
                .commands
                .push(QueuedCommand::LoadDeliverable {
                    system: system_e,
                    stockpile_index: idx,
                });
            queue.predicted_system = Some(system_e);
        }
    }

    // --- Ship panel ---
    let selected_system_for_panel = selection.selected_system.0;

    // #229: Compute nearby structures for the selected ship. Threshold
    // chosen a bit loose (2 ly) so the UI still offers Transfer / Load
    // while the ship is sublight-cruising toward the structure. The
    // command processors re-check co-location via
    // `DEPLOY_POSITION_EPSILON` and auto-inject a `MoveToCoordinates`
    // when the ship isn't quite there yet.
    const NEARBY_STRUCTURE_RADIUS_LY: f64 = 2.0;
    let nearby_structures: Vec<ship_panel::NearbyStructure> = match selection.selected_ship.0 {
        Some(ship_e) => {
            let ship_pos = ships_query
                .get(ship_e)
                .ok()
                .and_then(|(_, _, state, _, _, _)| match &*state {
                    ShipState::Docked { system } => world.positions.get(*system).ok().copied(),
                    ShipState::Loitering { position } => Some(crate::components::Position::from(*position)),
                    ShipState::Surveying { target_system, .. }
                    | ShipState::Settling { system: target_system, .. } => world
                        .positions
                        .get(*target_system)
                        .ok()
                        .copied(),
                    _ => None,
                });
            match ship_pos {
                Some(sp) => {
                    let mut v: Vec<ship_panel::NearbyStructure> = Vec::new();
                    for (entity, ds, pos, platform, scrap) in
                        world.deep_space_structures.iter()
                    {
                        let d = sp.distance_to(pos);
                        if d > NEARBY_STRUCTURE_RADIUS_LY {
                            continue;
                        }
                        v.push(ship_panel::NearbyStructure {
                            entity,
                            name: ds.name.clone(),
                            is_platform: platform.is_some(),
                            is_scrapyard: scrap.is_some(),
                            distance_ly: d,
                        });
                    }
                    v
                }
                None => Vec::new(),
            }
        }
        None => Vec::new(),
    };

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
        &world.courier_routes,
        selected_system_for_panel,
        &world.fleet_members,
        &world.fleets,
        &nearby_structures,
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
            id: crate::knowledge::EventId::default(),
            timestamp: clock.elapsed,
            kind: GameEventKind::ShipScrapped,
            description,
            related_system: None,
        });
    }

    // #123: Handle ship refit (design-based). Resolve modules, cost and time
    // from the registered design at apply time so the ship is always brought
    // in line with the *current* design revision.
    if let Some(refit) = ship_panel_actions.refit {
        apply_design_refit(
            refit.ship_entity,
            refit.system_entity,
            &mut ships_query,
            &mut world.stockpiles,
            &registries.design_registry,
            &registries.hull_registry,
            &registries.module_registry,
            clock.elapsed,
        );
    }

    // #123: Handle fleet-wide refit — apply to every refit-eligible ship in
    // the fleet that is currently docked at a colony.
    if let Some(fleet_refit) = ship_panel_actions.fleet_refit {
        // #287 (γ-1): members are stored in the sibling FleetMembers
        // component now, not inside Fleet itself.
        let member_entities: Vec<Entity> = world
            .fleet_members
            .get(fleet_refit.fleet_entity)
            .map(|m| m.0.clone())
            .unwrap_or_default();
        for member in member_entities {
            // Determine if the member is docked at a system that has a colony
            // (matches the per-ship eligibility rule).
            let dock_system: Option<Entity> = ships_query
                .get(member)
                .ok()
                .and_then(|(_, ship, state, _, _, _)| {
                    let docked = match &*state {
                        ShipState::Docked { system } => Some(*system),
                        _ => None,
                    }?;
                    // Refit-eligible only if design revision is ahead.
                    let design = registries.design_registry.get(&ship.design_id)?;
                    if design.revision <= ship.design_revision {
                        return None;
                    }
                    Some(docked)
                });
            if let Some(sys) = dock_system {
                apply_design_refit(
                    member,
                    sys,
                    &mut ships_query,
                    &mut world.stockpiles,
                    &registries.design_registry,
                    &registries.hull_registry,
                    &registries.module_registry,
                    clock.elapsed,
                );
            }
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

    // #117: Courier route actions
    if let Some((ship_entity, target, mode)) = ship_panel_actions.courier_add_waypoint {
        // Read current route (if any), append waypoint, write back via insert.
        let existing = world.courier_routes.get(ship_entity).ok().cloned();
        let new_route = if let Some(mut route) = existing {
            route.waypoints.push(target);
            // Switching mode is a separate action; preserve current mode.
            route
        } else {
            crate::ship::CourierRoute::new(vec![target], mode)
        };
        commands.entity(ship_entity).insert(new_route);
    }
    if let Some(ship_entity) = ship_panel_actions.courier_toggle_pause {
        if let Ok(route) = world.courier_routes.get(ship_entity) {
            let mut new_route = route.clone();
            new_route.paused = !new_route.paused;
            commands.entity(ship_entity).insert(new_route);
        }
    }
    if let Some(ship_entity) = ship_panel_actions.courier_clear_route {
        commands.entity(ship_entity).remove::<crate::ship::CourierRoute>();
    }
    if let Some((ship_entity, mode)) = ship_panel_actions.courier_set_mode {
        if let Ok(route) = world.courier_routes.get(ship_entity) {
            let mut new_route = route.clone();
            new_route.mode = mode;
            commands.entity(ship_entity).insert(new_route);
        } else {
            // No existing route — create empty one with the chosen mode.
            commands
                .entity(ship_entity)
                .insert(crate::ship::CourierRoute::new(Vec::new(), mode));
        }
    }

    // #229: Ship panel deliverable actions.
    if let Some((ship_e, item_index)) = ship_panel_actions.deploy_mode_request {
        deliverables_res.deploy_mode.0 = Some(DeployPending {
            ship: ship_e,
            item_index,
        });
        info!(
            "Deploy mode armed: ship {:?} cargo #{} — click a star to place.",
            ship_e, item_index
        );
    }
    if let Some((ship_e, structure, minerals, energy)) = ship_panel_actions.transfer_request {
        if let Ok(mut queue) = command_queues.get_mut(ship_e) {
            queue.commands.push(QueuedCommand::TransferToStructure {
                structure,
                minerals,
                energy,
            });
        }
    }
    if let Some((ship_e, structure)) = ship_panel_actions.load_from_scrapyard_request {
        if let Ok(mut queue) = command_queues.get_mut(ship_e) {
            queue
                .commands
                .push(QueuedCommand::LoadFromScrapyard { structure });
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
    // #176/#293: Build hostile_systems using real-time for local, KnowledgeStore for remote
    let hostile_systems: std::collections::HashSet<Entity> = {
        let mut set: std::collections::HashSet<Entity> = std::collections::HashSet::new();
        // Local system: (AtSystem, FactionOwner, With<Hostile>)
        for (at_system, _owner) in world.hostile_presence.iter() {
            if Some(at_system.0) == player_system {
                set.insert(at_system.0);
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
    branch_registry: Res<crate::technology::TechBranchRegistry>,
    effects_preview: Res<crate::technology::TechEffectsPreview>,
    unlock_index: Res<crate::technology::TechUnlockIndex>,
) {
    crate::prof_span!("draw_overlays");
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
        &branch_registry,
        &effects_preview,
        &unlock_index,
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
            // #123: `upsert_edited` bumps the revision counter when an
            // existing design with the same ID is replaced. Ships pointing
            // at this design will pick up the bump and become refit-eligible.
            let id = design.id.clone();
            let name = design.name.clone();
            let new_rev = design_registry.upsert_edited(design);
            info!(
                "Ship design saved: {} ({}) — revision {}",
                name, id, new_rev
            );
            designer_state.open = false;
            designer_state.selected_hull = None;
            designer_state.selected_modules.clear();
            designer_state.design_name.clear();
            designer_state.editing_design_id = None;
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
    crate::prof_span!("draw_bottom_bar");
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
                    ShipState::Loitering { .. } => "Loitering",
                    ShipState::Scouting { .. } => "Scouting",
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
                        // #215: Tag tooltip freshness with observation source
                        // so the player can see at a glance whether intel came
                        // via direct light-speed, relay, or scout, and whether
                        // it has aged past the stale threshold.
                        let age = clock.elapsed - k.observed_at;
                        let years = age as f64 / crate::time_system::HEXADIES_PER_YEAR as f64;
                        let overlay_source = if age >= crate::knowledge::STALE_THRESHOLD_HEXADIES {
                            crate::knowledge::ObservationSource::Stale
                        } else {
                            k.source
                        };
                        let tag = match overlay_source {
                            crate::knowledge::ObservationSource::Direct => "[DIR]",
                            crate::knowledge::ObservationSource::Relay => "[REL]",
                            crate::knowledge::ObservationSource::Scout => "[SCT]",
                            crate::knowledge::ObservationSource::Stale => "[STALE]",
                        };
                        ui.label(
                            egui::RichText::new(format!("Info age: {:.1} yr {}", years, tag))
                                .weak()
                                .small(),
                        );
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

// ---------------------------------------------------------------------------
// #123: Design-based refit application
// ---------------------------------------------------------------------------

/// Apply the current registered design to a single ship: deduct the refit
/// cost from the system's stockpile and put the ship into the `Refitting`
/// state. No-op if the ship is not eligible (already in sync, design or
/// hull missing, not docked at the given system, etc.).
#[allow(clippy::too_many_arguments)]
fn apply_design_refit(
    ship_entity: Entity,
    system_entity: Entity,
    ships_query: &mut Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    stockpiles: &mut Query<
        (&mut ResourceStockpile, Option<&ResourceCapacity>),
        With<StarSystem>,
    >,
    design_registry: &ShipDesignRegistry,
    hull_registry: &HullRegistry,
    module_registry: &ModuleRegistry,
    now: i64,
) {
    let Ok((_, ship, mut state, _, _, _)) = ships_query.get_mut(ship_entity) else {
        return;
    };
    // Must be docked at the given system (not in transit, not refitting).
    let docked_here = matches!(&*state, ShipState::Docked { system } if *system == system_entity);
    if !docked_here {
        return;
    }
    let Some(design) = design_registry.get(&ship.design_id) else {
        return;
    };
    if design.revision <= ship.design_revision {
        // Already up to date.
        return;
    }
    let Some(hull) = hull_registry.get(&ship.hull_id) else {
        return;
    };
    let (cost_m, cost_e, time) = crate::ship_design::refit_cost_to_design(
        &ship.modules,
        design,
        hull,
        module_registry,
    );
    let new_modules = crate::ship_design::design_equipped_modules(design);
    let target_revision = design.revision;
    if let Ok((mut stockpile, _)) = stockpiles.get_mut(system_entity) {
        // Deduct what we can; refit cost can be zero (e.g. design only renamed).
        stockpile.minerals = stockpile.minerals.sub(cost_m.min(stockpile.minerals));
        stockpile.energy = stockpile.energy.sub(cost_e.min(stockpile.energy));
    }
    *state = ShipState::Refitting {
        system: system_entity,
        started_at: now,
        completes_at: now + time,
        new_modules,
        target_revision,
    };
}

// ---------------------------------------------------------------------------
// #152: draw_choice_dialog_system — modal player-choice dialog
// ---------------------------------------------------------------------------

/// Draws the modal choice dialog (#152) when a `PendingChoice` is active.
/// Options are rendered as vertical buttons; unavailable options are
/// greyed-out with a reason tooltip. Clicking an option stages the selection
/// in `PendingChoiceSelection`; the `apply_pending_choice_selection` system
/// consumes it next tick.
#[allow(clippy::too_many_arguments)]
fn draw_choice_dialog_system(
    mut contexts: EguiContexts,
    ui_state: Res<UiState>,
    mut pending: ResMut<PendingChoice>,
    mut selection: ResMut<PendingChoiceSelection>,
    empire_q: Query<(&TechTree, &GameFlags, &ScopedFlags), With<PlayerEmpire>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    if !pending.is_active() {
        return;
    }
    let Ok((tech_tree, game_flags, scoped_flags)) = empire_q.single() else {
        return;
    };

    // Evaluate availability against current empire state + capital stockpile.
    let capital_stockpile = ui_state.capital_stockpile;
    if let Some(active) = pending.current.as_mut() {
        crate::choice::evaluate_choice_availability(
            active,
            tech_tree,
            game_flags,
            scoped_flags,
            capital_stockpile,
        );
    }

    let Some(active) = pending.current.as_ref() else {
        return;
    };

    let mut pick_index: Option<usize> = None;
    egui::Window::new(egui::RichText::new(&active.title).size(18.0).strong())
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.set_max_width(520.0);
            if !active.description.is_empty() {
                ui.label(egui::RichText::new(&active.description).color(egui::Color32::LIGHT_GRAY));
                ui.add_space(6.0);
            }
            ui.separator();
            ui.add_space(4.0);

            for (i, opt) in active.options.iter().enumerate() {
                let unavailable = opt.condition_unmet || opt.cost_unmet;
                ui.add_enabled_ui(!unavailable, |ui| {
                    let label_text = if opt.cost.is_zero() {
                        opt.label.clone()
                    } else {
                        let mut parts: Vec<String> = Vec::new();
                        if opt.cost.minerals > Amt::ZERO {
                            parts.push(format!("{} M", opt.cost.minerals));
                        }
                        if opt.cost.energy > Amt::ZERO {
                            parts.push(format!("{} E", opt.cost.energy));
                        }
                        if parts.is_empty() {
                            opt.label.clone()
                        } else {
                            format!("{}  ({})", opt.label, parts.join(", "))
                        }
                    };

                    let mut button = egui::Button::new(
                        egui::RichText::new(label_text).strong(),
                    )
                    .min_size(egui::vec2(400.0, 28.0));
                    if unavailable {
                        button = button.fill(egui::Color32::from_rgba_premultiplied(40, 40, 40, 180));
                    }

                    let mut response = ui.add(button);
                    if let Some(desc) = &opt.description {
                        response = response.on_hover_text(desc);
                    }
                    if unavailable && !opt.unmet_reason.is_empty() {
                        response = response.on_disabled_hover_text(&opt.unmet_reason);
                    }
                    if response.clicked() {
                        // 1-based index to match `lua_option_index`.
                        pick_index = Some(i + 1);
                    }
                });
                ui.add_space(2.0);
            }
        });

    if let Some(idx) = pick_index {
        selection.pick = Some(idx);
    }
}
