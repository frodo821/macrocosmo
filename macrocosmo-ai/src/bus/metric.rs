//! Metric topic internal storage.
//!
//! Each declared metric owns a `VecDeque<TimestampedValue>` ordered oldest-front.
//! Eviction happens on `push` only — we drop samples whose timestamps fall
//! outside the configured retention window.
//!
//! Emit semantics:
//! - Monotonic in time. If `at < last.at`, the sample is dropped (the bus
//!   emits a warning at the call site, not here).

use std::collections::VecDeque;

use crate::spec::MetricSpec;
use crate::time::{Tick, TimestampedValue};

#[derive(Debug)]
pub(crate) struct MetricStore {
    pub spec: MetricSpec,
    pub history: VecDeque<TimestampedValue>,
}

impl MetricStore {
    pub(crate) fn new(spec: MetricSpec) -> Self {
        Self {
            spec,
            history: VecDeque::new(),
        }
    }

    /// Returns `true` if the sample was accepted, `false` if it was dropped
    /// as a time-reversed emit. The caller is responsible for warning.
    pub(crate) fn push(&mut self, at: Tick, value: f64) -> bool {
        if let Some(last) = self.history.back() {
            if at < last.at {
                return false; // time-reversed — drop
            }
        }
        self.history.push_back(TimestampedValue { at, value });
        self.evict(at);
        true
    }

    fn evict(&mut self, newest: Tick) {
        let cutoff = newest.saturating_sub(self.spec.retention.as_ticks());
        while let Some(front) = self.history.front() {
            if front.at < cutoff {
                self.history.pop_front();
            } else {
                break;
            }
        }
    }

    pub(crate) fn current(&self) -> Option<f64> {
        self.history.back().map(|tv| tv.value)
    }

    /// Iterator over samples within `[now - duration, now]`, oldest-first.
    /// If the window extends beyond retention, the iterator simply yields
    /// everything still in history (partial data).
    pub(crate) fn window(
        &self,
        now: Tick,
        duration: Tick,
    ) -> impl Iterator<Item = &TimestampedValue> + '_ {
        let lower = now.saturating_sub(duration.max(0));
        self.history
            .iter()
            .filter(move |tv| tv.at >= lower && tv.at <= now)
    }

    /// Value at exactly `t`, if a sample with that timestamp exists.
    pub(crate) fn at(&self, t: Tick) -> Option<f64> {
        // History is small (retention-bounded) — linear scan is fine.
        self.history
            .iter()
            .rev()
            .find(|tv| tv.at == t)
            .map(|tv| tv.value)
    }

    /// Latest sample at-or-before `t`. Used by DelT to pick the historical
    /// anchor value when no exact match exists.
    pub(crate) fn at_or_before(&self, t: Tick) -> Option<f64> {
        self.history
            .iter()
            .rev()
            .find(|tv| tv.at <= t)
            .map(|tv| tv.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::Retention;

    fn store(retention: Retention) -> MetricStore {
        MetricStore::new(MetricSpec::gauge(retention, "test"))
    }

    #[test]
    fn push_in_order_appends() {
        let mut s = store(Retention::Custom(100));
        assert!(s.push(0, 1.0));
        assert!(s.push(10, 2.0));
        assert!(s.push(20, 3.0));
        assert_eq!(s.current(), Some(3.0));
        assert_eq!(s.history.len(), 3);
    }

    #[test]
    fn push_time_reversed_drops_and_returns_false() {
        let mut s = store(Retention::Custom(100));
        assert!(s.push(10, 1.0));
        assert!(!s.push(5, 2.0));
        assert_eq!(s.current(), Some(1.0));
        assert_eq!(s.history.len(), 1);
    }

    #[test]
    fn push_same_timestamp_accepted() {
        let mut s = store(Retention::Custom(100));
        assert!(s.push(10, 1.0));
        assert!(s.push(10, 2.0));
        assert_eq!(s.current(), Some(2.0));
        assert_eq!(s.history.len(), 2);
    }

    #[test]
    fn eviction_drops_samples_older_than_retention() {
        let mut s = store(Retention::Custom(100));
        s.push(0, 1.0);
        s.push(50, 2.0);
        s.push(150, 3.0); // cutoff = 50; samples with at < 50 evicted
        assert_eq!(s.history.len(), 2);
        assert_eq!(s.history.front().unwrap().at, 50);
    }

    #[test]
    fn window_returns_samples_in_range() {
        let mut s = store(Retention::Custom(1000));
        for t in 0..10 {
            s.push(t * 10, t as f64);
        }
        let got: Vec<_> = s.window(50, 20).map(|tv| tv.value).collect();
        assert_eq!(got, vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn window_wider_than_retention_returns_partial() {
        let mut s = store(Retention::Custom(50));
        for t in 0..10 {
            s.push(t * 10, t as f64);
        }
        // newest = 90, retention = 50 -> cutoff = 40 -> samples at {40..90} kept.
        let got: Vec<_> = s.window(90, 1000).map(|tv| tv.value).collect();
        assert_eq!(got, vec![4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
    }

    #[test]
    fn at_exact_match() {
        let mut s = store(Retention::Custom(1000));
        s.push(10, 1.5);
        s.push(20, 2.5);
        assert_eq!(s.at(20), Some(2.5));
        assert_eq!(s.at(15), None);
    }

    #[test]
    fn at_or_before_picks_latest_not_after() {
        let mut s = store(Retention::Custom(1000));
        s.push(10, 1.0);
        s.push(20, 2.0);
        s.push(30, 3.0);
        assert_eq!(s.at_or_before(25), Some(2.0));
        assert_eq!(s.at_or_before(30), Some(3.0));
        assert_eq!(s.at_or_before(5), None);
    }
}
