//! Evidence topic internal storage.
//!
//! Evidence is stored per-kind in an append-only `Vec`. On emit, entries older
//! than the retention window (relative to the newest sample) are evicted.
//! Query-time filtering by observer/target is O(n) on the per-kind vector;
//! this is acceptable given realistic evidence volumes in Phase 2.

use crate::evidence::StandingEvidence;
use crate::spec::EvidenceSpec;
use crate::time::Tick;

#[derive(Debug)]
pub(crate) struct EvidenceStore {
    pub(crate) spec: EvidenceSpec,
    pub(crate) entries: Vec<StandingEvidence>,
    /// Monotonic counter bumped on every accepted push.
    pub(crate) version: u64,
}

impl EvidenceStore {
    pub(crate) fn new(spec: EvidenceSpec) -> Self {
        Self {
            spec,
            entries: Vec::new(),
            version: 0,
        }
    }

    /// Returns `true` if accepted, `false` if dropped due to time-reversed emit.
    pub(crate) fn push(&mut self, ev: StandingEvidence) -> bool {
        if let Some(last) = self.entries.last() {
            if ev.at < last.at {
                return false;
            }
        }
        let newest = ev.at;
        self.entries.push(ev);
        self.evict(newest);
        self.version = self.version.wrapping_add(1);
        true
    }

    fn evict(&mut self, newest: Tick) {
        let cutoff = newest.saturating_sub(self.spec.retention.as_ticks());
        // Entries are append-only in time order; find the first entry >= cutoff.
        // Using partition_point on at keeps this O(log n).
        let drop_upto = self.entries.partition_point(|e| e.at < cutoff);
        if drop_upto > 0 {
            self.entries.drain(..drop_upto);
        }
    }

    /// Iterate entries in the window `[now - duration, now]`.
    pub(crate) fn window(
        &self,
        now: Tick,
        duration: Tick,
    ) -> impl Iterator<Item = &StandingEvidence> + '_ {
        let lower = now.saturating_sub(duration.max(0));
        self.entries
            .iter()
            .filter(move |e| e.at >= lower && e.at <= now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EvidenceKindId, FactionId};
    use crate::retention::Retention;

    fn spec(retention: Retention) -> EvidenceSpec {
        EvidenceSpec::new(retention, "test")
    }

    fn ev(observer: u32, target: u32, at: Tick) -> StandingEvidence {
        StandingEvidence::new(
            EvidenceKindId::from("hostile_engagement"),
            FactionId(observer),
            FactionId(target),
            1.0,
            at,
        )
    }

    #[test]
    fn push_and_window() {
        let mut s = EvidenceStore::new(spec(Retention::Custom(1000)));
        s.push(ev(1, 2, 10));
        s.push(ev(1, 3, 20));
        s.push(ev(2, 1, 30));
        let got: Vec<_> = s.window(30, 20).collect();
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn eviction_drops_expired() {
        let mut s = EvidenceStore::new(spec(Retention::Custom(50)));
        s.push(ev(1, 2, 10));
        s.push(ev(1, 2, 20));
        s.push(ev(1, 2, 80));
        // newest=80, cutoff=30 -> entries at 10, 20 evicted.
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].at, 80);
    }

    #[test]
    fn time_reversed_push_dropped() {
        let mut s = EvidenceStore::new(spec(Retention::Custom(1000)));
        assert!(s.push(ev(1, 2, 50)));
        assert!(!s.push(ev(1, 2, 10)));
        assert_eq!(s.entries.len(), 1);
    }
}
