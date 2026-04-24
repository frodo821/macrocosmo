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
}

/// Default dispatcher: stamps a fixed scalar delay and always returns
/// `Sent`. Enough for abstract scenarios where there is no physical
/// geography.
#[derive(Debug, Clone)]
pub struct FixedDelayDispatcher {
    pub delay: Tick,
}

impl FixedDelayDispatcher {
    pub fn new(delay: Tick) -> Self {
        Self { delay }
    }

    pub fn zero_delay() -> Self {
        Self { delay: 0 }
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
