//! #304 (S-9): Diplomacy panel UI.
//!
//! Provides a floating egui window (toggled via F2 or the top-bar button)
//! that displays:
//!
//! - List of known factions with relation state, standing bar, and standing
//!   value.
//! - Active wars with casus belli name and duration.
//! - Per-faction diplomatic option buttons (from `DiplomaticOptionRegistry`,
//!   filtered by `allowed_diplomatic_options`).
//! - End-of-war scenario picker when at war with a faction.
//! - Casus belli viewer showing CB details and demands.
//!
//! All drawing functions are plain `fn` taking `&egui::Context`, following the
//! project convention that sub-modules export functions rather than Bevy systems.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::casus_belli::{ActiveWars, CasusBelliRegistry};
use crate::faction::{FactionRelations, RelationState};
use crate::scripting::faction_api::{
    DiplomaticActionDefinition, DiplomaticActionRegistry, DiplomaticOptionDefinition,
    DiplomaticOptionRegistry, FactionRegistry,
};
use crate::time_system::GameClock;

/// F2 toggle keycode for the diplomacy panel.
pub const TOGGLE_KEY: KeyCode = KeyCode::F2;

/// Action returned by the diplomacy panel to the caller system.
pub enum DiplomacyAction {
    None,
    /// Send a built-in diplomatic action (declare war, propose peace, etc.)
    SendBuiltinAction {
        from: Entity,
        to: Entity,
        action: crate::faction::DiplomaticAction,
    },
    /// Send a custom Lua-defined diplomatic action.
    SendCustomAction {
        from: Entity,
        to: Entity,
        action_id: String,
    },
    /// End a war via a specific end scenario.
    EndWar {
        faction_a: Entity,
        faction_b: Entity,
        scenario_id: String,
    },
}

/// Per-faction data passed to the diplomacy panel for display.
pub struct FactionEntry {
    pub entity: Entity,
    pub name: String,
    pub can_diplomacy: bool,
    pub allowed_diplomatic_options: Vec<String>,
}

