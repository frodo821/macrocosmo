//! AI Debug UI — developer-facing inspector for the AI bus (#197 v1).
//!
//! Press F10 in-game to open the "AI Debug" window. The window has five tabs:
//!
//! - **Inspector**: browse declared metrics / commands / evidence kinds and
//!   inspect their current values and history.
//! - **Plots**: select metrics and view recent samples as line charts
//!   (hand-rolled egui painter — no `egui_plot` dependency).
//! - **Stream**: rolling log of bus events produced by diffing consecutive
//!   `BusSnapshot`s.
//! - **Governor**: per-faction overview of canonical Tier 1 metrics.
//! - **Replay**: load a `Playthrough` JSON file and step through events on an
//!   isolated bus.
//!
//! The UI is strictly read-only and safe to ship in release builds. All
//! tabs share one `AiDebugUi` resource; systems run in `EguiPrimaryContextPass`
//! chained after the existing overlay drawing systems.

use std::collections::VecDeque;
use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use macrocosmo_ai::{
    BusSnapshot, CommandKindId, EvidenceKindId, FactionId, MetricId, playthrough::Playthrough,
};

use crate::ai::plugin::AiBusResource;
use crate::time_system::GameClock;

pub mod governor;
pub mod inspector;
pub mod plots;
pub mod replay;
pub mod stream;

#[cfg(test)]
mod tests;

/// Maximum number of entries retained in the stream log.
pub const STREAM_LOG_CAP: usize = 512;

/// Active tab in the debug window.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugTab {
    #[default]
    Inspector,
    Plots,
    Stream,
    Governor,
    Replay,
}

/// Inspector category for the topic browser.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectorCategory {
    #[default]
    Metrics,
    Commands,
    Evidence,
    Pending,
}

#[derive(Default, Debug)]
pub struct InspectorState {
    pub filter: String,
    pub category: InspectorCategory,
    pub selected: Option<Arc<str>>,
}

#[derive(Debug)]
pub struct PlotsState {
    pub selected_metrics: Vec<MetricId>,
    /// Time window in ticks to plot (e.g. 500 hexadies).
    pub window_ticks: i64,
}

impl Default for PlotsState {
    fn default() -> Self {
        Self {
            selected_metrics: Vec::new(),
            window_ticks: 500,
        }
    }
}

/// What kind of event the stream log saw.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    MetricEmit {
        id: MetricId,
        value: f64,
    },
    CommandEnqueued {
        kind: CommandKindId,
        issuer: FactionId,
        priority: f64,
    },
    EvidenceEmitted {
        kind: EvidenceKindId,
        observer: FactionId,
        target: FactionId,
        magnitude: f64,
    },
    DeclarationAdded {
        kind: &'static str,
        id: String,
    },
}

