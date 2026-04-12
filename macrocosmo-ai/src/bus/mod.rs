//! `AiBus` — the central typed topic bus.
//!
//! Topics come in three flavors: metrics (time-series f64), commands (AI→game
//! output queue), and evidence (per-kind standing observations).
//!
//! All topics must be declared via the corresponding `declare_*` before use.
//! Emitting to an undeclared topic is a no-op + warning.

pub(crate) mod command;
pub(crate) mod evidence;
pub(crate) mod metric;
pub mod snapshot;

use ahash::AHashMap;

use crate::bus::command::CommandStore;
use crate::bus::evidence::EvidenceStore;
use crate::bus::metric::MetricStore;
use crate::bus::snapshot::{BusSnapshot, EvidenceSnapshot, MetricSnapshot};
use crate::bus_warn;
use crate::command::Command;
use crate::command::SerializedCommand;
use crate::evidence::StandingEvidence;
use crate::ids::{CommandKindId, EvidenceKindId, FactionId, MetricId};
use crate::spec::{CommandSpec, EvidenceSpec, MetricSpec};
use crate::time::{Tick, TimestampedValue};
use crate::warning::WarningMode;

/// Central AI bus — holds all declared topics and their histories.
#[derive(Debug, Default)]
pub struct AiBus {
    metrics: AHashMap<MetricId, MetricStore>,
    commands: CommandStore,
    evidence: AHashMap<EvidenceKindId, EvidenceStore>,
    pub(crate) warning_mode: WarningMode,
}

