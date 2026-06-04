use bevy::prelude::*;
use bevy_egui::egui;
use macrocosmo_ui_dsl::{UiDslRenderer, lua::parse_ui_fragment_definitions};

use crate::scripting::ScriptEngine;
use crate::scripting::log_buffer::{LogBuffer, LogSource};

/// Persistent state for the in-game Lua console overlay.
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct ConsoleState {
    /// Whether the console window is visible.
    pub visible: bool,
    /// Current text in the input field.
    pub input: String,
    /// Command history (most recent last).
    pub history: Vec<String>,
    /// Index into `history` for up/down navigation. `None` means "not browsing".
    pub history_index: Option<usize>,
    /// Whether to scroll to the bottom on the next frame.
    pub scroll_to_bottom: bool,
}

/// Draw the Lua console overlay. Called from the egui system chain.
///
/// Returns `Some(input)` when the user submits a line for evaluation.
pub fn draw_console(
    ctx: &egui::Context,
    state: &mut ConsoleState,
    log_buffer: &LogBuffer,
    engine: &ScriptEngine,
) -> Option<String> {
    if !state.visible {
        return None;
    }

    let mut submitted = None;
    let mut open = state.visible;

    // Screen-size-aware clamp: previous implementation computed the
    // scroll area's `max_height` from `ui.available_height()` which
    // created a feedback loop with the window's auto-size ScrollArea →
    // content. The window grew by ~30px every frame until it walked off
    // the screen. Closing the window while it was clipped off-screen
    // also stranded the remembered size, so the next toggle re-opened
    // it outside the viewport (→ "close once, can never reopen"
    // symptom). Fix: bound the window to 80% of screen height and use
    // `TopBottomPanel` for input so the log area simply fills the
    // remainder — no more self-amplifying height.
    let screen_h = ctx.screen_rect().height();
    let screen_w = ctx.screen_rect().width();
    let max_h = (screen_h * 0.8).max(200.0);
    let max_w = (screen_w * 0.8).max(400.0);

    egui::Window::new("Lua Console")
        .open(&mut open)
        .resizable(true)
        .default_size([600.0_f32.min(max_w), 400.0_f32.min(max_h)])
        .max_width(max_w)
        .max_height(max_h)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            // Input docked to the bottom first so the scroll area only
            // consumes the remaining (fixed) height. `resizable(false)`
            // keeps the panel size deterministic.
            egui::TopBottomPanel::bottom("lua_console_input")
                .resizable(false)
                .show_inside(ui, |ui| {
                    let response = ui
                        .horizontal(|ui| {
                            ui.label(egui::RichText::new(">").monospace().strong());
                            let te = egui::TextEdit::singleline(&mut state.input)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(ui.available_width() - 8.0)
                                .lock_focus(true);
                            let r = ui.add(te);
                            if state.scroll_to_bottom {
                                r.request_focus();
                                state.scroll_to_bottom = false;
                            }
                            r
                        })
                        .inner;

                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let input = state.input.trim().to_string();
                        if !input.is_empty() {
                            state.history.push(input.clone());
                            state.history_index = None;
                            submitted = Some(input);
                        }
                        state.input.clear();
                        response.request_focus();
                    }

                    if response.has_focus() {
                        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp))
                            && !state.history.is_empty()
                        {
                            let idx = match state.history_index {
                                Some(i) => i.saturating_sub(1),
                                None => state.history.len() - 1,
                            };
                            state.history_index = Some(idx);
                            state.input = state.history[idx].clone();
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                            if let Some(idx) = state.history_index {
                                if idx + 1 < state.history.len() {
                                    let new_idx = idx + 1;
                                    state.history_index = Some(new_idx);
                                    state.input = state.history[new_idx].clone();
                                } else {
                                    state.history_index = None;
                                    state.input.clear();
                                }
                            }
                        }
                    }
                });

            egui::CentralPanel::default().show_inside(ui, |ui| {
                if draw_console_log_lua(ui, log_buffer, engine).is_err() {
                    draw_console_log_legacy(ui, log_buffer);
                }
            });
        });

    state.visible = open;
    submitted
}

