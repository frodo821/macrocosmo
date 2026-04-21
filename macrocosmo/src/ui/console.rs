use bevy::prelude::*;
use bevy_egui::egui;

use crate::scripting::log_buffer::{LogBuffer, LogSource};

/// Persistent state for the in-game Lua console overlay.
#[derive(Resource, Default)]
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
) -> Option<String> {
    if !state.visible {
        return None;
    }

    let mut submitted = None;
    let mut open = state.visible;

    egui::Window::new("Lua Console")
        .open(&mut open)
        .resizable(true)
        .default_size([600.0, 400.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            // --- Output area ---
            let available = ui.available_height() - 30.0;
            let scroll_area = egui::ScrollArea::vertical()
                .max_height(available.max(100.0))
                .auto_shrink([false, false])
                .stick_to_bottom(true);

            scroll_area.show(ui, |ui| {
                for entry in &log_buffer.entries {
                    let (color, prefix) = match &entry.source {
                        LogSource::Console => (egui::Color32::from_rgb(180, 180, 255), "> "),
                        LogSource::ConsoleResult => (egui::Color32::from_rgb(200, 255, 200), "= "),
                        LogSource::Error => (egui::Color32::from_rgb(255, 100, 100), "! "),
                        LogSource::Event(_) => (egui::Color32::from_rgb(255, 220, 80), ""),
                        LogSource::Lifecycle(_) => (egui::Color32::from_rgb(180, 220, 255), ""),
                        LogSource::Define => (egui::Color32::from_rgb(160, 160, 160), ""),
                        LogSource::Print => (egui::Color32::from_rgb(220, 220, 220), ""),
                    };

                    let source_tag = match &entry.source {
                        LogSource::Event(name) => format!("[evt:{}] ", name),
                        LogSource::Lifecycle(name) => format!("[lc:{}] ", name),
                        _ => String::new(),
                    };

                    let text = format!("{}{}{}", source_tag, prefix, entry.text);
                    ui.label(egui::RichText::new(&text).color(color).monospace());
                }
            });

            ui.separator();

            // --- Input area ---
            let response = ui
                .horizontal(|ui| {
                    ui.label(egui::RichText::new(">").monospace().strong());
                    let te = egui::TextEdit::singleline(&mut state.input)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width() - 8.0)
                        .lock_focus(true);
                    let r = ui.add(te);

                    // Request focus on the input field when console opens
                    if state.scroll_to_bottom {
                        r.request_focus();
                        state.scroll_to_bottom = false;
                    }

                    r
                })
                .inner;

            // Handle Enter key
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let input = state.input.trim().to_string();
                if !input.is_empty() {
                    state.history.push(input.clone());
                    state.history_index = None;
                    submitted = Some(input);
                }
                state.input.clear();
                // Re-focus the text field
                response.request_focus();
            }

            // Handle Up/Down arrow for history navigation
            if response.has_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    if !state.history.is_empty() {
                        let idx = match state.history_index {
                            Some(i) => i.saturating_sub(1),
                            None => state.history.len() - 1,
                        };
                        state.history_index = Some(idx);
                        state.input = state.history[idx].clone();
                    }
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

    state.visible = open;
    submitted
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
