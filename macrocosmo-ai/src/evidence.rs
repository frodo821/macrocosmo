//! Evidence topic value types.
//!
//! `StandingEvidence` is a single datum supporting (or weakening) an observer's
//! opinion of a target faction. Evidence optionally decays exponentially over
//! time: `effective = magnitude * 0.5^((now - at) / halflife)`.

use serde::{Deserialize, Serialize};

use crate::ids::{EvidenceKindId, FactionId};
use crate::time::Tick;

/// A single piece of evidence influencing perceived standing between factions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandingEvidence {
    pub kind: EvidenceKindId,
    pub observer: FactionId,
    pub target: FactionId,
    /// Signed magnitude. Positive = friendly, negative = hostile. Unit is
    /// defined per-kind; #193 defines the interpretation.
    pub magnitude: f64,
    pub at: Tick,
    /// Optional exponential half-life in ticks. `None` = no decay.
    pub decay_halflife: Option<Tick>,
}

impl StandingEvidence {
    pub fn new(
        kind: EvidenceKindId,
        observer: FactionId,
        target: FactionId,
        magnitude: f64,
        at: Tick,
    ) -> Self {
        Self {
            kind,
            observer,
            target,
            magnitude,
            at,
            decay_halflife: None,
        }
    }

    pub fn with_halflife(mut self, halflife: Tick) -> Self {
        self.decay_halflife = Some(halflife);
        self
    }

    /// Effective magnitude at `now`, applying exponential decay if a half-life
    /// is set. If `now < at` (evidence from the future), returns the raw
    /// magnitude — the bus enforces monotonic emits so this only happens
    /// when querying an observation earlier than its observation time,
    /// which callers should not do.
    pub fn current_magnitude(&self, now: Tick) -> f64 {
        match self.decay_halflife {
            Some(hl) if hl > 0 => {
                let elapsed = (now - self.at).max(0) as f64;
                let factor = 0.5f64.powf(elapsed / hl as f64);
                self.magnitude * factor
            }
            _ => self.magnitude,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(magnitude: f64, at: Tick, halflife: Option<Tick>) -> StandingEvidence {
        let mut e = StandingEvidence::new(
            EvidenceKindId::from("test"),
            FactionId(1),
            FactionId(2),
            magnitude,
            at,
        );
        if let Some(hl) = halflife {
            e = e.with_halflife(hl);
        }
        e
    }

    #[test]
    fn no_halflife_no_decay() {
        let e = ev(10.0, 0, None);
        assert_eq!(e.current_magnitude(0), 10.0);
        assert_eq!(e.current_magnitude(1000), 10.0);
    }

    #[test]
    fn halflife_half_after_one_halflife() {
        let e = ev(10.0, 0, Some(100));
        let v = e.current_magnitude(100);
        assert!((v - 5.0).abs() < 1e-9, "expected 5.0, got {v}");
    }

    #[test]
    fn halflife_quarter_after_two_halflives() {
        let e = ev(10.0, 0, Some(100));
        let v = e.current_magnitude(200);
        assert!((v - 2.5).abs() < 1e-9, "expected 2.5, got {v}");
    }

    #[test]
    fn no_decay_for_negative_elapsed() {
        let e = ev(10.0, 100, Some(50));
        // Querying before observation — should not amplify.
        assert_eq!(e.current_magnitude(50), 10.0);
    }
}
