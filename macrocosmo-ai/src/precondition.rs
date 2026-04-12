//! Preconditions — severity-weighted [`Condition`] wrappers.
//!
//! A [`PreconditionSet`] is the primary API surface for encoding "should we
//! keep this Objective / Campaign / Intent alive?" checks. Each item pairs
//! a boolean [`Condition`] with a severity in `[0.0, 1.0]`. Severity
//! `CRITICAL` (1.0) violations surface separately so that callers can wire
//! them to hard-abort logic; lower severities contribute to a weighted
//! satisfaction score used for soft grading.
//!
//! The module is split into three layers:
//! - [`PreconditionItem`] / [`PreconditionSet`]: definition
//! - [`PreconditionEvalResult`] / [`PreconditionSummary`]: evaluation output
//! - [`PreconditionTracker`]: history / `violated_for` bookkeeping
//!
//! Cached evaluation is provided separately in
//! [`crate::precondition_cache`].

use std::sync::Arc;

use ahash::AHashMap;
use serde::{Deserialize, Serialize};

use crate::condition::Condition;
use crate::eval::EvalContext;
use crate::time::Tick;

/// Severity constants.
///
/// Mirrors the Lua-side `SEVERITY.CRITICAL` / `SEVERITY.MAJOR` / … constants
/// that will be defined when scripting support lands (#130). A violated
/// `CRITICAL` precondition is intended to trigger immediate abort; lower
/// severities contribute to soft satisfaction grades.
pub mod severity {
    /// Violation → immediate abort.
    pub const CRITICAL: f32 = 1.0;
    pub const MAJOR: f32 = 0.7;
    pub const MODERATE: f32 = 0.5;
    pub const MINOR: f32 = 0.3;
    pub const TRIVIAL: f32 = 0.1;
}

/// Single precondition: a named boolean [`Condition`] with a severity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreconditionItem {
    pub name: Arc<str>,
    pub severity: f32,
    pub condition: Condition,
}

impl PreconditionItem {
    pub fn new(name: impl Into<Arc<str>>, severity: f32, condition: Condition) -> Self {
        Self {
            name: name.into(),
            severity: severity.clamp(0.0, 1.0),
            condition,
        }
    }
}

/// Ergonomic constructor matching the issue's factory signature.
pub fn precond(name: &str, severity: f32, condition: Condition) -> PreconditionItem {
    PreconditionItem::new(name, severity, condition)
}

/// A set of preconditions evaluated together.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PreconditionSet {
    pub items: Vec<PreconditionItem>,
}

impl PreconditionSet {
    pub fn new(items: Vec<PreconditionItem>) -> Self {
        Self { items }
    }

    pub fn with(mut self, item: PreconditionItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Evaluate every precondition and roll up the results into a
    /// [`PreconditionSummary`].
    pub fn evaluate(&self, ctx: &EvalContext) -> PreconditionSummary {
        let results = self.evaluate_detailed(ctx);
        PreconditionSummary::from_results(&results, ctx.now)
    }

    /// Evaluate every precondition and return the per-item results.
    pub fn evaluate_detailed(&self, ctx: &EvalContext) -> Vec<PreconditionEvalResult> {
        self.items
            .iter()
            .map(|item| PreconditionEvalResult {
                name: item.name.clone(),
                severity: item.severity,
                satisfied: item.condition.evaluate(ctx),
                evaluated_at: ctx.now,
            })
            .collect()
    }
}

/// Per-item evaluation result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreconditionEvalResult {
    pub name: Arc<str>,
    pub severity: f32,
    pub satisfied: bool,
    pub evaluated_at: Tick,
}

/// Aggregate summary over a [`PreconditionSet`] evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreconditionSummary {
    pub total: usize,
    pub satisfied: usize,
    /// `sum(satisfied[i] * severity[i]) / sum(severity[i])`. `1.0` if the
    /// set is empty or total severity is 0.
    pub weighted_satisfaction: f32,
    /// Names of preconditions with severity == [`severity::CRITICAL`] that
    /// are currently violated.
    pub critical_violations: Vec<Arc<str>>,
    pub evaluated_at: Tick,
}

