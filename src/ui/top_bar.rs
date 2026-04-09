use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildQueue, Colony, Production, ResourceStockpile};
use crate::time_system::{GameClock, GameSpeed};

use super::ResearchPanelOpen;

pub fn draw_top_bar(
    ctx: &egui::Context,
    clock: &GameClock,
    speed: &mut GameSpeed,
    colonies: &Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut ResourceStockpile>,
        Option<&mut BuildQueue>,
    )>,
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

            // Resource summary from colonies query
            let mut total_minerals = 0.0_f64;
            let mut total_energy = 0.0_f64;
            for (_, _, _, stockpile, _) in colonies.iter() {
                if let Some(stockpile) = stockpile {
                    total_minerals += stockpile.minerals;
                    total_energy += stockpile.energy;
                }
            }

            ui.label(format!("M:{:.0}  E:{:.0}", total_minerals, total_energy));

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
