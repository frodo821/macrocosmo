//! Replay tab: load a `Playthrough` JSON file and step through events on
//! an isolated bus.

use bevy_egui::egui;
use macrocosmo_ai::{
    AiBus, Command,
    playthrough::{Playthrough, PlaythroughEvent, replay as replay_fn},
};

use super::ReplayState;

/// Build a fresh bus and apply just the declarations of a playthrough.
pub(super) fn build_empty_bus(pt: &Playthrough) -> AiBus {
    let mut bus = AiBus::with_warning_mode(macrocosmo_ai::WarningMode::Silent);
    for (id, spec) in &pt.declarations.metrics {
        bus.declare_metric(id.clone(), spec.clone());
    }
    for (kind, spec) in &pt.declarations.commands {
        bus.declare_command(kind.clone(), spec.clone());
    }
    for (kind, spec) in &pt.declarations.evidence {
        bus.declare_evidence(kind.clone(), spec.clone());
    }
    bus
}

/// Apply a single playthrough event to a bus (same shape as `replay_fn` but
/// one event at a time so the UI can step through them).
pub(super) fn apply_event(bus: &mut AiBus, event: &PlaythroughEvent) {
    match event {
        PlaythroughEvent::Metric { id, value, at } => bus.emit(id, *value, *at),
        PlaythroughEvent::Command(sc) => bus.emit_command(Command::from(sc.clone())),
        PlaythroughEvent::Evidence(ev) => bus.emit_evidence(ev.clone()),
    }
}

/// Reset the bus to declarations only, and replay the first `n` events.
pub(super) fn rebuild_bus_to(pt: &Playthrough, n: usize) -> AiBus {
    let mut bus = build_empty_bus(pt);
    for event in pt.events.iter().take(n) {
        apply_event(&mut bus, event);
    }
    bus
}

pub fn draw_replay(ui: &mut egui::Ui, state: &mut ReplayState) {
    ui.horizontal(|ui| {
        ui.label("Path:");
        ui.text_edit_singleline(&mut state.path_input);
        if ui.button("Load").clicked() {
            match load_playthrough(&state.path_input) {
                Ok(pt) => {
                    state.bus = Some(build_empty_bus(&pt));
                    state.cursor = 0;
                    state.last_error = None;
                    state.loaded = Some(pt);
                }
                Err(err) => {
                    state.last_error = Some(err);
                    state.loaded = None;
                    state.bus = None;
                    state.cursor = 0;
                }
            }
        }
        if state.loaded.is_some() && ui.button("Unload").clicked() {
            state.loaded = None;
            state.bus = None;
            state.cursor = 0;
        }
    });

    if let Some(err) = &state.last_error {
        ui.label(
            egui::RichText::new(format!("Error: {}", err))
                .color(egui::Color32::from_rgb(220, 120, 120)),
        );
    }

    let Some(pt) = state.loaded.as_ref() else {
        ui.separator();
        ui.label(
            egui::RichText::new(
                "Load a `Playthrough` JSON file (produced by `macrocosmo-ai` \
                 playthrough recording) to step through its events here.",
            )
            .weak()
            .italics(),
        );
        return;
    };

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} (seed={}, duration={} ticks)",
                pt.meta.name, pt.meta.seed, pt.meta.duration_ticks,
            ))
            .strong(),
        );
    });

    let total = pt.events.len();
    ui.horizontal(|ui| {
        let rewind = ui.button("|<").on_hover_text("Rewind to start").clicked();
        let step_back = ui.button("<").on_hover_text("Step back one event").clicked();
        let step_forward = ui
            .button(">")
            .on_hover_text("Step forward one event")
            .clicked();
        let jump_end = ui.button(">|").on_hover_text("Jump to end").clicked();
        ui.label(format!("event {} / {}", state.cursor, total));

        if rewind {
            state.cursor = 0;
            state.bus = Some(build_empty_bus(pt));
        }
        if step_back && state.cursor > 0 {
            state.cursor -= 1;
            state.bus = Some(rebuild_bus_to(pt, state.cursor));
        }
        if step_forward && state.cursor < total {
            if let Some(bus) = state.bus.as_mut() {
                apply_event(bus, &pt.events[state.cursor]);
            }
            state.cursor += 1;
        }
        if jump_end {
            state.cursor = total;
            state.bus = Some(rebuild_bus_to(pt, total));
        }
    });

    ui.separator();

    // Last applied event details.
    if state.cursor == 0 {
        ui.label(
            egui::RichText::new("Cursor at start — no events applied yet.")
                .weak()
                .italics(),
        );
    } else if let Some(event) = pt.events.get(state.cursor - 1) {
        ui.label(egui::RichText::new("Last applied event:").strong());
        ui.label(format_event(event));
    }

    ui.separator();
    ui.label(egui::RichText::new("Reconstructed bus state").strong());
    if let Some(bus) = state.bus.as_ref() {
        // Lightweight summary (a full inspector here would duplicate
        // Inspector tab wiring — keep it compact).
        let snap = bus.snapshot();
        ui.label(format!(
            "metrics={} commands_declared={} evidence_kinds={} pending_commands={}",
            snap.metrics.len(),
            snap.commands.len(),
            snap.evidence.len(),
            snap.pending_commands.len(),
        ));

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Current metric values").italics());
                egui::Grid::new("ai_debug_replay_metric_grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        let mut keys: Vec<_> = snap.metrics.keys().collect();
                        keys.sort();
                        for id in keys {
                            ui.label(id.as_str());
                            let v = bus
                                .current(id)
                                .map(|v| format!("{:.4}", v))
                                .unwrap_or_else(|| "—".into());
                            ui.label(v);
                            ui.end_row();
                        }
                    });
            });
    }
}

fn format_event(event: &PlaythroughEvent) -> String {
    match event {
        PlaythroughEvent::Metric { id, value, at } => {
            format!("[tick={}] METRIC {}={:.4}", at, id.as_str(), value)
        }
        PlaythroughEvent::Command(cmd) => format!(
            "[tick={}] COMMAND {} by Faction({}) priority={:.2}",
            cmd.at,
            cmd.kind.as_str(),
            cmd.issuer.0,
            cmd.priority
        ),
        PlaythroughEvent::Evidence(ev) => format!(
            "[tick={}] EVIDENCE {} F({})->F({}) mag={:+.3}",
            ev.at,
            ev.kind.as_str(),
            ev.observer.0,
            ev.target.0,
            ev.magnitude
        ),
    }
}

fn load_playthrough(path: &str) -> Result<Playthrough, String> {
    if path.trim().is_empty() {
        return Err("empty path".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let pt: Playthrough =
        serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))?;
    // Catch version mismatches up front so the UI doesn't silently accept a
    // playthrough it can't step through.
    let _ = replay_fn(&pt).map_err(|e| format!("{e}"))?;
    Ok(pt)
}