impl AiBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_warning_mode(mode: WarningMode) -> Self {
        Self {
            warning_mode: mode,
            ..Self::default()
        }
    }

    pub fn set_warning_mode(&mut self, mode: WarningMode) {
        self.warning_mode = mode;
    }

    pub fn warning_mode(&self) -> WarningMode {
        self.warning_mode
    }

    // ---- Metric topic ---------------------------------------------------

    /// Declare a metric topic. Re-declaring an existing id overrides the spec
    /// (preserving history) and emits a warning.
    pub fn declare_metric(&mut self, id: MetricId, spec: MetricSpec) {
        if let Some(existing) = self.metrics.get_mut(&id) {
            bus_warn!(
                self.warning_mode,
                "metric '{id}' re-declared; overriding spec {:?} -> {:?}",
                existing.spec,
                spec,
            );
            existing.spec = spec;
        } else {
            self.metrics.insert(id, MetricStore::new(spec));
        }
    }

    /// Whether a metric has been declared.
    pub fn has_metric(&self, id: &MetricId) -> bool {
        self.metrics.contains_key(id)
    }

    /// Emit a metric sample. No-op + warning if the id is undeclared or if
    /// `at` precedes the latest emitted sample for this metric.
    pub fn emit(&mut self, id: &MetricId, value: f64, at: Tick) {
        let Some(store) = self.metrics.get_mut(id) else {
            bus_warn!(
                self.warning_mode,
                "emit to undeclared metric '{id}' (value={value}, at={at}); dropping"
            );
            return;
        };
        if !store.push(at, value) {
            let last_at = store.history.back().map(|tv| tv.at).unwrap_or(i64::MIN);
            bus_warn!(
                self.warning_mode,
                "time-reversed emit on metric '{id}' (at={at}, last={last_at}); dropping"
            );
        }
    }

    /// Latest value for a metric, or `None` if undeclared / never emitted.
    pub fn current(&self, id: &MetricId) -> Option<f64> {
        self.metrics.get(id).and_then(MetricStore::current)
    }

    /// Iterator over samples in the window `[now - duration, now]`, oldest-first.
    ///
    /// If the metric is undeclared, yields an empty iterator. If `duration`
    /// exceeds retention, yields everything still in history (partial).
    pub fn window<'a>(
        &'a self,
        id: &MetricId,
        now: Tick,
        duration: Tick,
    ) -> Box<dyn Iterator<Item = &'a TimestampedValue> + 'a> {
        match self.metrics.get(id) {
            Some(store) => Box::new(store.window(now, duration)),
            None => Box::new(std::iter::empty()),
        }
    }

    /// Exact-timestamp lookup. Returns `None` if undeclared or no sample exists
    /// at that tick.
    pub fn at(&self, id: &MetricId, t: Tick) -> Option<f64> {
        self.metrics.get(id).and_then(|s| s.at(t))
    }

    /// Latest sample at-or-before `t`. Used by DelT and other lookback
    /// evaluators where an exact match is not guaranteed.
    pub fn at_or_before(&self, id: &MetricId, t: Tick) -> Option<f64> {
        self.metrics.get(id).and_then(|s| s.at_or_before(t))
    }

    // ---- Command topic --------------------------------------------------

    /// Declare a command kind. Re-declaring overrides the spec and warns.
    pub fn declare_command(&mut self, kind: CommandKindId, spec: CommandSpec) {
        if let Some(existing) = self.commands.specs.get_mut(&kind) {
            bus_warn!(
                self.warning_mode,
                "command kind '{kind}' re-declared; overriding spec {:?} -> {:?}",
                existing,
                spec,
            );
            *existing = spec;
        } else {
            self.commands.specs.insert(kind, spec);
        }
    }

    /// Whether a command kind has been declared.
    pub fn has_command_kind(&self, kind: &CommandKindId) -> bool {
        self.commands.specs.contains_key(kind)
    }

    /// Emit a command. No-op + warning if the command's kind is undeclared.
    /// The command's `at` field is supplied by the caller (typically the
    /// current game tick).
    pub fn emit_command(&mut self, cmd: Command) {
        if !self.commands.specs.contains_key(&cmd.kind) {
            bus_warn!(
                self.warning_mode,
                "emit of undeclared command kind '{}'; dropping",
                cmd.kind
            );
            return;
        }
        self.commands.pending.push(cmd);
    }

    /// Drain all pending commands. The game consumer calls this each tick.
    pub fn drain_commands(&mut self) -> Vec<Command> {
        self.commands.drain()
    }

    /// Non-draining peek at the pending command queue.
    pub fn pending_commands(&self) -> &[Command] {
        &self.commands.pending
    }

    // ---- Evidence topic -------------------------------------------------

    /// Declare an evidence kind. Re-declaring overrides the spec and warns.
    pub fn declare_evidence(&mut self, kind: EvidenceKindId, spec: EvidenceSpec) {
        if let Some(existing) = self.evidence.get_mut(&kind) {
            bus_warn!(
                self.warning_mode,
                "evidence kind '{kind}' re-declared; overriding spec {:?} -> {:?}",
                existing.spec,
                spec,
            );
            existing.spec = spec;
        } else {
            self.evidence.insert(kind, EvidenceStore::new(spec));
        }
    }

    /// Whether an evidence kind has been declared.
    pub fn has_evidence_kind(&self, kind: &EvidenceKindId) -> bool {
        self.evidence.contains_key(kind)
    }

    /// Emit a standing evidence observation. No-op + warning if the kind is
    /// undeclared or the timestamp precedes the latest sample for this kind.
    pub fn emit_evidence(&mut self, ev: StandingEvidence) {
        let kind = ev.kind.clone();
        let Some(store) = self.evidence.get_mut(&kind) else {
            bus_warn!(
                self.warning_mode,
                "emit of undeclared evidence kind '{kind}' (observer={:?}, target={:?}, at={}); dropping",
                ev.observer,
                ev.target,
                ev.at,
            );
            return;
        };
        let at = ev.at;
        if !store.push(ev) {
            let last_at = store.entries.last().map(|e| e.at).unwrap_or(i64::MIN);
            bus_warn!(
                self.warning_mode,
                "time-reversed evidence emit on kind '{kind}' (at={at}, last={last_at}); dropping"
            );
        }
    }

    /// Evidence for a given observer within `[now - duration, now]`, across
    /// all declared evidence kinds. Iterator yields references into the
    /// underlying stores.
    pub fn evidence_for<'a>(
        &'a self,
        observer: FactionId,
        now: Tick,
        duration: Tick,
    ) -> impl Iterator<Item = &'a StandingEvidence> + 'a {
        self.evidence
            .values()
            .flat_map(move |store| store.window(now, duration))
            .filter(move |e| e.observer == observer)
    }

    /// Evidence for a given observer and kind within the window. More
    /// efficient than `evidence_for` when the caller knows the kind.
    pub fn evidence_of_kind<'a>(
        &'a self,
        kind: &EvidenceKindId,
        observer: FactionId,
        now: Tick,
        duration: Tick,
    ) -> Box<dyn Iterator<Item = &'a StandingEvidence> + 'a> {
        match self.evidence.get(kind) {
            Some(store) => Box::new(store.window(now, duration).filter(move |e| e.observer == observer)),
            None => Box::new(std::iter::empty()),
        }
    }

    // ---- Snapshot -------------------------------------------------------

    /// Produce a read-only, deterministic-order snapshot of the entire bus
    /// state. Useful for equivalence checking (record/replay tests) and
    /// serialization. Unconditionally available — cost is one allocation per
    /// call, which is irrelevant outside tests.
    pub fn snapshot(&self) -> BusSnapshot {
        let metrics = self
            .metrics
            .iter()
            .map(|(id, store)| {
                (
                    id.clone(),
                    MetricSnapshot {
                        spec: store.spec.clone(),
                        history: store.history.iter().cloned().collect(),
                    },
                )
            })
            .collect();

        let commands = self
            .commands
            .specs
            .iter()
            .map(|(k, spec)| (k.clone(), spec.clone()))
            .collect();

        let pending_commands = self
            .commands
            .pending
            .iter()
            .cloned()
            .map(SerializedCommand::from)
            .collect();

        let evidence = self
            .evidence
            .iter()
            .map(|(k, store)| {
                (
                    k.clone(),
                    EvidenceSnapshot {
                        spec: store.spec.clone(),
                        entries: store.entries.clone(),
                    },
                )
            })
            .collect();

        BusSnapshot {
            metrics,
            commands,
            pending_commands,
            evidence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::Retention;

    fn readiness() -> MetricId {
        MetricId::from("fleet_readiness")
    }

    fn hostile() -> EvidenceKindId {
        EvidenceKindId::from("hostile_engagement")
    }

    #[test]
    fn declare_emit_current() {
        let mut bus = AiBus::new();
        bus.declare_metric(readiness(), MetricSpec::ratio(Retention::Short, "r"));
        bus.emit(&readiness(), 0.7, 10);
        bus.emit(&readiness(), 0.8, 20);
        assert_eq!(bus.current(&readiness()), Some(0.8));
    }

    #[test]
    fn emit_to_undeclared_is_noop() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        bus.emit(&MetricId::from("unknown"), 1.0, 0);
        assert_eq!(bus.current(&MetricId::from("unknown")), None);
    }

    #[test]
    fn time_reversed_emit_dropped() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        bus.declare_metric(readiness(), MetricSpec::ratio(Retention::Short, "r"));
        bus.emit(&readiness(), 0.5, 100);
        bus.emit(&readiness(), 0.9, 50); // reversed; dropped
        assert_eq!(bus.current(&readiness()), Some(0.5));
    }

    #[test]
    fn redeclare_overrides_spec_preserves_history() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        bus.declare_metric(readiness(), MetricSpec::ratio(Retention::Short, "old"));
        bus.emit(&readiness(), 0.5, 10);
        bus.declare_metric(readiness(), MetricSpec::ratio(Retention::Long, "new"));
        assert_eq!(bus.current(&readiness()), Some(0.5));
    }

    #[test]
    fn window_collects_samples() {
        let mut bus = AiBus::new();
        bus.declare_metric(readiness(), MetricSpec::ratio(Retention::Long, "r"));
        for t in 0..10 {
            bus.emit(&readiness(), t as f64 / 10.0, t * 5);
        }
        let collected: Vec<f64> = bus.window(&readiness(), 25, 10).map(|tv| tv.value).collect();
        assert_eq!(collected, vec![0.3, 0.4, 0.5]);
    }

    #[test]
    fn window_on_undeclared_is_empty() {
        let bus = AiBus::new();
        let got: Vec<_> = bus.window(&MetricId::from("no"), 100, 50).collect();
        assert!(got.is_empty());
    }

    #[test]
    fn declare_emit_drain_command() {
        let mut bus = AiBus::new();
        let kind = CommandKindId::from("attack_target");
        bus.declare_command(kind.clone(), CommandSpec::new("attack"));
        bus.emit_command(Command::new(kind.clone(), FactionId(1), 42).with_priority(0.8));
        bus.emit_command(Command::new(kind.clone(), FactionId(1), 43));
        let drained = bus.drain_commands();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].at, 42);
        assert!(bus.drain_commands().is_empty());
    }

    #[test]
    fn emit_undeclared_command_kind_is_noop() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        bus.emit_command(Command::new(CommandKindId::from("unknown"), FactionId(1), 0));
        assert!(bus.drain_commands().is_empty());
    }

    #[test]
    fn redeclare_command_overrides_spec() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let kind = CommandKindId::from("cmd");
        bus.declare_command(kind.clone(), CommandSpec::new("old"));
        bus.declare_command(kind.clone(), CommandSpec::new("new"));
        assert!(bus.has_command_kind(&kind));
    }

    #[test]
    fn declare_emit_query_evidence() {
        let mut bus = AiBus::new();
        bus.declare_evidence(hostile(), EvidenceSpec::new(Retention::Long, "hostile"));
        bus.emit_evidence(StandingEvidence::new(
            hostile(),
            FactionId(1),
            FactionId(2),
            1.0,
            10,
        ));
        bus.emit_evidence(StandingEvidence::new(
            hostile(),
            FactionId(1),
            FactionId(3),
            0.5,
            20,
        ));
        bus.emit_evidence(StandingEvidence::new(
            hostile(),
            FactionId(2),
            FactionId(1),
            2.0,
            25,
        ));

        // Filter by observer=1
        let got: Vec<_> = bus.evidence_for(FactionId(1), 30, 50).collect();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.observer == FactionId(1)));

        // Filter by observer=2
        let got2: Vec<_> = bus.evidence_for(FactionId(2), 30, 50).collect();
        assert_eq!(got2.len(), 1);
    }

    #[test]
    fn emit_undeclared_evidence_is_noop() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        bus.emit_evidence(StandingEvidence::new(
            hostile(),
            FactionId(1),
            FactionId(2),
            1.0,
            0,
        ));
        let got: Vec<_> = bus.evidence_for(FactionId(1), 0, 100).collect();
        assert!(got.is_empty());
    }

    #[test]
    fn evidence_of_kind_filter() {
        let mut bus = AiBus::new();
        bus.declare_evidence(hostile(), EvidenceSpec::new(Retention::Long, "h"));
        let other = EvidenceKindId::from("trade");
        bus.declare_evidence(other.clone(), EvidenceSpec::new(Retention::Long, "t"));
        bus.emit_evidence(StandingEvidence::new(
            hostile(),
            FactionId(1),
            FactionId(2),
            1.0,
            10,
        ));
        bus.emit_evidence(StandingEvidence::new(
            other.clone(),
            FactionId(1),
            FactionId(2),
            1.0,
            20,
        ));
        let hostile_only: Vec<_> = bus
            .evidence_of_kind(&hostile(), FactionId(1), 30, 50)
            .collect();
        assert_eq!(hostile_only.len(), 1);
        assert_eq!(hostile_only[0].kind, hostile());
    }

    #[test]
    fn at_and_at_or_before() {
        let mut bus = AiBus::new();
        bus.declare_metric(readiness(), MetricSpec::ratio(Retention::Long, "r"));
        bus.emit(&readiness(), 0.1, 10);
        bus.emit(&readiness(), 0.2, 20);
        assert_eq!(bus.at(&readiness(), 20), Some(0.2));
        assert_eq!(bus.at(&readiness(), 15), None);
        assert_eq!(bus.at_or_before(&readiness(), 15), Some(0.1));
        assert_eq!(bus.at_or_before(&readiness(), 5), None);
    }
}
