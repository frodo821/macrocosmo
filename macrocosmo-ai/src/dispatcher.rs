//! Intent dispatcher — routes `IntentSpec` to a materialized `Intent`.
//!
//! The dispatch step is where `macrocosmo-ai` hands off to
//! game-specific logic: delivery mechanism selection (courier /
//! relay / light-speed signal), resource consumption, relay-topology
//! awareness. See `docs/ai-three-layer.md` §dispatcher.
//!
//! # Contract
//!
//! A dispatcher receives an `IntentSpec` plus the sender's `FactionId`
//! and the current tick. It returns:
//!
//! - [`DispatchResult::Sent`] with the fully-materialized `Intent`
//!   (delay stamped into `arrives_at`). This is the normal case.
//! - [`DispatchResult::Deferred`] — truly nothing can be sent right now
//!   (e.g., total communication blackout). The orchestrator retries
//!   next tick. **Rare** — a well-written dispatcher almost always
//!   finds *some* slower mechanism.
//! - [`DispatchResult::Dropped`] — the target is unreachable (faction
//!   gone, destination destroyed). The orchestrator logs and forgets.
//!
//! # Smartness is the impl's problem
//!
//! A game-side dispatcher may internally compare FTL courier /
//! relay / light-speed signal, decide to build a courier ship on
//! the fly, consume resources, etc. The trait is `&mut self` +
//! single call — everything is the impl's responsibility.
//!
//! `macrocosmo-ai` ships only [`FixedDelayDispatcher`] for abstract
//! scenarios and tests.

use std::sync::Arc;

use crate::ids::{FactionId, IntentId};
use crate::intent::{Intent, IntentSpec};
use crate::time::Tick;

/// Result of a single dispatch attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum DispatchResult {
    /// Successfully materialized — delay stamped.
    Sent(Intent),
    /// Nothing can be sent right now. Orchestrator will retry next
    /// tick. Should be **rare** in well-tuned impls.
    Deferred,
    /// Truly undeliverable (target gone, etc.). Forget it.
    Dropped { reason: Arc<str> },
}

/// Dispatch an `IntentSpec` into an `Intent` (or defer / drop).
///
/// The orchestrator owns intent-id minting; impls receive an allocated
/// `IntentId` via the `id` parameter to avoid conflicts when multiple
/// dispatchers might share an id space.
pub trait IntentDispatcher {
    fn dispatch(
        &mut self,
        id: IntentId,
        spec: IntentSpec,
        issued_at: Tick,
        from: FactionId,
    ) -> DispatchResult;

    /// Pre-flight estimate of dispatch delay for `(spec, from)` without
    /// committing any resources. Used by AI agents to decide whether
    /// the intent's `expires_at_offset` window can accommodate the
    /// delivery time, so they can either widen the window or fall
    /// back to an alternative pursuit.
    ///
    /// Returns `None` when the dispatcher cannot estimate (== "ask
    /// at dispatch time"). Default impl returns `None` for backward
    /// compatibility — concrete dispatchers override.
    fn estimate_delay(&self, _spec: &IntentSpec, _from: FactionId) -> Option<Tick> {
        None
    }
}

/// Default dispatcher: stamps a fixed scalar delay. Always returns
/// `Sent` unless `drop_when_expiry_exceeded` is set (then returns
/// `Dropped` for intents whose `expires_at_offset` is too short to
/// accommodate the delay).
///
/// Useful both for abstract scenarios and as a building block in
/// game-side dispatchers that want to reuse the expiry comparison.
#[derive(Debug, Clone)]
pub struct FixedDelayDispatcher {
    pub delay: Tick,
    /// When `true`, intents whose `expires_at_offset < delay` are
    /// dropped pre-flight ("delivery would arrive after expiry =
    /// no point sending"). The drop's `reason` carries both numbers
    /// so AI agents can adapt. Default `false` keeps existing
    /// behavior (always Sent).
    pub drop_when_expiry_exceeded: bool,
}

impl FixedDelayDispatcher {
    pub fn new(delay: Tick) -> Self {
        Self {
            delay,
            drop_when_expiry_exceeded: false,
        }
    }

    pub fn zero_delay() -> Self {
        Self::new(0)
    }

    pub fn with_expiry_check(mut self, on: bool) -> Self {
        self.drop_when_expiry_exceeded = on;
        self
    }
}

impl IntentDispatcher for FixedDelayDispatcher {
    fn dispatch(
        &mut self,
        id: IntentId,
        spec: IntentSpec,
        issued_at: Tick,
        _from: FactionId,
    ) -> DispatchResult {
        if self.drop_when_expiry_exceeded {
            if let Some(expiry_offset) = spec.expires_at_offset {
                if self.delay > expiry_offset {
                    return DispatchResult::Dropped {
                        reason: Arc::from(format!(
                            "estimated delay {} > expiry offset {} (futile)",
                            self.delay, expiry_offset
                        )),
                    };
                }
            }
        }
        let arrives_at = issued_at + self.delay;
        let expires_at = spec.expires_at_offset.map(|o| issued_at + o);
        DispatchResult::Sent(Intent {
            id,
            spec,
            issued_at,
            arrives_at,
            expires_at,
        })
    }

    fn estimate_delay(&self, _spec: &IntentSpec, _from: FactionId) -> Option<Tick> {
        Some(self.delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::IntentSpec;

    fn spec() -> IntentSpec {
        IntentSpec::new("pursue", "faction")
    }

    #[test]
    fn fixed_delay_stamps_arrives_at() {
        let mut d = FixedDelayDispatcher::new(10);
        let result = d.dispatch(IntentId::from("intent_0"), spec(), 5, FactionId(0));
        match result {
            DispatchResult::Sent(intent) => {
                assert_eq!(intent.issued_at, 5);
                assert_eq!(intent.arrives_at, 15);
            }
            other => panic!("expected Sent, got {other:?}"),
        }
    }

    #[test]
    fn fixed_delay_converts_expires_offset_to_absolute() {
        let mut d = FixedDelayDispatcher::new(3);
        let mut s = spec();
        s.expires_at_offset = Some(20);
        let result = d.dispatch(IntentId::from("intent_0"), s, 100, FactionId(0));
        match result {
            DispatchResult::Sent(intent) => {
                assert_eq!(intent.expires_at, Some(120));
            }
            other => panic!("expected Sent, got {other:?}"),
        }
    }

    #[test]
    fn zero_delay_arrives_immediately() {
        let mut d = FixedDelayDispatcher::zero_delay();
        let result = d.dispatch(IntentId::from("intent_0"), spec(), 50, FactionId(0));
        match result {
            DispatchResult::Sent(intent) => {
                assert_eq!(intent.arrives_at, intent.issued_at);
                assert!(intent.has_arrived(50));
            }
            other => panic!("expected Sent, got {other:?}"),
        }
    }
}
