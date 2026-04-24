//! Victory condition — per-faction target describing what "winning"
//! means and what must remain true for winning to be possible.
//!
//! `VictoryCondition` is **pure data**: two `Condition` expressions
//! plus optional time limit / score hint. Long-term agents traverse
//! both `win` and `prerequisites` to generate intents — see
//! `docs/ai-three-layer.md` "ObjectiveDrivenLongTerm".
//!
//! # Semantics
//!
//! - `win` — **internal** faction-state condition. When True, the
//!   faction has won.
//! - `prerequisites` — **external** / environmental condition. Must
//!   continue to hold for the victory to remain reachable. When False
//!   (and assumed irrecoverable), the victory is `Unreachable`.
//! - `time_limit` — absolute tick after which `TimedOut` is returned
//!   if `win` has not yet become True.
//! - `score_hint` — optional progress expression `[0.0, 1.0]`, used
//!   by UI / tuning and fed into `VictoryStatus::Ongoing`. Purely
//!   informational — does not affect win/lose decisions.
//!
//! The deliberately-omitted `lose` condition: victory progress
//! trending down + prerequisites violation cover the same ground.
//! Game-level defeat (e.g. HP 0) is the game's responsibility, not
//! `macrocosmo-ai`'s.

use serde::{Deserialize, Serialize};

use crate::condition::Condition;
use crate::eval::EvalContext;
use crate::time::Tick;
use crate::value_expr::ValueExpr;

/// Victory status at a given tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VictoryStatus {
    /// `win` is currently True.
    Won,
    /// `prerequisites` failed — victory path is blocked.
    Unreachable,
    /// `time_limit` has been exceeded without winning.
    TimedOut,
    /// In progress. `progress` is derived from `score_hint` (or
    /// defaults to 0.0 when no hint is provided).
    Ongoing { progress: f32 },
}

impl VictoryStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            VictoryStatus::Won | VictoryStatus::Unreachable | VictoryStatus::TimedOut
        )
    }
}

/// Per-faction victory definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VictoryCondition {
    /// Internal faction-state condition; True when the faction has won.
    pub win: Condition,
    /// External / environmental condition; False means `Unreachable`.
    /// Long-term agents treat this as a pursuit target in addition to `win`.
    pub prerequisites: Condition,
    /// Optional tick after which `TimedOut` is reported if `win` is
    /// not yet True.
    pub time_limit: Option<Tick>,
    /// Optional `[0.0, 1.0]` expression for progress reporting.
    pub score_hint: Option<ValueExpr>,
}

impl VictoryCondition {
    /// Simple constructor with no time limit and no score hint.
    pub fn simple(win: Condition, prerequisites: Condition) -> Self {
        Self {
            win,
            prerequisites,
            time_limit: None,
            score_hint: None,
        }
    }

    /// Evaluate the current status. Check order:
    ///
    /// 1. `prerequisites` False → `Unreachable`
    /// 2. `win` True → `Won`
    /// 3. `time_limit` passed → `TimedOut`
    /// 4. otherwise → `Ongoing { progress }` (from `score_hint`, or 0.0)
    ///
    /// Prerequisites are checked first so a broken path cannot be
    /// retroactively "won" just because `win` also happened to flip
    /// True on the same tick.
    pub fn evaluate(&self, ctx: &EvalContext<'_>) -> VictoryStatus {
        if !self.prerequisites.evaluate(ctx) {
            return VictoryStatus::Unreachable;
        }
        if self.win.evaluate(ctx) {
            return VictoryStatus::Won;
        }
        if let Some(limit) = self.time_limit {
            if ctx.now >= limit {
                return VictoryStatus::TimedOut;
            }
        }
        let progress = self
            .score_hint
            .as_ref()
            .map(|e| e.evaluate(ctx) as f32)
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        VictoryStatus::Ongoing { progress }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AiBus;
    use crate::condition::{Condition, ConditionAtom};
    use crate::ids::MetricId;
    use crate::retention::Retention;
    use crate::spec::MetricSpec;
    use crate::warning::WarningMode;

    fn declare_and_emit(bus: &mut AiBus, metric: &str, value: f64, at: crate::time::Tick) {
        let id = MetricId::from(metric);
        bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Short, "test"));
        bus.emit(&id, value, at);
    }

    fn bus_with(metric: &str, value: f64) -> AiBus {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        declare_and_emit(&mut bus, metric, value, 10);
        bus
    }

    fn metric_above(metric: &str, threshold: f64) -> Condition {
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from(metric),
            threshold,
        })
    }

    #[test]
    fn unreachable_when_prerequisites_fail() {
        let bus = bus_with("stockpile_months", -1.0);
        let ctx = EvalContext::new(&bus, 10);
        let vc = VictoryCondition::simple(
            metric_above("econ", 100.0),
            metric_above("stockpile_months", 0.0),
        );
        assert_eq!(vc.evaluate(&ctx), VictoryStatus::Unreachable);
    }

    #[test]
    fn won_when_win_true_and_prereq_true() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        for (k, v) in [("econ", 150.0), ("stockpile_months", 5.0)] {
            declare_and_emit(&mut bus, k, v, 10);
        }
        let ctx = EvalContext::new(&bus, 10);
        let vc = VictoryCondition::simple(
            metric_above("econ", 100.0),
            metric_above("stockpile_months", 0.0),
        );
        assert_eq!(vc.evaluate(&ctx), VictoryStatus::Won);
    }

    #[test]
    fn timed_out_when_limit_passed_and_not_won() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        for (k, v) in [("econ", 20.0), ("stockpile_months", 5.0)] {
            declare_and_emit(&mut bus, k, v, 10);
        }
        let ctx = EvalContext::new(&bus, 200);
        let mut vc = VictoryCondition::simple(
            metric_above("econ", 100.0),
            metric_above("stockpile_months", 0.0),
        );
        vc.time_limit = Some(100);
        assert_eq!(vc.evaluate(&ctx), VictoryStatus::TimedOut);
    }

    #[test]
    fn ongoing_with_zero_progress_when_no_score_hint() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        for (k, v) in [("econ", 50.0), ("stockpile_months", 5.0)] {
            declare_and_emit(&mut bus, k, v, 10);
        }
        let ctx = EvalContext::new(&bus, 50);
        let vc = VictoryCondition::simple(
            metric_above("econ", 100.0),
            metric_above("stockpile_months", 0.0),
        );
        match vc.evaluate(&ctx) {
            VictoryStatus::Ongoing { progress } => assert_eq!(progress, 0.0),
            other => panic!("expected Ongoing, got {other:?}"),
        }
    }

    #[test]
    fn prereq_takes_precedence_over_win_on_same_tick() {
        // prerequisites False AND win True simultaneously → Unreachable
        // (broken path cannot be retroactively won)
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        for (k, v) in [("econ", 150.0), ("stockpile_months", -0.1)] {
            declare_and_emit(&mut bus, k, v, 10);
        }
        let ctx = EvalContext::new(&bus, 10);
        let vc = VictoryCondition::simple(
            metric_above("econ", 100.0),
            metric_above("stockpile_months", 0.0),
        );
        assert_eq!(vc.evaluate(&ctx), VictoryStatus::Unreachable);
    }

    #[test]
    fn unreachable_status_is_terminal() {
        assert!(VictoryStatus::Won.is_terminal());
        assert!(VictoryStatus::Unreachable.is_terminal());
        assert!(VictoryStatus::TimedOut.is_terminal());
        assert!(!VictoryStatus::Ongoing { progress: 0.5 }.is_terminal());
    }
}
