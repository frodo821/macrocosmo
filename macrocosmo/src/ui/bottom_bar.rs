use bevy_egui::egui;

use crate::communication::CommandLog;
use crate::time_system::GameClock;

/// Draws the bottom bar showing the command log / event log.
pub fn draw_bottom_bar(ctx: &egui::Context, command_log: &CommandLog, clock: &GameClock) {
    egui::TopBottomPanel::bottom("bottom_bar")
        .max_height(120.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Event Log").strong());
            ui.separator();

            // Show the last 6 events from the command log
            let entries = &command_log.entries;
            let start = entries.len().saturating_sub(6);
            let recent = &entries[start..];

            if recent.is_empty() {
                ui.label("No events yet.");
            } else {
                egui::ScrollArea::vertical()
                    .max_height(80.0)
                    .show(ui, |ui| {
                        for entry in recent.iter().rev() {
                            let status = if entry.arrived {
                                "arrived"
                            } else {
                                "in transit"
                            };
                            let eta = if entry.arrived {
                                String::new()
                            } else {
                                let remaining = entry.arrives_at - clock.elapsed;
                                format!(" (ETA: {} sd)", remaining.max(0))
                            };
                            ui.label(format!(
                                "[sd {}] {} [{}]{}",
                                entry.sent_at, entry.description, status, eta,
                            ));
                        }
                    });
            }
        });
}
