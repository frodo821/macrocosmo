use bevy_egui::egui;
use macrocosmo_ui_dsl::{UiDslRenderer, lua::parse_ui_fragment_definitions};

use crate::events::EventLog;
use crate::scripting::ScriptEngine;

/// Draws the bottom bar showing game events with category-based colors.
pub fn draw_bottom_bar(ctx: &egui::Context, event_log: &EventLog, engine: Option<&ScriptEngine>) {
    if let Some(engine) = engine
        && draw_bottom_bar_lua(ctx, event_log, engine).is_ok()
    {
        return;
    }

    draw_bottom_bar_legacy(ctx, event_log);
}

fn draw_bottom_bar_legacy(ctx: &egui::Context, event_log: &EventLog) {
    egui::TopBottomPanel::bottom("bottom_bar")
        .max_height(120.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("Event Log").strong());
            ui.separator();

            let entries = &event_log.entries;
            let start = entries.len().saturating_sub(8);
            let recent = &entries[start..];

            if recent.is_empty() {
                ui.label("No events yet.");
            } else {
                egui::ScrollArea::vertical()
                    .max_height(80.0)
                    .show(ui, |ui| {
                        for entry in recent.iter().rev() {
                            let [r, g, b] = entry.kind.category().color();
                            let text = format!("[hd {}] {}", entry.timestamp, entry.description,);
                            ui.label(
                                egui::RichText::new(text).color(egui::Color32::from_rgb(r, g, b)),
                            );
                        }
                    });
            }
        });
}

fn draw_bottom_bar_lua(
    ctx: &egui::Context,
    event_log: &EventLog,
    engine: &ScriptEngine,
) -> mlua::Result<()> {
    let lua = engine.lua();
    let registry = parse_ui_fragment_definitions(lua)?;
    let Some(fragment) = registry.get("core.ui.bottom_bar") else {
        return Err(mlua::Error::RuntimeError(
            "Lua UI fragment 'core.ui.bottom_bar' is not registered".into(),
        ));
    };

    let view = bottom_bar_view_table(lua, event_log)?;
    let node = fragment.inflate(lua, view)?;
    egui::TopBottomPanel::bottom("bottom_bar")
        .max_height(120.0)
        .show(ctx, |ui| {
            let mut renderer = UiDslRenderer::default();
            let _ = renderer.render(ui, &node);
        });
    Ok(())
}

fn bottom_bar_view_table(lua: &mlua::Lua, event_log: &EventLog) -> mlua::Result<mlua::Table> {
    let view = lua.create_table()?;
    let entries = lua.create_table()?;
    let start = event_log.entries.len().saturating_sub(8);
    for (index, entry) in event_log.entries[start..].iter().rev().enumerate() {
        let row = lua.create_table()?;
        row.set("timestamp", entry.timestamp)?;
        row.set("description", entry.description.clone())?;
        row.set(
            "text",
            format!("[hd {}] {}", entry.timestamp, entry.description),
        )?;
        entries.set(index + 1, row)?;
    }
    view.set("entries", entries)?;
    Ok(view)
}
