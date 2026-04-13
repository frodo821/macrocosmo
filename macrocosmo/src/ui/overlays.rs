use bevy::prelude::*;
use bevy_egui::egui;

use crate::amount::Amt;
use crate::ship_design::{
    DesignSlotAssignment, HullRegistry, ModuleRegistry, ShipDesignDefinition, ShipDesignRegistry,
};
use crate::technology::{
    ResearchPool, ResearchQueue, TechBranchRegistry, TechEffectsPreview, TechId, TechTree,
    TechUnlockIndex, UnlockKind,
};

use super::ResearchPanelOpen;

/// Resource tracking the ship designer overlay state.
#[derive(Resource)]
pub struct ShipDesignerState {
    pub open: bool,
    pub selected_hull: Option<String>,
    /// Module selection per slot: index is the expanded slot index, value is the module_id.
    pub selected_modules: Vec<Option<String>>,
    pub design_name: String,
    /// #123: When `Some`, the designer is editing the design with this ID.
    /// Saving will update that design in place and bump its `revision`,
    /// flagging existing ships of that design as "needs refit".
    /// When `None`, saving creates a new design (id derived from the name).
    pub editing_design_id: Option<String>,
}

impl Default for ShipDesignerState {
    fn default() -> Self {
        Self {
            open: false,
            selected_hull: None,
            selected_modules: Vec::new(),
            design_name: String::new(),
            editing_design_id: None,
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
            // #123: Existing design picker (edit-in-place support).
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Edit existing:").strong());
                let editing_label = state
                    .editing_design_id
                    .as_ref()
                    .and_then(|id| design_registry.get(id))
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "(new design)".to_string());
                egui::ComboBox::from_id_salt("designer_edit_existing")
                    .selected_text(&editing_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                state.editing_design_id.is_none(),
                                "(new design)",
                            )
                            .clicked()
                        {
                            state.editing_design_id = None;
                            state.selected_hull = None;
                            state.selected_modules.clear();
                            state.design_name.clear();
                        }
                        let mut design_ids = design_registry.all_design_ids();
                        design_ids.sort();
                        for did in &design_ids {
                            let Some(def) = design_registry.get(did) else {
                                continue;
                            };
                            let is_selected = state.editing_design_id.as_deref() == Some(did);
                            if ui.selectable_label(is_selected, &def.name).clicked() && !is_selected {
                                // Populate state from the chosen design.
                                state.editing_design_id = Some(def.id.clone());
                                state.selected_hull = Some(def.hull_id.clone());
                                state.design_name = def.name.clone();
                                if let Some(hull) = hull_registry.get(&def.hull_id) {
                                    let total_slots: usize =
                                        hull.slots.iter().map(|s| s.count as usize).sum();
                                    let mut selections = vec![None; total_slots];
                                    // Greedy fill: walk per-slot-type and assign in order.
                                    let mut slot_idx = 0;
                                    for hull_slot in &hull.slots {
                                        let mut taken = 0u32;
                                        for assignment in def.modules.iter() {
                                            if assignment.slot_type == hull_slot.slot_type
                                                && taken < hull_slot.count
                                            {
                                                selections[slot_idx + taken as usize] =
                                                    Some(assignment.module_id.clone());
                                                taken += 1;
                                            }
                                        }
                                        slot_idx += hull_slot.count as usize;
                                    }
                                    state.selected_modules = selections;
                                }
                            }
                        }
                    });
                if state.editing_design_id.is_some()
                    && ui.button("New").clicked()
                {
                    state.editing_design_id = None;
                    state.selected_hull = None;
                    state.selected_modules.clear();
                    state.design_name.clear();
                }
            });

            ui.separator();

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
                    cost_m.display_compact(),
                    cost_e.display_compact(),
                    build_time
                ));
                ui.label(format!("Maintenance: {}/hd", maint.display_compact()));

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

                    let save_label = if state.editing_design_id.is_some() {
                        "Save Design (bumps revision)"
                    } else {
                        "Save Design"
                    };
                    if ui
                        .add_enabled(can_save, egui::Button::new(save_label))
                        .clicked()
                    {
                        // #123: When editing an existing design, keep its ID
                        // so the registry replaces in-place and bumps the
                        // revision counter (flagging existing ships of that
                        // design as needing refit). Otherwise allocate a
                        // fresh `custom_<name>` ID, deduplicating against
                        // the registry.
                        let design_id = if let Some(id) = state.editing_design_id.as_ref() {
                            id.clone()
                        } else {
                            let base = format!(
                                "custom_{}",
                                state
                                    .design_name
                                    .trim()
                                    .to_lowercase()
                                    .replace(' ', "_")
                            );
                            if design_registry.get(&base).is_some() {
                                let mut counter = 2;
                                let mut unique_id = format!("{}_{}", base, counter);
                                while design_registry.get(&unique_id).is_some() {
                                    counter += 1;
                                    unique_id = format!("{}_{}", base, counter);
                                }
                                unique_id
                            } else {
                                base
                            }
                        };

                        let modules = build_design_modules(state, hull);
                        let mod_defs: Vec<_> = modules
                            .iter()
                            .filter_map(|a| module_registry.get(&a.module_id))
                            .collect();
                        // #236: all stats/cost/capabilities derived from hull
                        // + modules via the shared helper. Hull modifiers are
                        // applied (previously ignored by the inline compute).
                        let d = crate::ship_design::design_derived(hull, &mod_defs);
                        action = ShipDesignerAction::SaveDesign(ShipDesignDefinition {
                            id: design_id,
                            name: state.design_name.trim().to_string(),
                            description: String::new(),
                            hull_id: hull.id.clone(),
                            modules,
                            can_survey: d.can_survey,
                            can_colonize: d.can_colonize,
                            maintenance: d.maintenance,
                            build_cost_minerals: d.build_cost_minerals,
                            build_cost_energy: d.build_cost_energy,
                            build_time: d.build_time,
                            hp: d.hp,
                            sublight_speed: d.sublight_speed,
                            ftl_range: d.ftl_range,
                            // Revision is filled in by `upsert_edited`.
                            revision: 0,
                        });
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

/// True when a tech should be surfaced with a warning badge in the research
/// panel. Extracted as a helper so tests can verify detection logic without
/// spinning up egui.
pub fn tech_is_dangerous(tech: &crate::technology::Technology) -> bool {
    tech.dangerous
}

/// Collect the Tech-kind follow-ons reachable via `TechUnlockIndex` — the
/// "leads to" list shown under a tech's details. Skips Module/Building/
/// Structure entries which have their own Unlocks section.
pub fn tech_follow_ons<'a>(
    unlock_index: &'a TechUnlockIndex,
    tech_id: &TechId,
) -> Vec<&'a crate::technology::UnlockEntry> {
    unlock_index
        .for_tech(&tech_id.0)
        .iter()
        .filter(|e| e.kind == UnlockKind::Tech)
        .collect()
}

