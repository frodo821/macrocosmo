use bevy_egui::egui;

use crate::amount::{Amt, SignedAmt};
use crate::time_system::{GameClock, GameSpeed};

use super::ResearchPanelOpen;
use super::overlays::ShipDesignerState;

#[allow(clippy::too_many_arguments)]
pub fn draw_top_bar(
    ctx: &egui::Context,
    clock: &GameClock,
    speed: &mut GameSpeed,
    total_minerals: Amt,
    total_energy: Amt,
    total_food: Amt,
    total_authority: Amt,
    net_food: SignedAmt,
    net_energy: SignedAmt,
    net_minerals: SignedAmt,
    net_authority: SignedAmt,
    research_open: &mut ResearchPanelOpen,
    designer_state: &mut ShipDesignerState,
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

            // Resource stockpiles with net income
            for (label, stockpile, net) in [
                ("F", total_food, net_food),
                ("E", total_energy, net_energy),
                ("M", total_minerals, net_minerals),
                ("A", total_authority, net_authority),
            ] {
                ui.label(format!("{}:{}", label, stockpile.display_compact()));
                let net_color = if net.raw() > 0 {
                    egui::Color32::from_rgb(100, 200, 100)
                } else if net.raw() < 0 {
                    egui::Color32::from_rgb(255, 100, 100)
                } else {
                    egui::Color32::GRAY
                };
                ui.label(egui::RichText::new(format!("({})", net.display_compact())).color(net_color));
            }

            ui.separator();

            let r_label = if research_open.0 {
                "Research [open]"
            } else {
                "Research"
            };
            if ui.button(r_label).clicked() {
                research_open.0 = !research_open.0;
            }

            let d_label = if designer_state.open {
                "Ship Designer [open]"
            } else {
                "Ship Designer"
            };
            if ui.button(d_label).clicked() {
                designer_state.open = !designer_state.open;
            }
        });
    });
}
