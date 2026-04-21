//! Inspector tab: browse declared topics and view current values / history.

use std::sync::Arc;

use bevy_egui::egui;
use macrocosmo_ai::{AiBus, CommandKindId, EvidenceKindId, MetricId};

use super::{InspectorCategory, InspectorState};

pub fn draw_inspector(ui: &mut egui::Ui, state: &mut InspectorState, bus: &AiBus, now: i64) {
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.text_edit_singleline(&mut state.filter);
        egui::ComboBox::from_id_salt("ai_debug_inspector_category")
            .selected_text(category_label(state.category))
            .show_ui(ui, |ui| {
                for cat in [
                    InspectorCategory::Metrics,
                    InspectorCategory::Commands,
                    InspectorCategory::Evidence,
                    InspectorCategory::Pending,
                ] {
                    if ui
                        .selectable_label(state.category == cat, category_label(cat))
                        .clicked()
                    {
                        state.category = cat;
                        state.selected = None;
                    }
                }
            });
    });
    ui.separator();

    let snapshot = bus.snapshot();
    let filter = state.filter.to_lowercase();

    egui::SidePanel::left("ai_debug_inspector_list")
        .resizable(true)
        .default_width(240.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match state.category {
                    InspectorCategory::Metrics => {
                        let mut ids: Vec<&MetricId> = snapshot.metrics.keys().collect();
                        ids.sort();
                        for id in ids {
                            if !filter.is_empty() && !id.as_str().to_lowercase().contains(&filter) {
                                continue;
                            }
                            let selected = state
                                .selected
                                .as_deref()
                                .map(|s| s == id.as_str())
                                .unwrap_or(false);
                            if ui.selectable_label(selected, id.as_str()).clicked() {
                                state.selected = Some(Arc::from(id.as_str()));
                            }
                        }
                    }
                    InspectorCategory::Commands => {
                        let mut ids: Vec<&CommandKindId> = snapshot.commands.keys().collect();
                        ids.sort();
                        for id in ids {
                            if !filter.is_empty() && !id.as_str().to_lowercase().contains(&filter) {
                                continue;
                            }
                            let selected = state
                                .selected
                                .as_deref()
                                .map(|s| s == id.as_str())
                                .unwrap_or(false);
                            if ui.selectable_label(selected, id.as_str()).clicked() {
                                state.selected = Some(Arc::from(id.as_str()));
                            }
                        }
                    }
                    InspectorCategory::Evidence => {
                        let mut ids: Vec<&EvidenceKindId> = snapshot.evidence.keys().collect();
                        ids.sort();
                        for id in ids {
                            if !filter.is_empty() && !id.as_str().to_lowercase().contains(&filter) {
                                continue;
                            }
                            let selected = state
                                .selected
                                .as_deref()
                                .map(|s| s == id.as_str())
                                .unwrap_or(false);
                            if ui.selectable_label(selected, id.as_str()).clicked() {
                                state.selected = Some(Arc::from(id.as_str()));
                            }
                        }
                    }
                    InspectorCategory::Pending => {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} pending",
                                snapshot.pending_commands.len()
                            ))
                            .strong(),
                        );
                    }
                });
        });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match state.category {
                InspectorCategory::Metrics => draw_metric_detail(ui, bus, &state.selected, now),
                InspectorCategory::Commands => draw_command_detail(ui, &snapshot, &state.selected),
                InspectorCategory::Evidence => draw_evidence_detail(ui, &snapshot, &state.selected),
                InspectorCategory::Pending => draw_pending_list(ui, &snapshot),
            });
    });
}

fn category_label(c: InspectorCategory) -> &'static str {
    match c {
        InspectorCategory::Metrics => "Metrics",
        InspectorCategory::Commands => "Commands",
        InspectorCategory::Evidence => "Evidence",
        InspectorCategory::Pending => "Pending",
    }
}