/// Draws the research overlay panel.
///
/// Returns a `ResearchAction` so the caller can apply mutations that require
/// mutable access to colony stockpiles (upfront cost deduction).
#[allow(clippy::too_many_arguments)]
pub fn draw_overlays(
    ctx: &egui::Context,
    research_open: &mut ResearchPanelOpen,
    tech_tree: &TechTree,
    research_queue: &ResearchQueue,
    research_pool: &ResearchPool,
    branch_registry: &TechBranchRegistry,
    effects_preview: &TechEffectsPreview,
    unlock_index: &TechUnlockIndex,
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
                            tech.cost.research.display_compact()
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

            let branches: Vec<&crate::technology::TechBranchDefinition> =
                branch_registry.iter_ordered().collect();
            if branches.is_empty() {
                ui.label(
                    egui::RichText::new("No tech branches defined.")
                        .weak()
                        .italics(),
                );
                return;
            }

            if selected_idx >= branches.len() {
                selected_idx = 0;
            }

            ui.horizontal(|ui| {
                for (i, branch) in branches.iter().enumerate() {
                    let [r, g, b] = branch.color;
                    let color = egui::Color32::from_rgb(
                        (r.clamp(0.0, 1.0) * 255.0) as u8,
                        (g.clamp(0.0, 1.0) * 255.0) as u8,
                        (b.clamp(0.0, 1.0) * 255.0) as u8,
                    );
                    let label = egui::RichText::new(&branch.name).color(color);
                    if ui.selectable_label(selected_idx == i, label).clicked() {
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
                    let techs = tech_tree.techs_in_branch(&selected_branch.id);
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

                                // #137: Dangerous-tech warning badge. Shown
                                // regardless of researched state so the player
                                // can see, after the fact, what has been
                                // unleashed.
                                if tech.dangerous {
                                    ui.label(
                                        egui::RichText::new("[!] Dangerous")
                                            .color(egui::Color32::from_rgb(255, 120, 60))
                                            .strong(),
                                    )
                                    .on_hover_text(
                                        "This technology has significant or risky consequences. \
                                         Starting research requires confirmation.",
                                    );
                                }

                                // Tech name
                                let name_text = if is_researched {
                                    egui::RichText::new(&tech.name)
                                        .color(egui::Color32::from_rgb(100, 220, 100))
                                } else if tech.dangerous {
                                    // Dangerous techs are tinted even when
                                    // available so they visually stand apart.
                                    egui::RichText::new(&tech.name)
                                        .color(egui::Color32::from_rgb(255, 160, 90))
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
                                            vec![format!("{} RP", tech.cost.research.display_compact())];
                                        if tech.cost.minerals > Amt::ZERO {
                                            cost_parts.push(format!(
                                                "M:{}",
                                                tech.cost.minerals.display_compact()
                                            ));
                                        }
                                        if tech.cost.energy > Amt::ZERO {
                                            cost_parts.push(format!(
                                                "E:{}",
                                                tech.cost.energy.display_compact()
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

                            // #156: Effects + Unlocks (collapsible per tech).
                            // Only render the section when there's something
                            // to show, to keep already-cluttered tech rows
                            // readable.
                            // #137: Split the Tech-kind entries from the rest
                            // so we can show a dedicated "Leads to" list of
                            // follow-on technologies above concrete unlocks
                            // (modules / buildings / structures).
                            let preview = effects_preview.for_tech(&tech.id);
                            let unlocks = unlock_index.for_tech(&tech.id.0);
                            let (tech_follow_ons, concrete_unlocks): (Vec<_>, Vec<_>) =
                                unlocks.iter().partition(|e| e.kind == UnlockKind::Tech);
                            if !preview.is_empty() || !unlocks.is_empty() {
                                let header_id =
                                    egui::Id::new(("research_details", &tech.id.0));
                                egui::CollapsingHeader::new("Details")
                                    .id_salt(header_id)
                                    .default_open(false)
                                    .show(ui, |ui| {
                                        if !preview.is_empty() {
                                            ui.label(
                                                egui::RichText::new("Effects:").strong(),
                                            );
                                            for effect in preview {
                                                ui.label(format!("  - {}", effect.display_text()));
                                            }
                                        }
                                        if !tech_follow_ons.is_empty() {
                                            ui.label(
                                                egui::RichText::new("Leads to:")
                                                    .strong()
                                                    .color(egui::Color32::from_rgb(180, 200, 255)),
                                            );
                                            for entry in &tech_follow_ons {
                                                // Flag a dangerous follow-on so
                                                // the player can see what lies
                                                // ahead before committing.
                                                let is_dangerous = tech_tree
                                                    .get(&TechId(entry.id.clone()))
                                                    .map(|t| t.dangerous)
                                                    .unwrap_or(false);
                                                let text = if is_dangerous {
                                                    egui::RichText::new(format!(
                                                        "  - {} [!]",
                                                        entry.name
                                                    ))
                                                    .color(egui::Color32::from_rgb(255, 160, 90))
                                                } else {
                                                    egui::RichText::new(format!(
                                                        "  - {}",
                                                        entry.name
                                                    ))
                                                };
                                                ui.label(text);
                                            }
                                        }
                                        if !concrete_unlocks.is_empty() {
                                            ui.label(
                                                egui::RichText::new("Unlocks:").strong(),
                                            );
                                            for entry in &concrete_unlocks {
                                                let kind_label = match entry.kind {
                                                    UnlockKind::Module => "Module",
                                                    UnlockKind::Building => "Building",
                                                    UnlockKind::Structure => "Structure",
                                                    UnlockKind::Hull => "Hull",
                                                    UnlockKind::ShipDesign => "Ship Design",
                                                    // `Tech` entries handled in the "Leads to" list above.
                                                    UnlockKind::Tech => continue,
                                                };
                                                ui.label(format!(
                                                    "  - {}: {}",
                                                    kind_label, entry.name
                                                ));
                                            }
                                        }
                                    });
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
                                    let btn_label = if tech.dangerous {
                                        "Start Research [!]"
                                    } else {
                                        "Start Research"
                                    };
                                    let btn = if tech.dangerous {
                                        egui::Button::new(
                                            egui::RichText::new(btn_label)
                                                .color(egui::Color32::from_rgb(255, 160, 90))
                                                .strong(),
                                        )
                                    } else {
                                        egui::Button::new(btn_label)
                                    };
                                    if ui.add(btn).clicked() {
                                        if tech.dangerous {
                                            // Defer action until the
                                            // confirmation modal is acknowledged.
                                            ui.memory_mut(|m| {
                                                m.data.insert_temp(
                                                    egui::Id::new("research_dangerous_confirm"),
                                                    tech.id.0.clone(),
                                                );
                                            });
                                        } else {
                                            action =
                                                ResearchAction::StartResearch(tech.id.clone());
                                        }
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

            // #137: Dangerous-tech confirmation modal. Rendered last so it
            // sits above the tech list. The pending TechId is stashed in
            // egui temp memory by the Start Research button, so we only need
            // to read/clear it here and translate Confirm -> StartResearch.
            let confirm_id = egui::Id::new("research_dangerous_confirm");
            let pending: Option<String> = ui.memory(|m| m.data.get_temp(confirm_id));
            if let Some(pending_id) = pending {
                let tech_opt = tech_tree.get(&TechId(pending_id.clone()));
                let tech_name = tech_opt
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| pending_id.clone());
                let mut open = true;
                let mut decided: Option<bool> = None;
                egui::Window::new(
                    egui::RichText::new("Confirm Dangerous Research")
                        .color(egui::Color32::from_rgb(255, 160, 90))
                        .strong(),
                )
                .id(egui::Id::new("research_dangerous_confirm_window"))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "\"{}\" is flagged as dangerous.",
                            tech_name
                        ))
                        .strong(),
                    );
                    ui.label(
                        "Researching it may have significant or irreversible consequences.",
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("Proceed")
                                    .color(egui::Color32::from_rgb(255, 160, 90))
                                    .strong(),
                            ))
                            .clicked()
                        {
                            decided = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decided = Some(false);
                        }
                    });
                });
                // Window close (X) counts as cancel.
                if !open && decided.is_none() {
                    decided = Some(false);
                }
                if let Some(confirmed) = decided {
                    ui.memory_mut(|m| m.data.remove_temp::<String>(confirm_id));
                    if confirmed {
                        action = ResearchAction::StartResearch(TechId(pending_id));
                    }
                }
            }
        });

    action
}
