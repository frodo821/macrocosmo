mod top_bar;
mod side_panel;
mod bottom_bar;
mod overlays;

use bevy::prelude::*;
use bevy_egui::EguiPlugin;

/// Resource tracking whether the research overlay is open.
#[derive(Resource, Default)]
pub struct ResearchPanelOpen(pub bool);

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<ResearchPanelOpen>()
            .add_systems(
                Update,
                (
                    top_bar::draw_top_bar,
                    side_panel::draw_side_panel,
                    bottom_bar::draw_bottom_bar,
                    overlays::draw_overlays,
                ),
            );
    }
}