#[derive(Debug, Clone)]
pub struct StreamEntry {
    pub at: i64,
    pub event: StreamEvent,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFilter {
    #[default]
    All,
    MetricsOnly,
    CommandsOnly,
    EvidenceOnly,
}

#[derive(Default, Debug)]
pub struct StreamState {
    pub log: VecDeque<StreamEntry>,
    pub paused: bool,
    pub filter: StreamFilter,
}

impl StreamState {
    /// Push an entry, enforcing the ring-buffer cap.
    pub fn push(&mut self, entry: StreamEntry) {
        self.log.push_back(entry);
        while self.log.len() > STREAM_LOG_CAP {
            self.log.pop_front();
        }
    }
}

#[derive(Debug)]
pub struct GovernorState {
    pub faction: u32,
}

impl Default for GovernorState {
    fn default() -> Self {
        Self { faction: 0 }
    }
}

#[derive(Default, Debug)]
pub struct ReplayState {
    pub path_input: String,
    pub last_error: Option<String>,
    pub loaded: Option<Playthrough>,
    /// How many events have been applied to `bus` (0..=loaded.events.len()).
    pub cursor: usize,
    pub bus: Option<macrocosmo_ai::AiBus>,
}

/// Resource backing the AI Debug UI.
///
/// **Reflect skipped**: this resource transitively contains types from
/// the engine-agnostic `macrocosmo-ai` crate (`AiBus`, `BusSnapshot`,
/// `Playthrough`, `MetricId`, etc.). `macrocosmo-ai` cannot take a
/// `bevy_reflect` dependency (enforced by `ai-core-isolation.yml`),
/// and `AiBus` is not `Clone`, so neither field-level Reflect nor
/// `#[reflect(opaque)]` (which requires `Clone`) can be applied. As a
/// result `AiDebugUi` is not introspectable via BRP — this is fine
/// because it's a UI debug overlay, not gameplay state.
#[derive(Resource, Default)]
pub struct AiDebugUi {
    pub open: bool,
    pub active_tab: DebugTab,
    pub inspector: InspectorState,
    pub plots: PlotsState,
    pub stream: StreamState,
    pub governor: GovernorState,
    pub replay: ReplayState,
    /// Last snapshot seen by `sample_ai_debug_stream`, used for diffing.
    pub last_snapshot: Option<BusSnapshot>,
}

/// Toggle the debug window. Default binding is F10; rebindable via the
/// #347 keybinding registry under
/// [`crate::input::actions::UI_TOGGLE_AI_DEBUG`].
pub fn toggle_ai_debug(
    keys: Res<ButtonInput<KeyCode>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    mut ui: ResMut<AiDebugUi>,
) {
    let pressed = match keybindings.as_deref() {
        Some(kb) => kb.is_just_pressed(crate::input::actions::UI_TOGGLE_AI_DEBUG, &keys),
        None => keys.just_pressed(KeyCode::F10),
    };
    if pressed {
        ui.open = !ui.open;
    }
}

/// Compute stream events by diffing the current bus snapshot against the
/// previously-observed one. Returns the new entries produced by this diff
/// (caller decides whether to push them or drop them).
pub fn diff_snapshots(
    previous: Option<&BusSnapshot>,
    current: &BusSnapshot,
    now: i64,
) -> Vec<StreamEntry> {
    let mut out = Vec::new();

    // Metric declarations / new metric samples.
    for (id, snap) in &current.metrics {
        match previous.and_then(|p| p.metrics.get(id)) {
            None => {
                out.push(StreamEntry {
                    at: now,
                    event: StreamEvent::DeclarationAdded {
                        kind: "metric",
                        id: id.as_str().to_string(),
                    },
                });
                // Every historical sample is "new" on first appearance.
                for tv in &snap.history {
                    out.push(StreamEntry {
                        at: tv.at,
                        event: StreamEvent::MetricEmit {
                            id: id.clone(),
                            value: tv.value,
                        },
                    });
                }
            }
            Some(prev) => {
                let prev_last = prev.history.last().map(|tv| tv.at).unwrap_or(i64::MIN);
                for tv in &snap.history {
                    if tv.at > prev_last {
                        out.push(StreamEntry {
                            at: tv.at,
                            event: StreamEvent::MetricEmit {
                                id: id.clone(),
                                value: tv.value,
                            },
                        });
                    }
                }
            }
        }
    }

    // Command kind declarations.
    for id in current.commands.keys() {
        if previous
            .map(|p| !p.commands.contains_key(id))
            .unwrap_or(true)
        {
            out.push(StreamEntry {
                at: now,
                event: StreamEvent::DeclarationAdded {
                    kind: "command",
                    id: id.as_str().to_string(),
                },
            });
        }
    }

    // Pending commands that appeared since last snapshot. Commands are
    // drained each tick by CommandDrain, but we may still catch some in
    // the window; a naive "not in previous by (kind,issuer,at)" check is
    // enough for a rolling log.
    let prev_pending: std::collections::HashSet<(CommandKindId, FactionId, i64)> = previous
        .map(|p| {
            p.pending_commands
                .iter()
                .map(|c| (c.kind.clone(), c.issuer, c.at))
                .collect()
        })
        .unwrap_or_default();
    for cmd in &current.pending_commands {
        let key = (cmd.kind.clone(), cmd.issuer, cmd.at);
        if !prev_pending.contains(&key) {
            out.push(StreamEntry {
                at: cmd.at,
                event: StreamEvent::CommandEnqueued {
                    kind: cmd.kind.clone(),
                    issuer: cmd.issuer,
                    priority: cmd.priority,
                },
            });
        }
    }

    // Evidence declarations + new evidence entries.
    for (id, snap) in &current.evidence {
        match previous.and_then(|p| p.evidence.get(id)) {
            None => {
                out.push(StreamEntry {
                    at: now,
                    event: StreamEvent::DeclarationAdded {
                        kind: "evidence",
                        id: id.as_str().to_string(),
                    },
                });
                for ev in &snap.entries {
                    out.push(StreamEntry {
                        at: ev.at,
                        event: StreamEvent::EvidenceEmitted {
                            kind: ev.kind.clone(),
                            observer: ev.observer,
                            target: ev.target,
                            magnitude: ev.magnitude,
                        },
                    });
                }
            }
            Some(prev) => {
                let prev_last = prev.entries.last().map(|e| e.at).unwrap_or(i64::MIN);
                for ev in &snap.entries {
                    if ev.at > prev_last {
                        out.push(StreamEntry {
                            at: ev.at,
                            event: StreamEvent::EvidenceEmitted {
                                kind: ev.kind.clone(),
                                observer: ev.observer,
                                target: ev.target,
                                magnitude: ev.magnitude,
                            },
                        });
                    }
                }
            }
        }
    }

    out
}

/// Sample the bus snapshot and update the stream log. Runs each frame
/// when the debug window is open.
pub fn sample_ai_debug_stream(
    mut ui: ResMut<AiDebugUi>,
    bus: Res<AiBusResource>,
    clock: Res<GameClock>,
) {
    if !ui.open {
        return;
    }
    let current = bus.0.snapshot();
    if !ui.stream.paused {
        let entries = diff_snapshots(ui.last_snapshot.as_ref(), &current, clock.elapsed);
        for entry in entries {
            ui.stream.push(entry);
        }
    }
    ui.last_snapshot = Some(current);
}

/// Draw the AI Debug window and dispatch to the active tab.
pub fn draw_ai_debug_system(
    mut contexts: EguiContexts,
    mut ui_res: ResMut<AiDebugUi>,
    bus: Res<AiBusResource>,
    clock: Res<GameClock>,
) {
    if !ui_res.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Borrow split: `open` owned separately so window close (X) works.
    let mut open = ui_res.open;
    let now = clock.elapsed;

    egui::Window::new("AI Debug")
        .id(egui::Id::new("ai_debug_window"))
        .open(&mut open)
        .resizable(true)
        .default_size([720.0, 520.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut ui_res.active_tab, DebugTab::Inspector, "Inspector");
                ui.selectable_value(&mut ui_res.active_tab, DebugTab::Plots, "Plots");
                ui.selectable_value(&mut ui_res.active_tab, DebugTab::Stream, "Stream");
                ui.selectable_value(&mut ui_res.active_tab, DebugTab::Governor, "Governor");
                ui.selectable_value(&mut ui_res.active_tab, DebugTab::Replay, "Replay");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("tick {}", now)).weak().small());
                });
            });
            ui.separator();

            let AiDebugUi {
                active_tab,
                inspector,
                plots,
                stream,
                governor,
                replay,
                ..
            } = &mut *ui_res;

            match *active_tab {
                DebugTab::Inspector => {
                    inspector::draw_inspector(ui, inspector, &bus.0, now);
                }
                DebugTab::Plots => {
                    plots::draw_plots(ui, plots, &bus.0, now);
                }
                DebugTab::Stream => {
                    stream::draw_stream(ui, stream);
                }
                DebugTab::Governor => {
                    governor::draw_governor(ui, governor, &bus.0);
                }
                DebugTab::Replay => {
                    replay::draw_replay(ui, replay);
                }
            }
        });

    ui_res.open = open;
}
