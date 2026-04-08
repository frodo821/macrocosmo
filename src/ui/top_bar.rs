use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::ResourceStockpile;
use crate::time_system::{GameClock, GameSpeed};

use super::ResearchPanelOpen;

pub fn draw_top_bar(
    ctx: &egui::Context,
    clock: &GameClock,
    speed: &mut GameSpeed,
    stockpiles: &Query<&ResourceStockpile>,
    research_open: &mut ResearchPanelOpen,
) {
    egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            // Game time
            ui.label(
                egui::RichText::new(format!(
                    "Year {} Month {} Sexadie {}",
                    clock.year(),
                    clock.month(),
                    clock.sexadie(),
                ))
                .strong(),
            );

            ui.separator();

            // Speed controls
            if ui.button("\u{23F8}").on_hover_text("Pause").clicked() {
                speed.sexadies_per_second = 0.0;
            }
            if ui.button("\u{25B6}").on_hover_text("Normal speed").clicked() {
                speed.sexadies_per_second = 1.0;
            }
            if ui.button("\u{23E9}").on_hover_text("Fast forward").clicked() {
                speed.sexadies_per_second = (speed.sexadies_per_second * 2.0).max(1.0);
            }

            let speed_text = if speed.sexadies_per_second <= 0.0 {
                "PAUSED".to_string()
            } else {
                format!("x{:.0} sd/s", speed.sexadies_per_second)
            };
            ui.label(&speed_text);

            ui.separator();

            // Resource summary (total across all colonies)
            let mut total_minerals = 0.0_f64;
            let mut total_energy = 0.0_f64;
            let mut total_research = 0.0_f64;
            for stockpile in stockpiles {
                total_minerals += stockpile.minerals;
                total_energy += stockpile.energy;
                total_research += stockpile.research;
            }

            ui.label(format!(
                "M:{:.0}  E:{:.0}  R:{:.0}",
                total_minerals, total_energy, total_research,
            ));

            ui.separator();

            // Research toggle
            let r_label = if research_open.0 {
                "R: Research [open]"
            } else {
                "R: Research"
            };
            if ui.button(r_label).clicked() {
                research_open.0 = !research_open.0;
            }
        });
    });
}
