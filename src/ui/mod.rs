pub mod bottom_bar;
pub mod overlays;
pub mod side_panel;
pub mod top_bar;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin};

use crate::colony::{BuildQueue, Colony, Production, ResourceStockpile};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::player::{Player, StationedAt};
use crate::ship::{Ship, ShipState};
use crate::time_system::{GameClock, GameSpeed};
use crate::visualization::{SelectedShip, SelectedSystem};

/// Resource tracking whether the research overlay is open.
#[derive(Resource, Default)]
pub struct ResearchPanelOpen(pub bool);

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<ResearchPanelOpen>()
            .add_systems(Update, draw_all_ui);
    }
}

/// Single unified UI system. All egui panels must be drawn from the same
/// system to avoid the "available_rect() before Context::run()" panic
/// that occurs when multiple systems try to access EguiContexts concurrently.
#[allow(clippy::too_many_arguments)]
pub fn draw_all_ui(
    mut contexts: EguiContexts,
    clock: Res<GameClock>,
    mut speed: ResMut<GameSpeed>,
    stockpiles: Query<&ResourceStockpile>,
    mut research_open: ResMut<ResearchPanelOpen>,
    selected_system: Res<SelectedSystem>,
    mut selected_ship: ResMut<SelectedShip>,
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: Query<&StationedAt, With<Player>>,
    mut colonies: Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&ResourceStockpile>,
        Option<&mut BuildQueue>,
    )>,
    mut ships_query: Query<(Entity, &mut Ship, &mut ShipState)>,
    positions: Query<&Position>,
    knowledge: Res<KnowledgeStore>,
    command_log: Res<CommandLog>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    top_bar::draw_top_bar(ctx, &clock, &mut speed, &stockpiles, &mut research_open);

    side_panel::draw_side_panel(
        ctx,
        &selected_system,
        &mut selected_ship,
        &stars,
        &player_q,
        &mut colonies,
        &mut ships_query,
        &positions,
        &knowledge,
        &clock,
    );

    bottom_bar::draw_bottom_bar(ctx, &command_log, &clock);

    overlays::draw_overlays(ctx, &mut research_open);
}
