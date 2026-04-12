//! Per-topic version-based cache for [`Condition`] / [`PreconditionSet`]
//! evaluation.
//!
//! The cache is keyed by `(faction, fingerprint)` where `fingerprint` is a
//! content hash of the condition tree ([`Condition::fingerprint`]). Each
//! cache entry records the bus topic versions seen during the walk; a
//! subsequent lookup is a hit only if every recorded version still matches
//! [`AiBus::metric_version`] / [`AiBus::evidence_version`].
//!
//! Tick alone does NOT invalidate — an unchanged bus state will return the
//! cached answer even as `now` advances. This matches the design goal of
//! "evaluate only on change, not on clock". Tick-sensitive atoms
//! (`MetricStale`, `EvidenceRateAbove`) bypass this safely: their underlying
//! data is on the bus with a version counter, so the next emit naturally
//! bumps the cache key.
//!
//! # Collision safety
//!
//! Fingerprint is 64-bit; the probability of collision at realistic AI
//! eval volumes is negligible. If this ever becomes an issue, the
//! fingerprint can be upgraded to 128-bit without API changes.

use std::hash::Hasher;

use ahash::{AHashMap, AHasher};
use serde::{Deserialize, Serialize};

use crate::bus::AiBus;
use crate::condition::{CompareOp, Condition, ConditionAtom};
use crate::eval::EvalContext;
use crate::ids::{EvidenceKindId, FactionId, MetricId};
use crate::precondition::{PreconditionSet, PreconditionSummary};
use crate::time::Tick;
use crate::value_expr::{Dependencies, ValueExpr};

/// Composite cache key: faction + fingerprint. `faction` distinguishes
/// evaluations that share a tree but against different observers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    faction: Option<FactionId>,
    fingerprint: u64,
}

/// Snapshot of bus topic versions at the time a cache entry was recorded.
#[derive(Debug, Clone, Default)]
struct VersionSnapshot {
    metrics: Vec<(MetricId, u64)>,
    evidence: Vec<(EvidenceKindId, u64)>,
}

impl VersionSnapshot {
    fn capture(bus: &AiBus, deps: &Dependencies) -> Self {
        let mut metrics = Vec::with_capacity(deps.metrics.len());
        let mut seen_m = ahash::AHashSet::new();
        for m in &deps.metrics {
            if seen_m.insert(m.clone()) {
                metrics.push((m.clone(), bus.metric_version(m)));
            }
        }
        let mut evidence = Vec::with_capacity(deps.evidence.len());
        let mut seen_e = ahash::AHashSet::new();
        for e in &deps.evidence {
            if seen_e.insert(e.clone()) {
                evidence.push((e.clone(), bus.evidence_version(e)));
            }
        }
        Self { metrics, evidence }
    }

    fn is_fresh(&self, bus: &AiBus) -> bool {
        self.metrics
            .iter()
            .all(|(id, v)| bus.metric_version(id) == *v)
            && self
                .evidence
                .iter()
                .all(|(k, v)| bus.evidence_version(k) == *v)
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    value: bool,
    snapshot: VersionSnapshot,
    #[allow(dead_code)]
    recorded_at: Tick,
}

/// Registry that owns a precondition cache. Cached entries are keyed by
/// `(faction, fingerprint)` and invalidated by per-topic version
/// divergence.
#[derive(Debug, Default)]
pub struct PreconditionCacheRegistry {
    entries: AHashMap<CacheKey, CacheEntry>,
    stats: CacheStats,
}

/// Lightweight hit/miss counters for observability.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
}