impl PreconditionSummary {
    pub fn from_results(results: &[PreconditionEvalResult], now: Tick) -> Self {
        let total = results.len();
        let mut satisfied = 0usize;
        let mut weighted_num = 0.0f32;
        let mut weighted_den = 0.0f32;
        let mut critical_violations = Vec::new();
        for r in results {
            if r.satisfied {
                satisfied += 1;
                weighted_num += r.severity;
            }
            weighted_den += r.severity;
            if !r.satisfied && (r.severity - severity::CRITICAL).abs() < f32::EPSILON {
                critical_violations.push(r.name.clone());
            }
        }
        let weighted_satisfaction = if weighted_den > 0.0 {
            weighted_num / weighted_den
        } else {
            1.0
        };
        Self {
            total,
            satisfied,
            weighted_satisfaction,
            critical_violations,
            evaluated_at: now,
        }
    }

    pub fn has_critical_violation(&self) -> bool {
        !self.critical_violations.is_empty()
    }
}

/// History record for a single named precondition across evaluations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreconditionHistory {
    pub name: Arc<str>,
    pub severity: f32,
    /// Tick at which the precondition first became violated in the current
    /// violation run. `None` if the precondition is currently satisfied.
    pub violated_since: Option<Tick>,
    pub last_evaluation: PreconditionEvalResult,
}

/// Tracks the persistence of precondition violations across evaluations.
///
/// On each [`record`](Self::record) call, items that transition from
/// satisfied → violated start a `violated_since` timer; items that recover
/// (violated → satisfied) reset it. [`violated_for`](Self::violated_for)
/// returns how long (in ticks) a precondition has been in its current
/// violation run, if any.
#[derive(Debug, Default, Clone)]
pub struct PreconditionTracker {
    history: AHashMap<Arc<str>, PreconditionHistory>,
}