/// Draws the diplomacy panel.
///
/// Returns a [`DiplomacyAction`] for the caller system to execute (the panel
/// only has read access to game state).
#[allow(clippy::too_many_arguments)]
pub fn draw_diplomacy_panel(
    ctx: &egui::Context,
    open: &mut bool,
    player_entity: Entity,
    relations: &FactionRelations,
    active_wars: &ActiveWars,
    cb_registry: &CasusBelliRegistry,
    option_registry: &DiplomaticOptionRegistry,
    action_registry: &DiplomaticActionRegistry,
    _faction_registry: &FactionRegistry,
    factions: &[FactionEntry],
    clock: &GameClock,
) -> DiplomacyAction {
    if !*open {
        return DiplomacyAction::None;
    }

    let mut action = DiplomacyAction::None;

    egui::Window::new("Diplomacy")
        .open(open)
        .resizable(true)
        .default_size([480.0, 520.0])
        .show(ctx, |ui| {
            // --- Active Wars section ---
            let player_wars = active_wars.wars_involving(player_entity);
            if !player_wars.is_empty() {
                ui.label(egui::RichText::new("Active Wars").strong());
                for war in &player_wars {
                    let opponent = if war.attacker == player_entity {
                        war.defender
                    } else {
                        war.attacker
                    };
                    let opponent_name = factions
                        .iter()
                        .find(|f| f.entity == opponent)
                        .map(|f| f.name.as_str())
                        .unwrap_or("Unknown");
                    let cb_name = cb_registry
                        .get(&war.cb_id)
                        .map(|cb| cb.name.as_str())
                        .unwrap_or(&war.cb_id);
                    let duration = clock.elapsed - war.started_at;

                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("War with {}", opponent_name))
                                    .strong()
                                    .color(egui::Color32::from_rgb(255, 100, 100)),
                            );
                            ui.label(format!("({} hd)", duration));
                        });
                        ui.label(format!("Casus Belli: {}", cb_name));

                        // CB details (demands)
                        if let Some(cb_def) = cb_registry.get(&war.cb_id) {
                            if !cb_def.demands.is_empty() {
                                ui.label(
                                    egui::RichText::new("Demands:")
                                        .small()
                                        .color(egui::Color32::LIGHT_GRAY),
                                );
                                for demand in &cb_def.demands {
                                    ui.label(
                                        egui::RichText::new(format!("  - {}", demand.kind))
                                            .small()
                                            .color(egui::Color32::LIGHT_GRAY),
                                    );
                                }
                            }
                            if !cb_def.additional_demand_groups.is_empty() {
                                ui.label(
                                    egui::RichText::new("Additional demands available:")
                                        .small()
                                        .color(egui::Color32::from_rgb(200, 200, 150)),
                                );
                                for group in &cb_def.additional_demand_groups {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "  {} (max {} picks)",
                                            group.label, group.max_picks
                                        ))
                                        .small()
                                        .color(egui::Color32::from_rgb(200, 200, 150)),
                                    );
                                }
                            }

                            // End-of-war scenario picker
                            if !cb_def.end_scenarios.is_empty() {
                                ui.separator();
                                ui.label(egui::RichText::new("End War Options:").strong());
                                for scenario in &cb_def.end_scenarios {
                                    if ui.button(&scenario.label).clicked() {
                                        action = DiplomacyAction::EndWar {
                                            faction_a: player_entity,
                                            faction_b: opponent,
                                            scenario_id: scenario.id.clone(),
                                        };
                                    }
                                }
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
                ui.separator();
            }

            // --- Faction list ---
            ui.label(egui::RichText::new("Known Factions").strong());
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for entry in factions {
                        if entry.entity == player_entity {
                            continue;
                        }

                        let view = relations.get_or_default(player_entity, entry.entity);

                        let can_diplomacy = entry.can_diplomacy;

                        ui.group(|ui| {
                            // Header: faction name + relation state
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&entry.name).strong());
                                let (state_text, state_color) = match view.state {
                                    RelationState::Neutral => (
                                        "Neutral",
                                        egui::Color32::from_rgb(180, 180, 180),
                                    ),
                                    RelationState::Peace => (
                                        "Peace",
                                        egui::Color32::from_rgb(100, 200, 100),
                                    ),
                                    RelationState::War => (
                                        "War",
                                        egui::Color32::from_rgb(255, 100, 100),
                                    ),
                                    RelationState::Alliance => (
                                        "Alliance",
                                        egui::Color32::from_rgb(100, 150, 255),
                                    ),
                                };
                                ui.label(
                                    egui::RichText::new(format!("[{}]", state_text))
                                        .color(state_color),
                                );
                            });

                            // Standing bar
                            ui.horizontal(|ui| {
                                ui.label("Standing:");
                                let normalized =
                                    ((view.standing + 100.0) / 200.0).clamp(0.0, 1.0) as f32;
                                let bar_color = if view.standing >= 0.0 {
                                    egui::Color32::from_rgb(100, 200, 100)
                                } else {
                                    egui::Color32::from_rgb(255, 100, 100)
                                };
                                let bar = egui::ProgressBar::new(normalized)
                                    .text(format!("{:.0}", view.standing))
                                    .fill(bar_color);
                                ui.add_sized([150.0, 16.0], bar);
                            });

                            // Diplomatic options
                            if can_diplomacy {
                                let allowed_options = &entry.allowed_diplomatic_options;

                                // Built-in diplomatic actions
                                ui.horizontal(|ui| {
                                    let is_at_war = view.state == RelationState::War;
                                    let is_allied = view.state == RelationState::Alliance;

                                    // Declare War (available when not at war)
                                    if !is_at_war {
                                        if ui
                                            .button(
                                                egui::RichText::new("Declare War")
                                                    .color(egui::Color32::from_rgb(255, 100, 100)),
                                            )
                                            .clicked()
                                        {
                                            action = DiplomacyAction::SendBuiltinAction {
                                                from: player_entity,
                                                to: entry.entity,
                                                action: crate::faction::DiplomaticAction::DeclareWar,
                                            };
                                        }
                                    }

                                    // Propose Peace (available during war)
                                    if is_at_war
                                        && ui
                                            .button(
                                                egui::RichText::new("Propose Peace").color(
                                                    egui::Color32::from_rgb(100, 200, 100),
                                                ),
                                            )
                                            .clicked()
                                    {
                                        action = DiplomacyAction::SendBuiltinAction {
                                            from: player_entity,
                                            to: entry.entity,
                                            action: crate::faction::DiplomaticAction::ProposePeace,
                                        };
                                    }

                                    // Propose Alliance (available during peace, not war)
                                    if !is_at_war
                                        && !is_allied
                                        && view.state == RelationState::Peace
                                    {
                                        if ui
                                            .button(
                                                egui::RichText::new("Propose Alliance").color(
                                                    egui::Color32::from_rgb(100, 150, 255),
                                                ),
                                            )
                                            .clicked()
                                        {
                                            action = DiplomacyAction::SendBuiltinAction {
                                                from: player_entity,
                                                to: entry.entity,
                                                action:
                                                    crate::faction::DiplomaticAction::ProposeAlliance,
                                            };
                                        }
                                    }

                                    // Break Alliance (available when allied)
                                    if is_allied
                                        && ui
                                            .button(
                                                egui::RichText::new("Break Alliance").color(
                                                    egui::Color32::from_rgb(230, 200, 90),
                                                ),
                                            )
                                            .clicked()
                                    {
                                        action = DiplomacyAction::SendBuiltinAction {
                                            from: player_entity,
                                            to: entry.entity,
                                            action:
                                                crate::faction::DiplomaticAction::BreakAlliance,
                                        };
                                    }
                                });

                                // Lua-defined custom diplomatic actions
                                if !allowed_options.is_empty() {
                                    let custom_actions: Vec<&DiplomaticActionDefinition> =
                                        action_registry
                                            .actions
                                            .values()
                                            .filter(|a| allowed_options.contains(&a.id))
                                            .collect();

                                    if !custom_actions.is_empty() {
                                        ui.horizontal(|ui| {
                                            for act_def in &custom_actions {
                                                let enabled = is_custom_action_available(
                                                    act_def,
                                                    player_entity,
                                                    entry.entity,
                                                    relations,
                                                );
                                                let mut btn = ui.add_enabled(
                                                    enabled,
                                                    egui::Button::new(&act_def.name),
                                                );
                                                if !act_def.description.is_empty() {
                                                    btn = btn.on_hover_text(&act_def.description);
                                                }
                                                if btn.clicked() {
                                                    action = DiplomacyAction::SendCustomAction {
                                                        from: player_entity,
                                                        to: entry.entity,
                                                        action_id: act_def.id.clone(),
                                                    };
                                                }
                                            }
                                        });
                                    }
                                }

                                // Lua-defined diplomatic options (from DiplomaticOptionRegistry)
                                let option_defs: Vec<&DiplomaticOptionDefinition> = option_registry
                                    .options
                                    .values()
                                    .filter(|o| allowed_options.contains(&o.id))
                                    .collect();

                                if !option_defs.is_empty() {
                                    ui.horizontal(|ui| {
                                        for opt_def in &option_defs {
                                            let mut btn = ui.button(&opt_def.name);
                                            if !opt_def.description.is_empty() {
                                                btn = btn.on_hover_text(&opt_def.description);
                                            }
                                            if btn.clicked() {
                                                // For now, diplomatic options fire
                                                // a custom action with the option id.
                                                // Full negotiation modal is future scope.
                                                action = DiplomacyAction::SendCustomAction {
                                                    from: player_entity,
                                                    to: entry.entity,
                                                    action_id: opt_def.id.clone(),
                                                };
                                            }
                                        }
                                    });
                                }
                            } else {
                                ui.label(
                                    egui::RichText::new("(No diplomatic options)")
                                        .weak()
                                        .italics(),
                                );
                            }
                        });

                        ui.add_space(2.0);
                    }
                });
        });

    action
}

/// Simplified availability check for a custom diplomatic action, evaluating
/// the `requires_state` and `min_standing` prerequisites against the current
/// relation view. Does NOT check `requires_diplomacy` (the caller has already
/// filtered to diplomacy-capable factions).
fn is_custom_action_available(
    def: &DiplomaticActionDefinition,
    from: Entity,
    to: Entity,
    relations: &FactionRelations,
) -> bool {
    let view = relations.get_or_default(from, to);

    if let Some(state) = def.requires_state {
        if view.state != state {
            return false;
        }
    }

    if let Some(min) = def.min_standing {
        if view.standing < min {
            return false;
        }
    }

    true
}
