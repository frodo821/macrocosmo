//! `RecordingBus` — a decorator around `AiBus` that captures every accepted
//! emit for later serialization / replay.
//!
//! The recorder mirrors bus acceptance semantics exactly: emits that the bus
//! would silently drop (undeclared kind, time-reversed) are also dropped here
//! and **not** recorded, so replay produces a byte-identical final bus state.

use std::collections::BTreeMap;

use crate::bus::AiBus;
use crate::command::{Command, SerializedCommand};
use crate::evidence::StandingEvidence;
use crate::ids::{CommandKindId, EvidenceKindId, MetricId};
use crate::spec::{CommandSpec, EvidenceSpec, MetricSpec};
use crate::time::Tick;

use super::record::{
    Declarations, Playthrough, PlaythroughEvent, PlaythroughMeta, SUPPORTED_VERSION, ScenarioConfig,
};

/// Wraps an `AiBus` and records every accepted declaration + emit.
///
/// Usage:
/// 1. Create via `RecordingBus::new(bus)`.
/// 2. Call the `declare_*` / `emit*` methods instead of touching the bus directly.
/// 3. Call `finish(meta, config)` to produce a `Playthrough`.
///
/// You can still read the underlying bus via `bus()` / `bus_mut()` for queries,
/// but emitting through the inner bus will bypass the recorder.
pub struct RecordingBus {
    bus: AiBus,
    declarations: Declarations,
    events: Vec<PlaythroughEvent>,
    /// Track the last accepted timestamp per metric id so we can pre-check
    /// monotonicity before forwarding to the bus (matches bus semantics).
    last_metric_at: BTreeMap<MetricId, Tick>,
    /// Same, per evidence kind.
    last_evidence_at: BTreeMap<EvidenceKindId, Tick>,
}

impl RecordingBus {
    pub fn new(bus: AiBus) -> Self {
        Self {
            bus,
            declarations: Declarations::default(),
            events: Vec::new(),
            last_metric_at: BTreeMap::new(),
            last_evidence_at: BTreeMap::new(),
        }
    }

    // ---- Declarations ---------------------------------------------------

    pub fn declare_metric(&mut self, id: MetricId, spec: MetricSpec) {
        self.bus.declare_metric(id.clone(), spec.clone());
        self.declarations.metrics.insert(id, spec);
    }

    pub fn declare_command(&mut self, kind: CommandKindId, spec: CommandSpec) {
        self.bus.declare_command(kind.clone(), spec.clone());
        self.declarations.commands.insert(kind, spec);
    }

    pub fn declare_evidence(&mut self, kind: EvidenceKindId, spec: EvidenceSpec) {
        self.bus.declare_evidence(kind.clone(), spec.clone());
        self.declarations.evidence.insert(kind, spec);
    }

    // ---- Emits ----------------------------------------------------------

    /// Emit a metric sample. Only recorded if the bus would accept it
    /// (declared kind + non-reversed time).
    pub fn emit(&mut self, id: &MetricId, value: f64, at: Tick) {
        if !self.bus.has_metric(id) {
            // Forward anyway so the bus's warning semantics fire; do not record.
            self.bus.emit(id, value, at);
            return;
        }
        if let Some(&last_at) = self.last_metric_at.get(id) {
            if at < last_at {
                self.bus.emit(id, value, at); // bus will warn + drop
                return;
            }
        }
        self.bus.emit(id, value, at);
        self.last_metric_at.insert(id.clone(), at);
        self.events.push(PlaythroughEvent::Metric {
            id: id.clone(),
            value,
            at,
        });
    }

    /// Emit a command. Only recorded if the command kind is declared.
    pub fn emit_command(&mut self, cmd: Command) {
        if !self.bus.has_command_kind(&cmd.kind) {
            self.bus.emit_command(cmd); // bus will warn + drop
            return;
        }
        let serialized = SerializedCommand::from(cmd.clone());
        self.bus.emit_command(cmd);
        self.events.push(PlaythroughEvent::Command(serialized));
    }

    /// Emit evidence. Only recorded if the kind is declared and time is
    /// non-reversed.
    pub fn emit_evidence(&mut self, ev: StandingEvidence) {
        if !self.bus.has_evidence_kind(&ev.kind) {
            self.bus.emit_evidence(ev); // bus will warn + drop
            return;
        }
        if let Some(&last_at) = self.last_evidence_at.get(&ev.kind) {
            if ev.at < last_at {
                self.bus.emit_evidence(ev);
                return;
            }
        }
        let kind = ev.kind.clone();
        let at = ev.at;
        let cloned = ev.clone();
        self.bus.emit_evidence(ev);
        self.last_evidence_at.insert(kind, at);
        self.events.push(PlaythroughEvent::Evidence(cloned));
    }

    // ---- Accessors ------------------------------------------------------

    pub fn bus(&self) -> &AiBus {
        &self.bus
    }

    pub fn bus_mut(&mut self) -> &mut AiBus {
        &mut self.bus
    }

    pub fn declarations(&self) -> &Declarations {
        &self.declarations
    }

    pub fn events(&self) -> &[PlaythroughEvent] {
        &self.events
    }

    /// Consume the recorder, producing a serializable `Playthrough`.
    pub fn finish(self, meta: PlaythroughMeta, config: ScenarioConfig) -> Playthrough {
        Playthrough {
            version: SUPPORTED_VERSION,
            meta,
            config,
            declarations: self.declarations,
            events: self.events,
        }
    }

    /// Consume the recorder and return only the inner `AiBus`. Useful when a
    /// caller wants the populated bus but no playthrough artifact.
    pub fn into_bus(self) -> AiBus {
        self.bus
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::FactionId;
    use crate::retention::Retention;
    use crate::spec::{CommandSpec, EvidenceSpec, MetricSpec};
    use crate::warning::WarningMode;

    fn bus() -> AiBus {
        AiBus::with_warning_mode(WarningMode::Silent)
    }

    #[test]
    fn records_only_accepted_metric_emits() {
        let mut rb = RecordingBus::new(bus());
        let m = MetricId::from("m");
        rb.declare_metric(m.clone(), MetricSpec::gauge(Retention::Long, "m"));
        rb.emit(&m, 1.0, 10);
        rb.emit(&m, 2.0, 20);
        rb.emit(&m, 99.0, 5); // time-reversed; dropped
        rb.emit(&MetricId::from("undeclared"), 0.0, 30); // undeclared; dropped
        assert_eq!(rb.events().len(), 2);
    }

    #[test]
    fn records_command_emits() {
        let mut rb = RecordingBus::new(bus());
        let k = CommandKindId::from("k");
        rb.declare_command(k.clone(), CommandSpec::new("k"));
        rb.emit_command(Command::new(k.clone(), FactionId(1), 0));
        rb.emit_command(Command::new(CommandKindId::from("other"), FactionId(1), 0));
        assert_eq!(rb.events().len(), 1);
    }

    #[test]
    fn records_evidence_emits() {
        let mut rb = RecordingBus::new(bus());
        let k = EvidenceKindId::from("k");
        rb.declare_evidence(k.clone(), EvidenceSpec::new(Retention::Long, "k"));
        rb.emit_evidence(StandingEvidence::new(
            k.clone(),
            FactionId(1),
            FactionId(2),
            1.0,
            10,
        ));
        rb.emit_evidence(StandingEvidence::new(
            k.clone(),
            FactionId(1),
            FactionId(2),
            1.0,
            5, // reversed
        ));
        assert_eq!(rb.events().len(), 1);
    }
}