impl PreconditionCacheRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluate `cond`, reusing a cached result when all dependency topic
    /// versions on the bus still match. Tick is not part of the key.
    pub fn evaluate(&mut self, cond: &Condition, ctx: &EvalContext) -> bool {
        let key = CacheKey {
            faction: ctx.faction,
            fingerprint: cond.fingerprint(),
        };

        if let Some(entry) = self.entries.get(&key) {
            if entry.snapshot.is_fresh(ctx.bus) {
                self.stats.hits += 1;
                return entry.value;
            }
        }

        self.stats.misses += 1;
        let mut deps = Dependencies::new();
        cond.collect_deps(&mut deps);
        deps.dedup();
        let snapshot = VersionSnapshot::capture(ctx.bus, &deps);
        let value = cond.evaluate(ctx);
        self.entries.insert(
            key,
            CacheEntry {
                value,
                snapshot,
                recorded_at: ctx.now,
            },
        );
        value
    }

    /// Evaluate an entire [`PreconditionSet`] with per-item caching.
    pub fn evaluate_set(
        &mut self,
        set: &PreconditionSet,
        ctx: &EvalContext,
    ) -> PreconditionSummary {
        use crate::precondition::PreconditionEvalResult;
        let results: Vec<PreconditionEvalResult> = set
            .items
            .iter()
            .map(|item| PreconditionEvalResult {
                name: item.name.clone(),
                severity: item.severity,
                satisfied: self.evaluate(&item.condition, ctx),
                evaluated_at: ctx.now,
            })
            .collect();
        PreconditionSummary::from_results(&results, ctx.now)
    }

    /// Drop every cached entry.
    pub fn invalidate_all(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }
}

// -------------------------------------------------------------------------
// Fingerprinting (content hash) for Condition / ValueExpr
// -------------------------------------------------------------------------

impl Condition {
    /// 64-bit content hash used as a cache key. Stable across clones and
    /// insensitive to ephemeral state — two structurally equal conditions
    /// always produce the same fingerprint.
    pub fn fingerprint(&self) -> u64 {
        let mut h = AHasher::default();
        self.hash_into(&mut h);
        h.finish()
    }

    fn hash_into(&self, h: &mut AHasher) {
        match self {
            Condition::Always => h.write_u8(0),
            Condition::Never => h.write_u8(1),
            Condition::All(cs) => {
                h.write_u8(2);
                h.write_usize(cs.len());
                for c in cs {
                    c.hash_into(h);
                }
            }
            Condition::Any(cs) => {
                h.write_u8(3);
                h.write_usize(cs.len());
                for c in cs {
                    c.hash_into(h);
                }
            }
            Condition::OneOf(cs) => {
                h.write_u8(4);
                h.write_usize(cs.len());
                for c in cs {
                    c.hash_into(h);
                }
            }
            Condition::Not(inner) => {
                h.write_u8(5);
                inner.hash_into(h);
            }
            Condition::Atom(a) => {
                h.write_u8(6);
                a.hash_into(h);
            }
        }
    }
}

impl ConditionAtom {
    fn hash_into(&self, h: &mut AHasher) {
        match self {
            ConditionAtom::MetricAbove { metric, threshold } => {
                h.write_u8(0);
                h.write(metric.as_str().as_bytes());
                h.write_u64(threshold.to_bits());
            }
            ConditionAtom::MetricBelow { metric, threshold } => {
                h.write_u8(1);
                h.write(metric.as_str().as_bytes());
                h.write_u64(threshold.to_bits());
            }
            ConditionAtom::MetricPresent { metric } => {
                h.write_u8(2);
                h.write(metric.as_str().as_bytes());
            }
            ConditionAtom::EvidenceCountExceeds {
                kind,
                window,
                threshold,
            } => {
                h.write_u8(3);
                h.write(kind.as_str().as_bytes());
                h.write_i64(*window);
                h.write_usize(*threshold);
            }
            ConditionAtom::Compare { left, op, right } => {
                h.write_u8(4);
                left.hash_into(h);
                h.write_u8(op_tag(*op));
                right.hash_into(h);
            }
            ConditionAtom::ValueMissing(expr) => {
                h.write_u8(5);
                expr.hash_into(h);
            }
            ConditionAtom::MetricStale { metric, max_age } => {
                h.write_u8(6);
                h.write(metric.as_str().as_bytes());
                h.write_i64(*max_age);
            }
            ConditionAtom::EvidenceRateAbove {
                kind,
                window,
                rate_per_tick,
            } => {
                h.write_u8(7);
                h.write(kind.as_str().as_bytes());
                h.write_i64(*window);
                h.write_u64(rate_per_tick.to_bits());
            }
        }
    }
}

fn op_tag(op: CompareOp) -> u8 {
    match op {
        CompareOp::Eq => 0,
        CompareOp::NotEq => 1,
        CompareOp::Lt => 2,
        CompareOp::Le => 3,
        CompareOp::Gt => 4,
        CompareOp::Ge => 5,
    }
}

