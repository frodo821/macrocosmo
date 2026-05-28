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
use macrocosmo_ui_dsl::{UiDslRenderer, lua::parse_ui_fragment_definitions};

use crate::casus_belli::{ActiveWars, CasusBelliRegistry};
use crate::faction::{FactionRelations, RelationState};
use crate::scripting::ScriptEngine;
use crate::scripting::faction_api::{
    DiplomaticOptionDefinition, DiplomaticOptionRegistry, FactionRegistry,
};
use crate::time_system::GameClock;

// #347: the F2 toggle is now registered with `KeybindingRegistry` under
// `crate::input::actions::UI_TOGGLE_DIPLOMACY`. The previous
// `pub const TOGGLE_KEY: KeyCode = KeyCode::F2;` constant has been
// removed; consumers should look up the binding through the registry.

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
    _faction_registry: &FactionRegistry,
    factions: &[FactionEntry],
    clock: &GameClock,
    engine: Option<&ScriptEngine>,
) -> DiplomacyAction {
    if !*open {
        return DiplomacyAction::None;
    }

    if let Some(engine) = engine
        && let Ok(action) = draw_diplomacy_panel_lua(
            ctx,
            open,
            player_entity,
            relations,
            active_wars,
            cb_registry,
            option_registry,
            factions,
            clock,
            engine,
        )
    {
        return action;
    }

    draw_diplomacy_panel_legacy(
        ctx,
        open,
        player_entity,
        relations,
        active_wars,
        cb_registry,
        option_registry,
        factions,
        clock,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_diplomacy_panel_legacy(
    ctx: &egui::Context,
    open: &mut bool,
    player_entity: Entity,
    relations: &FactionRelations,
    active_wars: &ActiveWars,
    cb_registry: &CasusBelliRegistry,
    option_registry: &DiplomaticOptionRegistry,
    factions: &[FactionEntry],
    clock: &GameClock,
) -> DiplomacyAction {
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
                                let allowed_options = &entry.allowed_diplomatic_options;

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
                                                to: entry.entity,
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
                                            to: entry.entity,
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
                                                to: entry.entity,
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
                                            to: entry.entity,
                                            option_id: crate::faction::DIPLO_BREAK_ALLIANCE.into(),
                                        };
                                    }
                                });

                                // Lua-defined diplomatic options (from DiplomaticOptionRegistry).
                                // Exclude built-in options that are already rendered above
                                // with proper relation-state checks (#404).
                                let option_defs: Vec<&DiplomaticOptionDefinition> = option_registry
                                    .options
                                    .values()
                                    .filter(|o| {
                                        allowed_options.contains(&o.id)
                                            && !is_builtin_diplomatic_option(&o.id)
                                    })
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
                                                    to: entry.entity,
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

#[allow(clippy::too_many_arguments)]
fn draw_diplomacy_panel_lua(
    ctx: &egui::Context,
    open: &mut bool,
    player_entity: Entity,
    relations: &FactionRelations,
    active_wars: &ActiveWars,
    cb_registry: &CasusBelliRegistry,
    option_registry: &DiplomaticOptionRegistry,
    factions: &[FactionEntry],
    clock: &GameClock,
    engine: &ScriptEngine,
) -> mlua::Result<DiplomacyAction> {
    let lua = engine.lua();
    let registry = parse_ui_fragment_definitions(lua)?;
    let Some(fragment) = registry.get("core.ui.diplomacy") else {
        return Err(mlua::Error::RuntimeError(
            "Lua UI fragment 'core.ui.diplomacy' is not registered".into(),
        ));
    };

    let view = diplomacy_view_table(
        lua,
        player_entity,
        relations,
        active_wars,
        cb_registry,
        option_registry,
        factions,
        clock,
    )?;
    let node = fragment.inflate(lua, view)?;
    let mut clicked_commands = Vec::new();

    egui::Window::new("Diplomacy")
        .open(open)
        .resizable(true)
        .default_size([480.0, 520.0])
        .show(ctx, |ui| {
            let mut renderer = UiDslRenderer::default();
            clicked_commands = renderer.render(ui, &node).clicked_commands;
        });

    Ok(clicked_commands
        .into_iter()
        .find_map(|command| parse_diplomacy_command(&command, player_entity))
        .unwrap_or(DiplomacyAction::None))
}

