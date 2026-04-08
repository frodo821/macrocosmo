use bevy_egui::egui;

use super::ResearchPanelOpen;

/// Draws modal overlay windows (e.g. the research panel).
///
/// Currently this is a placeholder since the technology/research system
/// has not been implemented yet. When TechTree, ResearchQueue, and
/// ResearchPool are added, this will display the full tech tree with
/// clickable research options.
pub fn draw_overlays(
    ctx: &egui::Context,
    research_open: &mut ResearchPanelOpen,
) {
    if !research_open.0 {
        return;
    }

    egui::Window::new("Research")
        .open(&mut research_open.0)
        .resizable(true)
        .default_size([400.0, 300.0])
        .show(ctx, |ui| {
            ui.label("Technology research is not yet implemented.");
            ui.separator();
            ui.label("When the technology system is added, this panel will show:");
            ui.label("- Tech tree grouped by branch");
            ui.label("- Status: Researched / Available / Locked");
            ui.label("- Click to start researching a technology");
        });
}