impl ValueExpr {
    fn hash_into(&self, h: &mut AHasher) {
        match self {
            ValueExpr::Literal(v) => {
                h.write_u8(0);
                h.write_u64(v.to_bits());
            }
            ValueExpr::Missing => h.write_u8(1),
            ValueExpr::Metric(m) => {
                h.write_u8(2);
                h.write(m.id.as_str().as_bytes());
            }
            ValueExpr::DelT { metric, window } => {
                h.write_u8(3);
                h.write(metric.id.as_str().as_bytes());
                h.write_i64(*window);
            }
            ValueExpr::Add(cs) => {
                h.write_u8(4);
                h.write_usize(cs.len());
                for c in cs {
                    c.hash_into(h);
                }
            }
            ValueExpr::Mul(cs) => {
                h.write_u8(5);
                h.write_usize(cs.len());
                for c in cs {
                    c.hash_into(h);
                }
            }
            ValueExpr::Sub(a, b) => {
                h.write_u8(6);
                a.hash_into(h);
                b.hash_into(h);
            }
            ValueExpr::Div { num, den } => {
                h.write_u8(7);
                num.hash_into(h);
                den.hash_into(h);
            }
            ValueExpr::Neg(inner) => {
                h.write_u8(8);
                inner.hash_into(h);
            }
            ValueExpr::Min(cs) => {
                h.write_u8(9);
                h.write_usize(cs.len());
                for c in cs {
                    c.hash_into(h);
                }
            }
            ValueExpr::Max(cs) => {
                h.write_u8(10);
                h.write_usize(cs.len());
                for c in cs {
                    c.hash_into(h);
                }
            }
            ValueExpr::Abs(inner) => {
                h.write_u8(11);
                inner.hash_into(h);
            }
            ValueExpr::Clamp { expr, lo, hi } => {
                h.write_u8(12);
                expr.hash_into(h);
                h.write_u64(lo.to_bits());
                h.write_u64(hi.to_bits());
            }
            ValueExpr::IfThenElse { cond, then_, else_ } => {
                h.write_u8(13);
                cond.hash_into(h);
                then_.hash_into(h);
                else_.hash_into(h);
            }
            ValueExpr::WindowAvg { metric, window } => {
                h.write_u8(14);
                h.write(metric.id.as_str().as_bytes());
                h.write_i64(*window);
            }
            ValueExpr::WindowMin { metric, window } => {
                h.write_u8(15);
                h.write(metric.id.as_str().as_bytes());
                h.write_i64(*window);
            }
            ValueExpr::WindowMax { metric, window } => {
                h.write_u8(16);
                h.write(metric.id.as_str().as_bytes());
                h.write_i64(*window);
            }
            ValueExpr::WindowSum { metric, window } => {
                h.write_u8(17);
                h.write(metric.id.as_str().as_bytes());
                h.write_i64(*window);
            }
            ValueExpr::WindowCount { metric, window } => {
                h.write_u8(18);
                h.write(metric.id.as_str().as_bytes());
                h.write_i64(*window);
            }
            ValueExpr::Custom(s) => {
                h.write_u8(19);
                h.write(s.0.as_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AiBus;
    use crate::ids::MetricId;
    use crate::precondition::{precond, severity, PreconditionSet};
    use crate::retention::Retention;
    use crate::spec::MetricSpec;
    use crate::value_expr::MetricRef;
    use crate::warning::WarningMode;

    fn bus() -> AiBus {
        AiBus::with_warning_mode(WarningMode::Silent)
    }

    fn setup_metric(b: &mut AiBus, id: &MetricId, v: f64, at: Tick) {
        if !b.has_metric(id) {
            b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "m"));
        }
        b.emit(id, v, at);
    }

    #[test]
    fn fingerprint_stable_across_clones() {
        let c = Condition::gt(
            ValueExpr::Metric(MetricRef::new(MetricId::from("m"))),
            ValueExpr::Literal(0.5),
        );
        let fp1 = c.fingerprint();
        let fp2 = c.clone().fingerprint();
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn fingerprint_differs_on_structural_change() {
        let a = Condition::gt(
            ValueExpr::Metric(MetricRef::new(MetricId::from("m"))),
            ValueExpr::Literal(0.5),
        );
        let b = Condition::gt(
            ValueExpr::Metric(MetricRef::new(MetricId::from("m"))),
            ValueExpr::Literal(0.6),
        );
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn cache_hit_when_versions_unchanged() {
        let mut b = bus();
        let id = MetricId::from("x");
        setup_metric(&mut b, &id, 0.5, 0);
        let mut reg = PreconditionCacheRegistry::new();
        let c = Condition::gt(
            ValueExpr::Metric(MetricRef::new(id.clone())),
            ValueExpr::Literal(0.0),
        );
        let ctx = EvalContext::new(&b, 10);
        assert!(reg.evaluate(&c, &ctx));
        let stats1 = reg.stats();
        assert_eq!(stats1.misses, 1);
        assert_eq!(stats1.hits, 0);

        // Re-evaluate at a later tick without touching the bus.
        let ctx2 = EvalContext::new(&b, 100);
        assert!(reg.evaluate(&c, &ctx2));
        let stats2 = reg.stats();
        assert_eq!(stats2.misses, 1);
        assert_eq!(stats2.hits, 1);
    }

    #[test]
    fn cache_miss_when_metric_reemit_bumps_version() {
        let mut b = bus();
        let id = MetricId::from("x");
        setup_metric(&mut b, &id, 0.1, 0);
        let mut reg = PreconditionCacheRegistry::new();
        let c = Condition::gt(
            ValueExpr::Metric(MetricRef::new(id.clone())),
            ValueExpr::Literal(1.0),
        );
        let ctx = EvalContext::new(&b, 10);
        assert!(!reg.evaluate(&c, &ctx));
        let before = reg.stats();
        assert_eq!(before.misses, 1);

        // Re-emit raises value above threshold → cache must miss.
        b.emit(&id, 5.0, 20);
        let ctx2 = EvalContext::new(&b, 20);
        assert!(reg.evaluate(&c, &ctx2));
        let after = reg.stats();
        assert_eq!(after.misses, 2);
        assert_eq!(after.hits, 0);
    }

    #[test]
    fn cache_ignores_tick_without_emit() {
        let mut b = bus();
        let id = MetricId::from("x");
        setup_metric(&mut b, &id, 0.5, 0);
        let mut reg = PreconditionCacheRegistry::new();
        let c = Condition::Atom(crate::condition::ConditionAtom::MetricPresent {
            metric: id,
        });
        for t in [10, 20, 30, 40, 100_000] {
            let ctx = EvalContext::new(&b, t);
            reg.evaluate(&c, &ctx);
        }
        let s = reg.stats();
        assert_eq!(s.misses, 1);
        assert_eq!(s.hits, 4);
    }

    #[test]
    fn invalidate_all_clears_cache() {
        let mut b = bus();
        let id = MetricId::from("x");
        setup_metric(&mut b, &id, 1.0, 0);
        let mut reg = PreconditionCacheRegistry::new();
        let c = Condition::Atom(crate::condition::ConditionAtom::MetricPresent {
            metric: id,
        });
        reg.evaluate(&c, &EvalContext::new(&b, 0));
        assert_eq!(reg.len(), 1);
        reg.invalidate_all();
        assert!(reg.is_empty());
    }

    #[test]
    fn evaluate_set_caches_per_item() {
        let mut b = bus();
        let id = MetricId::from("m");
        setup_metric(&mut b, &id, 1.0, 0);
        let mut reg = PreconditionCacheRegistry::new();
        let set = PreconditionSet::new(vec![
            precond(
                "a",
                severity::MAJOR,
                Condition::Atom(crate::condition::ConditionAtom::MetricPresent {
                    metric: id.clone(),
                }),
            ),
            precond(
                "b",
                severity::MINOR,
                Condition::gt(
                    ValueExpr::Metric(MetricRef::new(id.clone())),
                    ValueExpr::Literal(0.0),
                ),
            ),
        ]);
        let s1 = reg.evaluate_set(&set, &EvalContext::new(&b, 5));
        assert_eq!(s1.satisfied, 2);
        assert_eq!(reg.stats().misses, 2);

        let s2 = reg.evaluate_set(&set, &EvalContext::new(&b, 100));
        assert_eq!(s2.satisfied, 2);
        assert_eq!(reg.stats().hits, 2);
    }
}