#[allow(clippy::too_many_arguments)]
fn diplomacy_view_table(
    lua: &mlua::Lua,
    player_entity: Entity,
    relations: &FactionRelations,
    active_wars: &ActiveWars,
    cb_registry: &CasusBelliRegistry,
    option_registry: &DiplomaticOptionRegistry,
    factions: &[FactionEntry],
    clock: &GameClock,
) -> mlua::Result<mlua::Table> {
    let view = lua.create_table()?;

    let wars = lua.create_table()?;
    for (index, war) in active_wars
        .wars_involving(player_entity)
        .into_iter()
        .enumerate()
    {
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

        let row = lua.create_table()?;
        row.set("opponent", opponent_name)?;
        row.set("title", format!("War with {}", opponent_name))?;
        row.set("duration", format!("{} hd", clock.elapsed - war.started_at))?;
        row.set("casus_belli", cb_name)?;

        let demands = lua.create_table()?;
        let scenarios = lua.create_table()?;
        if let Some(cb_def) = cb_registry.get(&war.cb_id) {
            for (demand_index, demand) in cb_def.demands.iter().enumerate() {
                demands.set(demand_index + 1, demand.kind.to_string())?;
            }
            for (scenario_index, scenario) in cb_def.end_scenarios.iter().enumerate() {
                let scenario_row = lua.create_table()?;
                scenario_row.set("label", scenario.label.clone())?;
                scenario_row.set(
                    "command",
                    format!(
                        "diplomacy.end_war:{}:{}:{}",
                        war.attacker.to_bits(),
                        war.defender.to_bits(),
                        scenario.id
                    ),
                )?;
                scenarios.set(scenario_index + 1, scenario_row)?;
            }
        }
        row.set("demands", demands)?;
        row.set("end_scenarios", scenarios)?;
        wars.set(index + 1, row)?;
    }
    view.set("active_wars", wars)?;

    let faction_rows = lua.create_table()?;
    let mut output_index = 1;
    for entry in factions {
        if entry.entity == player_entity {
            continue;
        }

        let relation = relations.get_or_default(player_entity, entry.entity);
        let row = lua.create_table()?;
        row.set("name", entry.name.clone())?;
        row.set("state", relation_state_label(relation.state))?;
        row.set("standing_label", format!("{:.0}", relation.standing))?;
        row.set(
            "standing_progress",
            ((relation.standing + 100.0) / 200.0).clamp(0.0, 1.0),
        )?;

        let options = lua.create_table()?;
        if entry.can_diplomacy {
            let mut option_index = 1;
            for (label, id) in builtin_diplomatic_actions(relation.state) {
                let option = lua.create_table()?;
                option.set("label", label)?;
                option.set(
                    "command",
                    format!("diplomacy.send:{}:{}", entry.entity.to_bits(), id),
                )?;
                options.set(option_index, option)?;
                option_index += 1;
            }

            let mut option_defs: Vec<&DiplomaticOptionDefinition> = option_registry
                .options
                .values()
                .filter(|option| {
                    entry.allowed_diplomatic_options.contains(&option.id)
                        && !is_builtin_diplomatic_option(&option.id)
                })
                .collect();
            option_defs.sort_by(|a, b| a.name.cmp(&b.name));
            for option_def in option_defs {
                let option = lua.create_table()?;
                option.set("label", option_def.name.clone())?;
                option.set(
                    "command",
                    format!(
                        "diplomacy.send:{}:{}",
                        entry.entity.to_bits(),
                        option_def.id
                    ),
                )?;
                options.set(option_index, option)?;
                option_index += 1;
            }
        }
        row.set("options", options)?;
        faction_rows.set(output_index, row)?;
        output_index += 1;
    }
    view.set("factions", faction_rows)?;

    Ok(view)
}

