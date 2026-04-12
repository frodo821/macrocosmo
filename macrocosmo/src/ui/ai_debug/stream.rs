//! Stream tab: rolling bus-event log produced by snapshot-diff sampling.

use bevy_egui::egui;

use super::{StreamEntry, StreamEvent, StreamFilter, StreamState};

pub fn draw_stream(ui: &mut egui::Ui, state: &mut StreamState) {
    ui.horizontal(|ui| {
        let pause_label = if state.paused { "Resume" } else { "Pause" };
        if ui.button(pause_label).clicked() {
            state.paused = !state.paused;
        }
        if ui.button("Clear").clicked() {
            state.log.clear();
        }
        ui.separator();
        ui.label("Filter:");
        for (f, label) in [
            (StreamFilter::All, "All"),
            (StreamFilter::MetricsOnly, "Metrics"),
            (StreamFilter::CommandsOnly, "Commands"),
            (StreamFilter::EvidenceOnly, "Evidence"),
        ] {
            if ui
                .selectable_label(state.filter == f, label)
                .clicked()
            {
                state.filter = f;
            }
        }
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(format!("{} / {}", state.log.len(), super::STREAM_LOG_CAP))
                        .weak()
                        .small(),
                );
            },
        );
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(!state.paused)
        .show(ui, |ui| {
            for entry in state.log.iter().rev() {
                if !passes_filter(entry, state.filter) {
                    continue;
                }
                ui.label(format_entry(entry));
            }
        });
}

fn passes_filter(entry: &StreamEntry, filter: StreamFilter) -> bool {
    match (filter, &entry.event) {
        (StreamFilter::All, _) => true,
        (StreamFilter::MetricsOnly, StreamEvent::MetricEmit { .. }) => true,
        (StreamFilter::CommandsOnly, StreamEvent::CommandEnqueued { .. }) => true,
        (StreamFilter::EvidenceOnly, StreamEvent::EvidenceEmitted { .. }) => true,
        // Declarations show up under "All" only so the headline view stays
        // focused when filtered.
        _ => false,
    }
}

fn format_entry(entry: &StreamEntry) -> String {
    match &entry.event {
        StreamEvent::MetricEmit { id, value } => {
            format!("[tick={}] METRIC {}={:.4}", entry.at, id.as_str(), value)
        }
        StreamEvent::CommandEnqueued {
            kind,
            issuer,
            priority,
        } => format!(
            "[tick={}] COMMAND {} by Faction({}) priority={:.2}",
            entry.at,
            kind.as_str(),
            issuer.0,
            priority
        ),
        StreamEvent::EvidenceEmitted {
            kind,
            observer,
            target,
            magnitude,
        } => format!(
            "[tick={}] EVIDENCE {} F({})->F({}) mag={:+.3}",
            entry.at,
            kind.as_str(),
            observer.0,
            target.0,
            magnitude
        ),
        StreamEvent::DeclarationAdded { kind, id } => {
            format!("[tick={}] DECLARE {}({})", entry.at, kind, id)
        }
    }
}