impl PreconditionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, results: &[PreconditionEvalResult], now: Tick) {
        for r in results {
            let entry = self
                .history
                .entry(r.name.clone())
                .or_insert_with(|| PreconditionHistory {
                    name: r.name.clone(),
                    severity: r.severity,
                    violated_since: None,
                    last_evaluation: r.clone(),
                });
            entry.severity = r.severity;
            entry.last_evaluation = r.clone();
            if r.satisfied {
                entry.violated_since = None;
            } else if entry.violated_since.is_none() {
                entry.violated_since = Some(now);
            }
        }
    }

    /// How long (in ticks) the named precondition has been continuously
    /// violated, or `None` if it is currently satisfied / unknown.
    pub fn violated_for(&self, name: &str, now: Tick) -> Option<Tick> {
        self.history
            .get(name)
            .and_then(|h| h.violated_since.map(|t| now - t))
    }

    /// Tick when the current violation run began, or `None`.
    pub fn violated_since(&self, name: &str) -> Option<Tick> {
        self.history.get(name).and_then(|h| h.violated_since)
    }

    /// Iterator over histories whose severity is [`severity::CRITICAL`] and
    /// are currently violated.
    pub fn critical_violations(&self) -> impl Iterator<Item = &PreconditionHistory> {
        self.history.values().filter(|h| {
            (h.severity - severity::CRITICAL).abs() < f32::EPSILON
                && h.violated_since.is_some()
        })
    }

    pub fn get(&self, name: &str) -> Option<&PreconditionHistory> {
        self.history.get(name)
    }

    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AiBus;
    use crate::ids::MetricId;
    use crate::retention::Retention;
    use crate::spec::MetricSpec;
    use crate::value_expr::{MetricRef, ValueExpr};
    use crate::warning::WarningMode;

    fn setup() -> (AiBus, MetricId) {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let id = MetricId::from("m");
        bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "m"));
        bus.emit(&id, 0.5, 0);
        (bus, id)
    }

    #[test]
    fn empty_set_is_fully_satisfied() {
        let (b, _) = setup();
        let ctx = EvalContext::new(&b, 0);
        let set = PreconditionSet::default();
        let s = set.evaluate(&ctx);
        assert_eq!(s.total, 0);
        assert_eq!(s.satisfied, 0);
        assert_eq!(s.weighted_satisfaction, 1.0);
        assert!(s.critical_violations.is_empty());
    }

    #[test]
    fn precondition_weighted_satisfaction() {
        let (b, id) = setup();
        let ctx = EvalContext::new(&b, 0);
        // high-severity: satisfied (metric present)
        // low-severity: violated (metric above 10)
        let set = PreconditionSet::new(vec![
            precond(
                "present",
                severity::MAJOR,
                Condition::Atom(crate::condition::ConditionAtom::MetricPresent {
                    metric: id.clone(),
                }),
            ),
            precond(
                "above_ten",
                severity::MINOR,
                Condition::gt(
                    ValueExpr::Metric(MetricRef::new(id)),
                    ValueExpr::Literal(10.0),
                ),
            ),
        ]);
        let s = set.evaluate(&ctx);
        assert_eq!(s.total, 2);
        assert_eq!(s.satisfied, 1);
        // weighted = 0.7 / (0.7 + 0.3) = 0.7
        assert!((s.weighted_satisfaction - 0.7).abs() < 1e-6);
    }

    #[test]
    fn critical_violation_surfaces_in_summary() {
        let (b, _) = setup();
        let ctx = EvalContext::new(&b, 10);
        let set = PreconditionSet::new(vec![
            precond("ok", severity::MAJOR, Condition::Always),
            precond("fatal", severity::CRITICAL, Condition::Never),
        ]);
        let s = set.evaluate(&ctx);
        assert_eq!(s.critical_violations.len(), 1);
        assert_eq!(&*s.critical_violations[0], "fatal");
        assert!(s.has_critical_violation());
    }

    #[test]
    fn tracker_violated_since_resets_on_recovery() {
        let (b, _) = setup();
        let set = PreconditionSet::new(vec![precond(
            "x",
            severity::CRITICAL,
            Condition::Never,
        )]);
        let mut tracker = PreconditionTracker::new();

        // Tick 10: violated, start of run
        let ctx = EvalContext::new(&b, 10);
        let r = set.evaluate_detailed(&ctx);
        tracker.record(&r, 10);
        assert_eq!(tracker.violated_for("x", 10), Some(0));

        // Tick 30: still violated — violated_for increases.
        let ctx = EvalContext::new(&b, 30);
        let r = set.evaluate_detailed(&ctx);
        tracker.record(&r, 30);
        assert_eq!(tracker.violated_for("x", 30), Some(20));

        // Tick 40: recovered.
        let set_ok = PreconditionSet::new(vec![precond(
            "x",
            severity::CRITICAL,
            Condition::Always,
        )]);
        let r = set_ok.evaluate_detailed(&EvalContext::new(&b, 40));
        tracker.record(&r, 40);
        assert_eq!(tracker.violated_for("x", 40), None);

        // Tick 50: violated again — new run starts now, not earlier.
        let r = set.evaluate_detailed(&EvalContext::new(&b, 50));
        tracker.record(&r, 50);
        assert_eq!(tracker.violated_for("x", 50), Some(0));
    }

    #[test]
    fn tracker_critical_violations_iter() {
        let (b, _) = setup();
        let set = PreconditionSet::new(vec![
            precond("critical", severity::CRITICAL, Condition::Never),
            precond("soft", severity::MINOR, Condition::Never),
            precond("ok", severity::CRITICAL, Condition::Always),
        ]);
        let mut tracker = PreconditionTracker::new();
        let r = set.evaluate_detailed(&EvalContext::new(&b, 0));
        tracker.record(&r, 0);
        let crits: Vec<_> = tracker.critical_violations().collect();
        assert_eq!(crits.len(), 1);
        assert_eq!(&*crits[0].name, "critical");
    }
}