fn parse_diplomacy_command(command: &str, player_entity: Entity) -> Option<DiplomacyAction> {
    if let Some(rest) = command.strip_prefix("diplomacy.send:") {
        let mut parts = rest.splitn(2, ':');
        let to_bits = parts.next()?.parse::<u64>().ok()?;
        let option_id = parts.next()?.to_string();
        let to = Entity::from_bits(to_bits);
        return Some(DiplomacyAction::SendDiplomaticEvent {
            from: player_entity,
            to,
            option_id,
        });
    }

    parse_end_war_command(command)
}

fn parse_end_war_command(command: &str) -> Option<DiplomacyAction> {
    let rest = command.strip_prefix("diplomacy.end_war:")?;
    let mut parts = rest.splitn(3, ':');
    let faction_a = Entity::from_bits(parts.next()?.parse::<u64>().ok()?);
    let faction_b = Entity::from_bits(parts.next()?.parse::<u64>().ok()?);
    let scenario_id = parts.next()?.to_string();
    Some(DiplomacyAction::EndWar {
        faction_a,
        faction_b,
        scenario_id,
    })
}

fn relation_state_label(state: RelationState) -> &'static str {
    match state {
        RelationState::Neutral => "Neutral",
        RelationState::Peace => "Peace",
        RelationState::War => "War",
        RelationState::Alliance => "Alliance",
    }
}

fn builtin_diplomatic_actions(state: RelationState) -> Vec<(&'static str, &'static str)> {
    match state {
        RelationState::War => vec![("Propose Peace", crate::faction::DIPLO_PROPOSE_PEACE)],
        RelationState::Peace => vec![
            ("Declare War", crate::faction::DIPLO_DECLARE_WAR),
            ("Propose Alliance", crate::faction::DIPLO_PROPOSE_ALLIANCE),
        ],
        RelationState::Alliance => vec![("Break Alliance", crate::faction::DIPLO_BREAK_ALLIANCE)],
        RelationState::Neutral => vec![("Declare War", crate::faction::DIPLO_DECLARE_WAR)],
    }
}

/// Returns `true` for diplomatic option ids that are handled as built-in
/// buttons with explicit relation-state prerequisites in the panel above.
/// These must not appear a second time in the Lua-defined options section.
fn is_builtin_diplomatic_option(id: &str) -> bool {
    matches!(
        id,
        crate::faction::DIPLO_DECLARE_WAR
            | crate::faction::DIPLO_BREAK_ALLIANCE
            | crate::faction::DIPLO_PROPOSE_PEACE
            | crate::faction::DIPLO_PROPOSE_ALLIANCE
            | crate::faction::DIPLO_ACCEPT_PEACE
            | crate::faction::DIPLO_ACCEPT_ALLIANCE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_options_are_excluded_from_lua_section() {
        // All built-in diplomatic option ids must be recognized so they are
        // filtered out of the Lua-defined options list (regression for #404).
        assert!(is_builtin_diplomatic_option("declare_war"));
        assert!(is_builtin_diplomatic_option("break_alliance"));
        assert!(is_builtin_diplomatic_option("propose_peace"));
        assert!(is_builtin_diplomatic_option("propose_alliance"));
        assert!(is_builtin_diplomatic_option("accept_peace"));
        assert!(is_builtin_diplomatic_option("accept_alliance"));
    }

    #[test]
    fn non_builtin_options_pass_through() {
        assert!(!is_builtin_diplomatic_option("generic_negotiation"));
        assert!(!is_builtin_diplomatic_option("trade_agreement"));
        assert!(!is_builtin_diplomatic_option("cultural_exchange"));
        assert!(!is_builtin_diplomatic_option(""));
    }
}