fn draw_metric_detail(ui: &mut egui::Ui, bus: &AiBus, selected: &Option<Arc<str>>, _now: i64) {
    let Some(name) = selected else {
        ui.label(
            egui::RichText::new("Select a metric from the list on the left.")
                .weak()
                .italics(),
        );
        return;
    };
    let id = MetricId::from(name.as_ref());
    let snapshot = bus.snapshot();
    let Some(metric) = snapshot.metrics.get(&id) else {
        ui.label(format!("Metric '{}' no longer declared.", name));
        return;
    };

    ui.label(egui::RichText::new(id.as_str()).strong().size(16.0));
    ui.label(format!(
        "Kind: {:?}  Retention: {:?}",
        metric.spec.kind, metric.spec.retention
    ));
    ui.label(
        egui::RichText::new(metric.spec.description.as_ref())
            .italics()
            .weak(),
    );
    ui.separator();

    let current = bus.current(&id);
    let latest_at = bus.latest_at(&id);
    ui.label(format!(
        "Current: {}",
        current
            .map(|v| format!("{:.4}", v))
            .unwrap_or_else(|| "—".into())
    ));
    ui.label(format!(
        "Latest at: {}",
        latest_at
            .map(|t| t.to_string())
            .unwrap_or_else(|| "—".into())
    ));
    ui.label(format!("History samples: {}", metric.history.len()));
    ui.separator();

    ui.label(egui::RichText::new("Last 10 samples:").strong());
    egui::Grid::new("ai_debug_metric_samples")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("tick").strong());
            ui.label(egui::RichText::new("value").strong());
            ui.end_row();
            for tv in metric.history.iter().rev().take(10) {
                ui.label(tv.at.to_string());
                ui.label(format!("{:.4}", tv.value));
                ui.end_row();
            }
        });
}

fn draw_command_detail(
    ui: &mut egui::Ui,
    snapshot: &macrocosmo_ai::BusSnapshot,
    selected: &Option<Arc<str>>,
) {
    let Some(name) = selected else {
        ui.label(
            egui::RichText::new("Select a command kind from the list on the left.")
                .weak()
                .italics(),
        );
        return;
    };
    let id = CommandKindId::from(name.as_ref());
    let Some(spec) = snapshot.commands.get(&id) else {
        ui.label(format!("Command kind '{}' no longer declared.", name));
        return;
    };
    ui.label(egui::RichText::new(id.as_str()).strong().size(16.0));
    ui.label(
        egui::RichText::new(spec.description.as_ref())
            .italics()
            .weak(),
    );
    ui.separator();
    let matching: Vec<_> = snapshot
        .pending_commands
        .iter()
        .filter(|c| c.kind == id)
        .collect();
    ui.label(format!("Pending of this kind: {}", matching.len()));
    for cmd in matching.iter().take(20) {
        ui.label(format!(
            "  [tick={}] issuer=Faction({}) priority={:.2}",
            cmd.at, cmd.issuer.0, cmd.priority
        ));
    }
}

fn draw_evidence_detail(
    ui: &mut egui::Ui,
    snapshot: &macrocosmo_ai::BusSnapshot,
    selected: &Option<Arc<str>>,
) {
    let Some(name) = selected else {
        ui.label(
            egui::RichText::new("Select an evidence kind from the list on the left.")
                .weak()
                .italics(),
        );
        return;
    };
    let id = EvidenceKindId::from(name.as_ref());
    let Some(store) = snapshot.evidence.get(&id) else {
        ui.label(format!("Evidence kind '{}' no longer declared.", name));
        return;
    };
    ui.label(egui::RichText::new(id.as_str()).strong().size(16.0));
    ui.label(format!("Retention: {:?}", store.spec.retention));
    ui.label(
        egui::RichText::new(store.spec.description.as_ref())
            .italics()
            .weak(),
    );
    ui.separator();
    ui.label(format!("Entries: {}", store.entries.len()));
    ui.label(egui::RichText::new("Last 20 entries:").strong());
    egui::Grid::new("ai_debug_evidence_entries")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("tick").strong());
            ui.label(egui::RichText::new("observer").strong());
            ui.label(egui::RichText::new("target").strong());
            ui.label(egui::RichText::new("magnitude").strong());
            ui.end_row();
            for ev in store.entries.iter().rev().take(20) {
                ui.label(ev.at.to_string());
                ui.label(format!("F({})", ev.observer.0));
                ui.label(format!("F({})", ev.target.0));
                ui.label(format!("{:+.3}", ev.magnitude));
                ui.end_row();
            }
        });
}

fn draw_pending_list(ui: &mut egui::Ui, snapshot: &macrocosmo_ai::BusSnapshot) {
    if snapshot.pending_commands.is_empty() {
        ui.label(egui::RichText::new("No pending commands.").weak().italics());
        return;
    }
    egui::Grid::new("ai_debug_pending_commands")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("kind").strong());
            ui.label(egui::RichText::new("issuer").strong());
            ui.label(egui::RichText::new("tick").strong());
            ui.label(egui::RichText::new("priority").strong());
            ui.end_row();
            for cmd in &snapshot.pending_commands {
                ui.label(cmd.kind.as_str());
                ui.label(format!("F({})", cmd.issuer.0));
                ui.label(cmd.at.to_string());
                ui.label(format!("{:.2}", cmd.priority));
                ui.end_row();
            }
        });
}
