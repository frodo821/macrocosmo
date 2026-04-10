use bevy::prelude::*;
use bevy_egui::egui;

use crate::amount::Amt;
use crate::ship_design::{
    DesignSlotAssignment, HullRegistry, ModuleRegistry, ShipDesignDefinition, ShipDesignRegistry,
};
use crate::technology::{ResearchPool, ResearchQueue, TechBranch, TechId, TechTree};

use super::ResearchPanelOpen;

/// Resource tracking the ship designer overlay state.
#[derive(Resource)]
pub struct ShipDesignerState {
    pub open: bool,
    pub selected_hull: Option<String>,
    /// Module selection per slot: index is the expanded slot index, value is the module_id.
    pub selected_modules: Vec<Option<String>>,
    pub design_name: String,
}

impl Default for ShipDesignerState {
    fn default() -> Self {
        Self {
            open: false,
            selected_hull: None,
            selected_modules: Vec::new(),
            design_name: String::new(),
        }
    }
}

/// Action returned from the ship designer.
pub enum ShipDesignerAction {
    None,
    SaveDesign(ShipDesignDefinition),
}

/// Draws the ship designer overlay panel.
pub fn draw_ship_designer(
    ctx: &egui::Context,
    state: &mut ShipDesignerState,
    hull_registry: &HullRegistry,
    module_registry: &ModuleRegistry,
    design_registry: &ShipDesignRegistry,
) -> ShipDesignerAction {
    if !state.open {
        return ShipDesignerAction::None;
    }

    let mut action = ShipDesignerAction::None;
    let mut open = state.open;

    egui::Window::new("Ship Designer")
        .open(&mut open)
        .resizable(true)
        .default_size([420.0, 480.0])
        .show(ctx, |ui| {
            // Hull selection
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Hull:").strong());
                let current_hull_name = state
                    .selected_hull
                    .as_ref()
                    .and_then(|id| hull_registry.get(id))
                    .map(|h| h.name.clone())
                    .unwrap_or_else(|| "Select hull...".to_string());

                egui::ComboBox::from_id_salt("hull_select")
                    .selected_text(&current_hull_name)
                    .show_ui(ui, |ui| {
                        let mut hull_ids: Vec<_> = hull_registry.hulls.keys().collect();
                        hull_ids.sort();
                        for hull_id in hull_ids {
                            let hull = &hull_registry.hulls[hull_id];
                            if ui
                                .selectable_label(
                                    state.selected_hull.as_ref() == Some(hull_id),
                                    &hull.name,
                                )
                                .clicked()
                            {
                                let changed = state.selected_hull.as_ref() != Some(hull_id);
                                state.selected_hull = Some(hull_id.clone());
                                if changed {
                                    // Reset module selections when hull changes
                                    let total_slots: usize =
                                        hull.slots.iter().map(|s| s.count as usize).sum();
                                    state.selected_modules = vec![None; total_slots];
                                }
                            }
                        }
                    });
            });

            // Show slots and module selection if a hull is selected
            if let Some(hull) = state
                .selected_hull
                .as_ref()
                .and_then(|id| hull_registry.get(id))
            {
                ui.separator();
                ui.label(egui::RichText::new("Slots:").strong());

                // Expand hull slots into individual slot entries
                let mut slot_idx = 0;
                for hull_slot in &hull.slots {
                    for i in 0..hull_slot.count {
                        let slot_label = if hull_slot.count > 1 {
                            format!("[{}] {}_{}", &hull_slot.slot_type.chars().next().unwrap_or('?').to_uppercase(), hull_slot.slot_type, i + 1)
                        } else {
                            format!("[{}] {}", &hull_slot.slot_type.chars().next().unwrap_or('?').to_uppercase(), hull_slot.slot_type)
                        };

                        let current_module_name = state
                            .selected_modules
                            .get(slot_idx)
                            .and_then(|opt| opt.as_ref())
                            .and_then(|id| module_registry.get(id))
                            .map(|m| m.name.clone())
                            .unwrap_or_else(|| "(empty)".to_string());

                        ui.horizontal(|ui| {
                            ui.label(&slot_label);
                            let combo_id = format!("module_slot_{}", slot_idx);
                            egui::ComboBox::from_id_salt(combo_id)
                                .selected_text(&current_module_name)
                                .show_ui(ui, |ui| {
                                    // Option to clear the slot
                                    if ui
                                        .selectable_label(
                                            state
                                                .selected_modules
                                                .get(slot_idx)
                                                .and_then(|o| o.as_ref())
                                                .is_none(),
                                            "(empty)",
                                        )
                                        .clicked()
                                    {
                                        if slot_idx < state.selected_modules.len() {
                                            state.selected_modules[slot_idx] = None;
                                        }
                                    }
                                    // List compatible modules
                                    let mut mod_ids: Vec<_> = module_registry
                                        .modules
                                        .iter()
                                        .filter(|(_, m)| m.slot_type == hull_slot.slot_type)
                                        .map(|(id, _)| id.clone())
                                        .collect();
                                    mod_ids.sort();
                                    for mod_id in mod_ids {
                                        let module = &module_registry.modules[&mod_id];
                                        let is_selected = state
                                            .selected_modules
                                            .get(slot_idx)
                                            .and_then(|o| o.as_ref())
                                            == Some(&mod_id);
                                        if ui
                                            .selectable_label(is_selected, &module.name)
                                            .clicked()
                                        {
                                            if slot_idx < state.selected_modules.len() {
                                                state.selected_modules[slot_idx] =
                                                    Some(mod_id.clone());
                                            }
                                        }
                                    }
                                });
                        });

                        slot_idx += 1;
                    }
                }

                // Preview stats
                ui.separator();
                ui.label(egui::RichText::new("Preview:").strong());

                let selected_mods: Vec<_> = state
                    .selected_modules
                    .iter()
                    .filter_map(|opt| opt.as_ref())
                    .filter_map(|id| module_registry.get(id))
                    .collect();

                let (hp, speed, evasion) =
                    crate::ship_design::design_stats(hull, &selected_mods);
                let (cost_m, cost_e, build_time, maint) =
                    crate::ship_design::design_cost(hull, &selected_mods);

                ui.label(format!(
                    "HP: {:.0}  Speed: {:.2}  Evasion: {:.0}",
                    hp, speed, evasion
                ));
                ui.label(format!(
                    "Cost: M:{} E:{}  Time: {} hd",
                    cost_m.display(),
                    cost_e.display(),
                    build_time
                ));
                ui.label(format!("Maintenance: {}/hd", maint.display()));

                // Design name input
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Design Name:");
                    ui.text_edit_singleline(&mut state.design_name);
                });

                // Save button
                ui.horizontal(|ui| {
                    let name_valid = !state.design_name.trim().is_empty();
                    let has_modules = state.selected_modules.iter().any(|m| m.is_some());

                    let can_save = name_valid && has_modules;

                    if ui
                        .add_enabled(can_save, egui::Button::new("Save Design"))
                        .clicked()
                    {
                        // Build the design definition
                        let design_id = format!(
                            "custom_{}",
                            state
                                .design_name
                                .trim()
                                .to_lowercase()
                                .replace(' ', "_")
                        );

                        // Check for duplicate ID
                        if design_registry.get(&design_id).is_some() {
                            // Just append a number to make it unique
                            let mut counter = 2;
                            let mut unique_id = format!("{}_{}", design_id, counter);
                            while design_registry.get(&unique_id).is_some() {
                                counter += 1;
                                unique_id = format!("{}_{}", design_id, counter);
                            }
                            let modules = build_design_modules(state, hull);
                            action = ShipDesignerAction::SaveDesign(ShipDesignDefinition {
                                id: unique_id,
                                name: state.design_name.trim().to_string(),
                                hull_id: hull.id.clone(),
                                modules,
                            });
                        } else {
                            let modules = build_design_modules(state, hull);
                            action = ShipDesignerAction::SaveDesign(ShipDesignDefinition {
                                id: design_id,
                                name: state.design_name.trim().to_string(),
                                hull_id: hull.id.clone(),
                                modules,
                            });
                        }
                    }

                    if ui.button("Cancel").clicked() {
                        state.open = false;
                    }
                });
            } else {
                ui.separator();
                ui.label(
                    egui::RichText::new("Select a hull to begin designing.")
                        .weak()
                        .italics(),
                );
            }
        });

    // Write back open state (user may have closed the window via the X button)
    state.open = open;

    action
}

/// Build DesignSlotAssignment list from the designer state.
fn build_design_modules(
    state: &ShipDesignerState,
    hull: &crate::ship_design::HullDefinition,
) -> Vec<DesignSlotAssignment> {
    let mut modules = Vec::new();
    let mut slot_idx = 0;
    for hull_slot in &hull.slots {
        for _i in 0..hull_slot.count {
            if let Some(Some(mod_id)) = state.selected_modules.get(slot_idx) {
                modules.push(DesignSlotAssignment {
                    slot_type: hull_slot.slot_type.clone(),
                    module_id: mod_id.clone(),
                });
            }
            slot_idx += 1;
        }
    }
    modules
}

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
            if let Some(ref current_id) = research_queue.current {
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
                        let is_researched = tech_tree.is_researched(&tech.id);
                        let is_current = research_queue.current.as_ref() == Some(&tech.id);
                        let is_available = tech_tree.can_research(&tech.id);

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
                                        action = ResearchAction::StartResearch(tech.id.clone());
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
                                    .filter(|pre| !tech_tree.is_researched(pre))
                                    .filter_map(|pre| {
                                        tech_tree.get(pre).map(|t| t.name.clone())
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
