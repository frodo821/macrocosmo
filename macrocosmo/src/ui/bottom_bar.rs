use bevy_egui::egui;

use crate::events::EventLog;

/// Draws the bottom bar showing game events with category-based colors.
pub fn draw_bottom_bar(ctx: &egui::Context, event_log: &EventLog) {
    egui::TopBottomPanel::bottom("bottom_bar")
        .max_height(120.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Event Log").strong());
            ui.separator();

            let entries = &event_log.entries;
            let start = entries.len().saturating_sub(8);
            let recent = &entries[start..];

            if recent.is_empty() {
                ui.label("No events yet.");
            } else {
                egui::ScrollArea::vertical()
                    .max_height(80.0)
                    .show(ui, |ui| {
                        for entry in recent.iter().rev() {
                            let [r, g, b] = entry.kind.category().color();
                            let text = format!(
                                "[hd {}] {}",
                                entry.timestamp, entry.description,
                            );
                            ui.label(
                                egui::RichText::new(text)
                                    .color(egui::Color32::from_rgb(r, g, b)),
                            );
                        }
                    });
            }
        });
}
