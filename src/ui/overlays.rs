use bevy_egui::egui;

use crate::amount::Amt;
use crate::technology::{ResearchPool, ResearchQueue, TechBranch, TechId, TechTree};

use super::ResearchPanelOpen;

/// Action requested by the research panel UI.
/// The caller (draw_all_ui) is responsible for executing these since the overlay
/// only has immutable access to colony stockpiles.
pub enum ResearchAction {
    None,
    StartResearch(TechId),
    CancelResearch,
}

/// Draws the research overlay panel.
///
/// Returns a `ResearchAction` so the caller can apply mutations that require
/// mutable access to colony stockpiles (upfront cost deduction).
pub fn draw_overlays(
    ctx: &egui::Context,
    research_open: &mut ResearchPanelOpen,
    tech_tree: &TechTree,
    research_queue: &ResearchQueue,
    research_pool: &ResearchPool,
    capital_stockpile: Option<(&Amt, &Amt)>,
    _clock_elapsed: i64,
) -> ResearchAction {
    if !research_open.0 {
        return ResearchAction::None;
    }

    let mut action = ResearchAction::None;

    egui::Window::new("Research")
        .open(&mut research_open.0)
        .resizable(true)
        .default_size([520.0, 500.0])
        .show(ctx, |ui| {
            // --- Research pool display ---
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Research Pool: {:.1} RP/hd",
                        research_pool.points
                    ))
                    .strong(),
                );
            });

            ui.separator();

            // --- Current research ---
            if let Some(current_id) = research_queue.current {
                if let Some(tech) = tech_tree.get(current_id) {
                    let cost = tech.cost.research.to_f64();
                    let progress = if cost > 0.0 {
                        (research_queue.accumulated as f32 / cost as f32).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Current: {}", tech.name)).strong(),
                        );
                    });

                    ui.add(
                        egui::ProgressBar::new(progress).text(format!(
                            "{:.0}/{} RP",
                            research_queue.accumulated,
                            tech.cost.research.display()
                        )),
                    );

                    if research_queue.blocked {
                        ui.label(
                            egui::RichText::new("[Blocked]")
                                .color(egui::Color32::from_rgb(255, 100, 100)),
                        );
                    }

                    if ui.button("Cancel Research").clicked() {
                        action = ResearchAction::CancelResearch;
                    }
                }
            } else {
                ui.label("No active research project.");
            }

            ui.separator();

            // --- Branch tabs ---
            let selected_branch_id = egui::Id::new("research_selected_branch");
            let mut selected_idx: usize = ui
                .memory(|m| m.data.get_temp(selected_branch_id))
                .unwrap_or(0);

            let branches = TechBranch::all();
            ui.horizontal(|ui| {
                for (i, branch) in branches.iter().enumerate() {
                    if ui.selectable_label(selected_idx == i, branch.name()).clicked() {
                        selected_idx = i;
                    }
                }
            });
            ui.memory_mut(|m| m.data.insert_temp(selected_branch_id, selected_idx));

            let selected_branch = branches[selected_idx];

            ui.separator();

            // --- Tech list for selected branch ---
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let techs = tech_tree.techs_in_branch(selected_branch);
                    for tech in &techs {
                        let is_researched = tech_tree.is_researched(tech.id);
                        let is_current = research_queue.current == Some(tech.id);
                        let is_available = tech_tree.can_research(tech.id);

                        ui.group(|ui| {
                            // Header line: status + name + cost
                            ui.horizontal(|ui| {
                                // Status label
                                if is_researched {
                                    ui.label(
                                        egui::RichText::new("[Done]")
                                            .color(egui::Color32::from_rgb(100, 220, 100)),
                                    );
                                } else if is_current {
                                    ui.label(
                                        egui::RichText::new("[Researching]")
                                            .color(egui::Color32::from_rgb(255, 220, 80)),
                                    );
                                } else if is_available {
                                    // no status label for available
                                } else {
                                    ui.label(
                                        egui::RichText::new("[Locked]")
                                            .color(egui::Color32::from_rgb(140, 140, 140)),
                                    );
                                }

                                // Tech name
                                let name_text = if is_researched {
                                    egui::RichText::new(&tech.name)
                                        .color(egui::Color32::from_rgb(100, 220, 100))
                                } else if !is_available && !is_current {
                                    egui::RichText::new(&tech.name)
                                        .color(egui::Color32::from_rgb(140, 140, 140))
                                } else {
                                    egui::RichText::new(&tech.name)
                                };
                                ui.label(name_text.strong());

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        // Cost display
                                        let mut cost_parts =
                                            vec![format!("{} RP", tech.cost.research.display())];
                                        if tech.cost.minerals > Amt::ZERO {
                                            cost_parts.push(format!(
                                                "M:{}",
                                                tech.cost.minerals.display()
                                            ));
                                        }
                                        if tech.cost.energy > Amt::ZERO {
                                            cost_parts.push(format!(
                                                "E:{}",
                                                tech.cost.energy.display()
                                            ));
                                        }
                                        ui.label(cost_parts.join(" | "));
                                    },
                                );
                            });

                            // Description
                            if !tech.description.is_empty() {
                                ui.label(
                                    egui::RichText::new(&tech.description)
                                        .weak()
                                        .italics(),
                                );
                            }

                            // Action row
                            if is_current {
                                let cost = tech.cost.research.to_f64();
                                let pct = if cost > 0.0 {
                                    (research_queue.accumulated / cost * 100.0).min(100.0)
                                } else {
                                    100.0
                                };
                                ui.label(
                                    egui::RichText::new(format!("Researching - {:.0}%", pct))
                                        .color(egui::Color32::from_rgb(255, 220, 80)),
                                );
                            } else if is_available && research_queue.current.is_none() {
                                // Check affordability of upfront costs
                                let can_afford = match capital_stockpile {
                                    Some((minerals, energy)) => {
                                        tech.cost.minerals <= *minerals
                                            && tech.cost.energy <= *energy
                                    }
                                    None => {
                                        // No capital stockpile — only allow if no upfront cost
                                        tech.cost.minerals == Amt::ZERO
                                            && tech.cost.energy == Amt::ZERO
                                    }
                                };

                                if can_afford {
                                    if ui.button("Start Research").clicked() {
                                        action = ResearchAction::StartResearch(tech.id);
                                    }
                                } else {
                                    ui.add_enabled(false, egui::Button::new("Start Research"))
                                        .on_disabled_hover_text("Insufficient resources at capital");
                                }
                            } else if is_available {
                                // Another tech is currently being researched
                                ui.label(
                                    egui::RichText::new("Available (finish current research first)")
                                        .weak(),
                                );
                            } else if !is_researched {
                                // Locked — show prerequisite names
                                let missing: Vec<String> = tech
                                    .prerequisites
                                    .iter()
                                    .filter(|pre| !tech_tree.is_researched(**pre))
                                    .filter_map(|pre| {
                                        tech_tree.get(*pre).map(|t| t.name.clone())
                                    })
                                    .collect();
                                if !missing.is_empty() {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Requires: {}",
                                            missing.join(", ")
                                        ))
                                        .color(egui::Color32::from_rgb(140, 140, 140)),
                                    );
                                }
                            }
                        });

                        ui.add_space(2.0);
                    }
                });
        });

    action
}
