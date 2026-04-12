//! Governor tab: per-faction overview of canonical Tier 1 metrics.
//!
//! v1 shows one faction at a time. Campaign / Standing / Objective
//! blocks are placeholders until #189 / #193 / #204 land.

use bevy_egui::egui;
use macrocosmo_ai::{AiBus, MetricId};

use crate::ai::schema::ids::metric as m;

use super::GovernorState;

/// Canonical Tier 1 metrics grouped by category for the Governor overview.
fn metric_groups() -> Vec<(&'static str, Vec<MetricId>)> {
    vec![
        (
            "Military",
            vec![
                m::my_total_ships(),
                m::my_strength(),
                m::my_fleet_ready(),
                m::my_vulnerability_score(),
                m::my_has_flagship(),
            ],
        ),
        (
            "Economy",
            vec![
                m::net_production_minerals(),
                m::net_production_energy(),
                m::net_production_food(),
                m::net_production_research(),
                m::net_production_authority(),
                m::food_surplus(),
            ],
        ),
        (
            "Stockpiles",
            vec![
                m::stockpile_minerals(),
                m::stockpile_energy(),
                m::stockpile_food(),
                m::stockpile_authority(),
            ],
        ),
        (
            "Population",
            vec![
                m::population_total(),
                m::population_growth_rate(),
                m::population_carrying_capacity(),
                m::population_ratio(),
            ],
        ),
        (
            "Territory",
            vec![
                m::colony_count(),
                m::colonized_system_count(),
                m::border_system_count(),
                m::habitable_systems_known(),
                m::colonizable_systems_remaining(),
                m::systems_with_hostiles(),
            ],
        ),
        (
            "Technology",
            vec![
                m::tech_total_researched(),
                m::tech_completion_percent(),
                m::tech_unlocks_available(),
                m::research_output_ratio(),
            ],
        ),
        (
            "Infrastructure",
            vec![
                m::systems_with_shipyard(),
                m::systems_with_port(),
                m::max_building_slots(),
                m::used_building_slots(),
                m::free_building_slots(),
                m::can_build_ships(),
            ],
        ),
        (
            "Diplomacy / Time",
            vec![
                m::game_elapsed_time(),
                m::number_of_allies(),
                m::number_of_enemies(),
            ],
        ),
    ]
}

pub fn draw_governor(ui: &mut egui::Ui, state: &mut GovernorState, bus: &AiBus) {
    ui.horizontal(|ui| {
        ui.label("Faction:");
        ui.add(egui::DragValue::new(&mut state.faction).range(0..=255));
        ui.label(
            egui::RichText::new(format!("Faction({})", state.faction))
                .weak()
                .small(),
        );
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Metrics").heading());
            for (group, ids) in metric_groups() {
                egui::CollapsingHeader::new(group)
                    .id_salt(format!("ai_debug_governor_group_{}", group))
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new(format!("ai_debug_governor_grid_{}", group))
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                for id in ids {
                                    ui.label(id.as_str());
                                    let value = bus
                                        .current(&id)
                                        .map(|v| format!("{:.4}", v))
                                        .unwrap_or_else(|| "—".into());
                                    ui.label(value);
                                    ui.end_row();
                                }
                            });
                    });
            }

            ui.add_space(10.0);
            ui.separator();
            ui.label(egui::RichText::new("Standing").heading());
            ui.label(
                egui::RichText::new("未実装 — #193 Perceived Standing 統合待ち")
                    .weak()
                    .italics(),
            );

            ui.add_space(10.0);
            ui.separator();
            ui.label(egui::RichText::new("Campaigns").heading());
            ui.label(
                egui::RichText::new("未実装 — #204 FleetCombatCapability 統合待ち")
                    .weak()
                    .italics(),
            );

            ui.add_space(10.0);
            ui.separator();
            ui.label(egui::RichText::new("Objectives").heading());
            ui.label(
                egui::RichText::new("未実装 — #189 AI umbrella 実装待ち")
                    .weak()
                    .italics(),
            );
        });
}
