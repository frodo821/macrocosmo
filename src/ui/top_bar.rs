use bevy_egui::egui;

use crate::amount::Amt;
use crate::time_system::{GameClock, GameSpeed};

use super::ResearchPanelOpen;

pub fn draw_top_bar(
    ctx: &egui::Context,
    clock: &GameClock,
    speed: &mut GameSpeed,
    total_minerals: Amt,
    total_energy: Amt,
    total_food: Amt,
    total_authority: Amt,
    research_open: &mut ResearchPanelOpen,
) {
    egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Year {} Month {} Hexadies {}",
                    clock.year(),
                    clock.month(),
                    clock.hexadies(),
                ))
                .strong(),
            );

            ui.separator();

            if ui.button("\u{23F8}").on_hover_text("Pause").clicked() {
                speed.hexadies_per_second = 0.0;
            }
            if ui.button("\u{25B6}").on_hover_text("Normal speed").clicked() {
                speed.hexadies_per_second = 1.0;
            }
            if ui.button("\u{23E9}").on_hover_text("Fast forward").clicked() {
                speed.hexadies_per_second = (speed.hexadies_per_second * 2.0).max(1.0).min(16.0);
            }

            let speed_text = if speed.hexadies_per_second <= 0.0 {
                "PAUSED".to_string()
            } else {
                format!("x{:.0} hd/s", speed.hexadies_per_second)
            };
            ui.label(&speed_text);

            ui.separator();

            ui.label(format!("F:{}  E:{}  M:{}  A:{}", total_food, total_energy, total_minerals, total_authority));

            ui.separator();

            let r_label = if research_open.0 {
                "Research [open]"
            } else {
                "Research"
            };
            if ui.button(r_label).clicked() {
                research_open.0 = !research_open.0;
            }
        });
    });
}