fn draw_console_log_lua(
    ui: &mut egui::Ui,
    log_buffer: &LogBuffer,
    engine: &ScriptEngine,
) -> mlua::Result<()> {
    let lua = engine.lua();
    let registry = parse_ui_fragment_definitions(lua)?;
    let Some(fragment) = registry.get("core.ui.lua_console") else {
        return Err(mlua::Error::RuntimeError(
            "Lua UI fragment 'core.ui.lua_console' is not registered".into(),
        ));
    };

    let view = console_view_table(lua, log_buffer)?;
    let node = fragment.inflate(lua, view)?;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            let mut renderer = UiDslRenderer::default();
            let _ = renderer.render(ui, &node);
        });
    Ok(())
}

fn console_view_table(lua: &mlua::Lua, log_buffer: &LogBuffer) -> mlua::Result<mlua::Table> {
    let view = lua.create_table()?;
    let entries = lua.create_table()?;
    for (index, entry) in log_buffer.entries.iter().enumerate() {
        let row = lua.create_table()?;
        row.set("source", log_source_name(&entry.source))?;
        row.set("timestamp", entry.timestamp)?;
        row.set("text", format_console_line(entry))?;
        entries.set(index + 1, row)?;
    }
    view.set("entries", entries)?;
    Ok(view)
}

fn draw_console_log_legacy(ui: &mut egui::Ui, log_buffer: &LogBuffer) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for entry in &log_buffer.entries {
                let color = match &entry.source {
                    LogSource::Console => egui::Color32::from_rgb(180, 180, 255),
                    LogSource::ConsoleResult => egui::Color32::from_rgb(200, 255, 200),
                    LogSource::Error => egui::Color32::from_rgb(255, 100, 100),
                    LogSource::Event(_) => egui::Color32::from_rgb(255, 220, 80),
                    LogSource::Lifecycle(_) => egui::Color32::from_rgb(180, 220, 255),
                    LogSource::Define => egui::Color32::from_rgb(160, 160, 160),
                    LogSource::Print => egui::Color32::from_rgb(220, 220, 220),
                };
                ui.label(
                    egui::RichText::new(format_console_line(entry))
                        .color(color)
                        .monospace(),
                );
            }
        });
}

fn format_console_line(entry: &crate::scripting::log_buffer::LogEntry) -> String {
    let prefix = match &entry.source {
        LogSource::Console => "> ",
        LogSource::ConsoleResult => "= ",
        LogSource::Error => "! ",
        LogSource::Event(_) | LogSource::Lifecycle(_) | LogSource::Define | LogSource::Print => "",
    };
    let source_tag = match &entry.source {
        LogSource::Event(name) => format!("[evt:{}] ", name),
        LogSource::Lifecycle(name) => format!("[lc:{}] ", name),
        _ => String::new(),
    };
    format!("{}{}{}", source_tag, prefix, entry.text)
}

fn log_source_name(source: &LogSource) -> &'static str {
    match source {
        LogSource::Console => "console",
        LogSource::ConsoleResult => "result",
        LogSource::Error => "error",
        LogSource::Event(_) => "event",
        LogSource::Lifecycle(_) => "lifecycle",
        LogSource::Define => "define",
        LogSource::Print => "print",
    }
}

/// Format a Lua value for display in the console output.
pub fn format_lua_value(value: &mlua::Value) -> String {
    match value {
        mlua::Value::Nil => "nil".to_string(),
        mlua::Value::Boolean(b) => b.to_string(),
        mlua::Value::Integer(i) => i.to_string(),
        mlua::Value::Number(n) => {
            // Show integers without decimal point
            if *n == (*n as i64) as f64 {
                format!("{}", *n as i64)
            } else {
                format!("{}", n)
            }
        }
        mlua::Value::String(s) => {
            format!("\"{}\"", s.to_string_lossy())
        }
        mlua::Value::Table(t) => {
            // Abbreviated table display
            let len = t.len().unwrap_or(0);
            if len > 0 {
                format!("table [{} items]", len)
            } else {
                "table {}".to_string()
            }
        }
        mlua::Value::Function(_) => "function".to_string(),
        mlua::Value::LightUserData(_) => "lightuserdata".to_string(),
        mlua::Value::UserData(_) => "userdata".to_string(),
        mlua::Value::Thread(_) => "thread".to_string(),
        mlua::Value::Error(e) => format!("error: {}", e),
        _ => format!("{:?}", value),
    }
}
