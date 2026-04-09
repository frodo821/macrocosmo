pub mod bottom_bar;
pub mod outline;
pub mod overlays;
pub mod side_panel;
pub mod top_bar;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};

use crate::colony::{BuildQueue, Colony, Production, ResourceStockpile};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::player::{Player, StationedAt};
use crate::ship::{Cargo, Ship, ShipState};
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
            .add_systems(EguiPrimaryContextPass, draw_all_ui);
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
    mut research_open: ResMut<ResearchPanelOpen>,
    mut selected_system: ResMut<SelectedSystem>,
    mut selected_ship: ResMut<SelectedShip>,
    stars: Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    player_q: Query<&StationedAt, With<Player>>,
    mut colonies: Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
    )>,
    mut ships_query: Query<(Entity, &mut Ship, &mut ShipState, Option<&mut Cargo>)>,
    positions: Query<&Position>,
    knowledge: Res<KnowledgeStore>,
    command_log: Res<CommandLog>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    top_bar::draw_top_bar(ctx, &clock, &mut speed, &colonies, &mut research_open);

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
        &knowledge,
        &clock,
    );

    side_panel::draw_ship_panel(
        ctx,
        &selected_system,
        &mut selected_ship,
        &stars,
        &mut ships_query,
        &positions,
        &clock,
        &mut colonies,
    );

    bottom_bar::draw_bottom_bar(ctx, &command_log, &clock);

    overlays::draw_overlays(ctx, &mut research_open);
}
