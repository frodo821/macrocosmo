//! #232: Shared helper for gated build-time ticking.
//!
//! Background: construction / upgrade queues advance `build_time_remaining`
//! one hexadies at a time inside the per-tick inner loop. The naive
//! `build_time_remaining -= 1` runs every tick regardless of whether the
//! order made progress, so when a star system stockpile is empty the
//! timer keeps draining and eventually sinks below zero while the order
//! never completes (the completion check also requires the remaining
//! resource balances to be zero). The UI then displays a finished
//! countdown on a stalled order.
//!
//! This module centralizes the tick rule so all three sites (planet
//! building construction, planet building upgrade, system building
//! upgrade — and for consistency the system building construction loop
//! that shares the same shape) agree on when the clock is allowed to
//! advance.

/// Advance `remaining` by one hexadies iff the order either received
/// resources this tick (`transferred == true`) or no longer needs any
/// resources (`no_more_needed == true`). Returns whether the counter was
/// decremented — useful for tests and for any caller that wants to log
/// stalled progress.
///
/// When both flags are `false` the order is stalled (starved of resources)
/// and the timer must stay pinned. This matches the design intent that an
/// under-resourced build cannot complete just because time has passed.
pub(crate) fn maybe_tick_build_time(
    remaining: &mut i64,
    transferred: bool,
    no_more_needed: bool,
) -> bool {
    if transferred || no_more_needed {
        *remaining -= 1;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_when_resources_transferred() {
        let mut remaining = 10;
        let ticked = maybe_tick_build_time(&mut remaining, true, false);
        assert!(ticked);
        assert_eq!(remaining, 9);
    }

    #[test]
    fn ticks_when_no_more_needed() {
        // Zero-cost or already-paid orders: the only thing left is to
        // wait out the clock.
        let mut remaining = 5;
        let ticked = maybe_tick_build_time(&mut remaining, false, true);
        assert!(ticked);
        assert_eq!(remaining, 4);
    }

    #[test]
    fn ticks_when_both_true() {
        // Both: resources trickled in AND the order is now fully paid.
        let mut remaining = 3;
        let ticked = maybe_tick_build_time(&mut remaining, true, true);
        assert!(ticked);
        assert_eq!(remaining, 2);
    }

    #[test]
    fn does_not_tick_when_stalled() {
        // Starved tick: no transfer, resources still required.
        let mut remaining = 4;
        let ticked = maybe_tick_build_time(&mut remaining, false, false);
        assert!(!ticked);
        assert_eq!(remaining, 4);
    }

    #[test]
    fn stalled_counter_does_not_underflow_past_zero_without_progress() {
        // Simulates the buggy pre-#232 path: many stalled ticks in a
        // row must not drain the counter.
        let mut remaining = 2;
        for _ in 0..1000 {
            maybe_tick_build_time(&mut remaining, false, false);
        }
        assert_eq!(remaining, 2);
    }
}
