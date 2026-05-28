use bevy::prelude::Entity;
use bevy_egui::egui;
use macrocosmo_ui_dsl::{UiDslRenderer, lua::parse_ui_fragment_definitions};

use crate::scripting::ScriptEngine;
use crate::time_system::{GameClock, GameSpeed};
use macrocosmo_core::amount::{Amt, SignedAmt};

use super::DiplomacyPanelOpen;
use super::ResearchPanelOpen;
use super::UiElementRegistry;
use super::overlays::ShipDesignerState;

/// Observer-mode metadata for the top-bar badge + faction selector.
///
/// `observer_factions` is a sorted `(Entity, display_name)` list. When
/// `enabled` is true, the top bar renders an "Observer Mode" badge and a
/// ComboBox for switching the currently inspected faction.
pub struct ObserverBarState<'a> {
    pub enabled: bool,
    /// When `true`, commands are disabled (god-view). Renders a
    /// "[read-only]" tag next to the badge.
    pub read_only: bool,
    pub selected: &'a mut Option<Entity>,
    pub factions: &'a [(Entity, String)],
}

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
    diplomacy_open: &mut DiplomacyPanelOpen,
    designer_state: &mut ShipDesignerState,
    observer: Option<ObserverBarState<'_>>,
    ui_registry: Option<&mut UiElementRegistry>,
    engine: Option<&ScriptEngine>,
) {
    if let Some(engine) = engine
        && draw_top_bar_lua(
            ctx,
            engine,
            clock,
            speed,
            total_minerals,
            total_energy,
            total_food,
            total_authority,
            net_food,
            net_energy,
            net_minerals,
            net_authority,
            research_open,
            diplomacy_open,
            designer_state,
            observer.as_ref(),
        )
        .is_ok()
    {
        return;
    }

    draw_top_bar_legacy(
        ctx,
        clock,
        speed,
        total_minerals,
        total_energy,
        total_food,
        total_authority,
        net_food,
        net_energy,
        net_minerals,
        net_authority,
        research_open,
        diplomacy_open,
        designer_state,
        observer,
        ui_registry,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_top_bar_legacy(
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
    diplomacy_open: &mut DiplomacyPanelOpen,
    designer_state: &mut ShipDesignerState,
    observer: Option<ObserverBarState<'_>>,
    ui_registry: Option<&mut UiElementRegistry>,
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

            let pause_resp = ui.button("\u{23F8}").on_hover_text("Pause");
            if pause_resp.clicked() {
                speed.hexadies_per_second = 0.0;
            }
            let play_resp = ui.button("\u{25B6}").on_hover_text("Normal speed");
            if play_resp.clicked() {
                speed.hexadies_per_second = 1.0;
            }
            let ff_resp = ui.button("\u{23E9}").on_hover_text("Fast forward");
            if ff_resp.clicked() {
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
                ui.label(
                    egui::RichText::new(format!("({})", net.display_compact())).color(net_color),
                );
            }

            ui.separator();

            let r_label = if research_open.0 {
                "Research [open]"
            } else {
                "Research"
            };
            let research_resp = ui.button(r_label);
            if research_resp.clicked() {
                research_open.0 = !research_open.0;
            }

            let dip_label = if diplomacy_open.0 {
                "Diplomacy [open]"
            } else {
                "Diplomacy"
            };
            let diplomacy_resp = ui.button(dip_label);
            if diplomacy_resp.clicked() {
                diplomacy_open.0 = !diplomacy_open.0;
            }

            let d_label = if designer_state.open {
                "Ship Designer [open]"
            } else {
                "Ship Designer"
            };
            let designer_resp = ui.button(d_label);
            if designer_resp.clicked() {
                designer_state.open = !designer_state.open;
            }

            // Observer-mode badge + faction selector.
            if let Some(obs) = observer {
                if obs.enabled {
                    ui.separator();
                    let badge_text = if obs.read_only {
                        "Observer Mode [read-only]"
                    } else {
                        "Observer Mode"
                    };
                    ui.label(
                        egui::RichText::new(badge_text)
                            .strong()
                            .color(egui::Color32::from_rgb(230, 200, 90)),
                    );

                    let current_label = obs
                        .selected
                        .and_then(|sel| {
                            obs.factions
                                .iter()
                                .find(|(e, _)| *e == sel)
                                .map(|(_, n)| n.clone())
                        })
                        .unwrap_or_else(|| "(none)".to_string());

                    egui::ComboBox::from_id_salt("observer_faction_select")
                        .selected_text(current_label)
                        .show_ui(ui, |ui| {
                            for (entity, name) in obs.factions {
                                let is_selected = Some(*entity) == *obs.selected;
                                if ui.selectable_label(is_selected, name).clicked() {
                                    *obs.selected = Some(*entity);
                                }
                            }
                        });
                }
            }

            // #390-T5: Register key top-bar widgets for BRP introspection.
            #[cfg(feature = "remote")]
            if let Some(reg) = ui_registry {
                super::register_ui_element(reg, "top_bar.pause", "Pause", pause_resp.rect);
                super::register_ui_element(reg, "top_bar.play", "Play", play_resp.rect);
                super::register_ui_element(
                    reg,
                    "top_bar.fast_forward",
                    "Fast Forward",
                    ff_resp.rect,
                );
                super::register_ui_element(reg, "top_bar.research", r_label, research_resp.rect);
                super::register_ui_element(
                    reg,
                    "top_bar.diplomacy",
                    dip_label,
                    diplomacy_resp.rect,
                );
                super::register_ui_element(
                    reg,
                    "top_bar.ship_designer",
                    d_label,
                    designer_resp.rect,
                );
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_top_bar_lua(
    ctx: &egui::Context,
    engine: &ScriptEngine,
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
    diplomacy_open: &mut DiplomacyPanelOpen,
    designer_state: &mut ShipDesignerState,
    observer: Option<&ObserverBarState<'_>>,
) -> mlua::Result<()> {
    let lua = engine.lua();
    let registry = parse_ui_fragment_definitions(lua)?;
    let Some(fragment) = registry.get("core.ui.top_bar") else {
        return Err(mlua::Error::RuntimeError(
            "Lua UI fragment 'core.ui.top_bar' is not registered".into(),
        ));
    };

    let view = top_bar_view_table(
        lua,
        clock,
        speed,
        total_minerals,
        total_energy,
        total_food,
        total_authority,
        net_food,
        net_energy,
        net_minerals,
        net_authority,
        research_open,
        diplomacy_open,
        designer_state,
        observer,
    )?;
    let node = fragment.inflate(lua, view)?;
    let mut clicked_commands = Vec::new();

    egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
        let mut renderer = UiDslRenderer::default();
        clicked_commands = renderer.render(ui, &node).clicked_commands;
    });

    for command in clicked_commands {
        apply_top_bar_command(
            &command,
            speed,
            research_open,
            diplomacy_open,
            designer_state,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn top_bar_view_table(
    lua: &mlua::Lua,
    clock: &GameClock,
    speed: &GameSpeed,
    total_minerals: Amt,
    total_energy: Amt,
    total_food: Amt,
    total_authority: Amt,
    net_food: SignedAmt,
    net_energy: SignedAmt,
    net_minerals: SignedAmt,
    net_authority: SignedAmt,
    research_open: &ResearchPanelOpen,
    diplomacy_open: &DiplomacyPanelOpen,
    designer_state: &ShipDesignerState,
    observer: Option<&ObserverBarState<'_>>,
) -> mlua::Result<mlua::Table> {
    let view = lua.create_table()?;
    view.set(
        "date",
        format!(
            "Year {} Month {} Hexadies {}",
            clock.year(),
            clock.month(),
            clock.hexadies()
        ),
    )?;
    view.set(
        "speed",
        if speed.hexadies_per_second <= 0.0 {
            "PAUSED".to_string()
        } else {
            format!("x{:.0} hd/s", speed.hexadies_per_second)
        },
    )?;
    view.set("research_open", research_open.0)?;
    view.set("diplomacy_open", diplomacy_open.0)?;
    view.set("ship_designer_open", designer_state.open)?;

    let resources = lua.create_table()?;
    for (index, (label, stockpile, net)) in [
        ("F", total_food, net_food),
        ("E", total_energy, net_energy),
        ("M", total_minerals, net_minerals),
        ("A", total_authority, net_authority),
    ]
    .into_iter()
    .enumerate()
    {
        let resource = lua.create_table()?;
        resource.set("label", label)?;
        resource.set("stockpile", stockpile.display_compact())?;
        resource.set("net", net.display_compact())?;
        resources.set(index + 1, resource)?;
    }
    view.set("resources", resources)?;

    if let Some(observer) = observer {
        view.set("observer_enabled", observer.enabled)?;
        view.set("observer_read_only", observer.read_only)?;
    } else {
        view.set("observer_enabled", false)?;
        view.set("observer_read_only", false)?;
    }

    Ok(view)
}

fn apply_top_bar_command(
    command: &str,
    speed: &mut GameSpeed,
    research_open: &mut ResearchPanelOpen,
    diplomacy_open: &mut DiplomacyPanelOpen,
    designer_state: &mut ShipDesignerState,
) {
    match command {
        "time.pause" => speed.hexadies_per_second = 0.0,
        "time.play" => speed.hexadies_per_second = 1.0,
        "time.fast" => {
            speed.hexadies_per_second = (speed.hexadies_per_second * 2.0).max(1.0).min(16.0);
        }
        "ui.toggle.research" => research_open.0 = !research_open.0,
        "ui.toggle.diplomacy" => diplomacy_open.0 = !diplomacy_open.0,
        "ui.toggle.ship_designer" => designer_state.open = !designer_state.open,
        _ => {}
    }
}
