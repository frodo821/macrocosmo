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
    DiplomaticOptionDefinition, DiplomaticOptionRegistry, FactionRegistry, FactionTypeRegistry,
};
use crate::time_system::GameClock;

/// F2 toggle keycode for the diplomacy panel.
pub const TOGGLE_KEY: KeyCode = KeyCode::F2;

/// Action returned by the diplomacy panel to the caller system.
pub enum DiplomacyAction {
    None,
    /// Send a diplomatic event with the given option id.
    SendDiplomaticEvent {
        from: Entity,
        to: Entity,
        option_id: String,
    },
    /// End a war via a specific end scenario.
    EndWar {
        faction_a: Entity,
        faction_b: Entity,
        scenario_id: String,
    },
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
    _faction_registry: &FactionRegistry,
    type_registry: &FactionTypeRegistry,
    factions: &[(Entity, String, Option<String>)], // (entity, name, faction_type_id)
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
                        .find(|(e, _, _)| *e == opponent)
                        .map(|(_, n, _)| n.as_str())
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
                    for (faction_entity, faction_name, faction_type_id) in factions {
                        if *faction_entity == player_entity {
                            continue;
                        }

                        let view = relations.get_or_default(player_entity, *faction_entity);

                        // Check if this faction type supports diplomacy
                        let can_diplomacy = faction_type_id
                            .as_ref()
                            .and_then(|ft| type_registry.get(ft))
                            .map(|ft| ft.can_diplomacy)
                            .unwrap_or(false);

                        ui.group(|ui| {
                            // Header: faction name + relation state
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(faction_name).strong());
                                let (state_text, state_color) = match view.state {
                                    RelationState::Neutral => {
                                        ("Neutral", egui::Color32::from_rgb(180, 180, 180))
                                    }
                                    RelationState::Peace => {
                                        ("Peace", egui::Color32::from_rgb(100, 200, 100))
                                    }
                                    RelationState::War => {
                                        ("War", egui::Color32::from_rgb(255, 100, 100))
                                    }
                                    RelationState::Alliance => {
                                        ("Alliance", egui::Color32::from_rgb(100, 150, 255))
                                    }
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
                                // Collect allowed option ids from the faction type
                                let allowed_options: Vec<String> = faction_type_id
                                    .as_ref()
                                    .and_then(|ft| type_registry.get(ft))
                                    .map(|ft| ft.allowed_diplomatic_options.clone())
                                    .unwrap_or_default();

                                // Built-in diplomatic actions (using DiplomaticEvent)
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
                                            action = DiplomacyAction::SendDiplomaticEvent {
                                                from: player_entity,
                                                to: *faction_entity,
                                                option_id: crate::faction::DIPLO_DECLARE_WAR.into(),
                                            };
                                        }
                                    }

                                    // Propose Peace (available during war)
                                    if is_at_war
                                        && ui
                                            .button(
                                                egui::RichText::new("Propose Peace")
                                                    .color(egui::Color32::from_rgb(100, 200, 100)),
                                            )
                                            .clicked()
                                    {
                                        action = DiplomacyAction::SendDiplomaticEvent {
                                            from: player_entity,
                                            to: *faction_entity,
                                            option_id: crate::faction::DIPLO_PROPOSE_PEACE.into(),
                                        };
                                    }

                                    // Propose Alliance (available during peace, not war)
                                    if !is_at_war
                                        && !is_allied
                                        && view.state == RelationState::Peace
                                    {
                                        if ui
                                            .button(
                                                egui::RichText::new("Propose Alliance")
                                                    .color(egui::Color32::from_rgb(100, 150, 255)),
                                            )
                                            .clicked()
                                        {
                                            action = DiplomacyAction::SendDiplomaticEvent {
                                                from: player_entity,
                                                to: *faction_entity,
                                                option_id: crate::faction::DIPLO_PROPOSE_ALLIANCE
                                                    .into(),
                                            };
                                        }
                                    }

                                    // Break Alliance (available when allied)
                                    if is_allied
                                        && ui
                                            .button(
                                                egui::RichText::new("Break Alliance")
                                                    .color(egui::Color32::from_rgb(230, 200, 90)),
                                            )
                                            .clicked()
                                    {
                                        action = DiplomacyAction::SendDiplomaticEvent {
                                            from: player_entity,
                                            to: *faction_entity,
                                            option_id: crate::faction::DIPLO_BREAK_ALLIANCE.into(),
                                        };
                                    }
                                });

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
                                                action = DiplomacyAction::SendDiplomaticEvent {
                                                    from: player_entity,
                                                    to: *faction_entity,
                                                    option_id: opt_def.id.clone(),
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
