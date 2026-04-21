//! Confidence decay over the projection horizon.
//!
//! Confidence is the product of two terms:
//!
//! 1. **Time decay** — full strength within the effective strategic window,
//!    then an exponential half-life tail.
//! 2. **Intrinsic model confidence** — baked into the [`ProjectionModel`]
//!    itself (e.g. `r_squared` for Linear, fixed for Saturating).

use crate::time::Tick;

use super::ConfidenceDecay;

/// Shrink the base strategic window by observed volatility.
///
/// Formula: `effective = base · clamp(1 / (1 + α · vol), 0.25, 1.0)`.
///
/// `alpha` is [`TrajectoryConfig::volatility_penalty`](
/// crate::projection::TrajectoryConfig::volatility_penalty). Volatility of
/// `0` leaves the window unchanged; high volatility compresses it by up
/// to 4× but never below 25% of the base window (guard against pathological
/// fit noise).
pub fn effective_strategic_window(base: Tick, volatility: f64, alpha: f64) -> Tick {
    let factor = (1.0 / (1.0 + alpha.max(0.0) * volatility.max(0.0))).clamp(0.25, 1.0);
    ((base as f64) * factor).round() as Tick
}

/// Confidence weight at a horizon offset `delta_t`.
///
/// `1.0` for `delta_t ≤ strategic_window`, then exponential decay with
/// [`ConfidenceDecay::half_life`], floored at [`ConfidenceDecay::floor`].
///
/// `effective_window` is usually the output of [`effective_strategic_window`].
pub fn confidence_at(delta_t: Tick, effective_window: Tick, decay: ConfidenceDecay) -> f32 {
    if delta_t <= effective_window {
        return 1.0;
    }
    let over = (delta_t - effective_window).max(0) as f64;
    let half_life = decay.half_life.max(1) as f64;
    let raw = (-std::f64::consts::LN_2 * over / half_life).exp() as f32;
    raw.max(decay.floor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_at_within_window_is_one() {
        let d = ConfidenceDecay::default();
        assert_eq!(confidence_at(0, 60, d), 1.0);
        assert_eq!(confidence_at(30, 60, d), 1.0);
        assert_eq!(confidence_at(60, 60, d), 1.0);
    }

    #[test]
    fn confidence_at_half_life_is_half() {
        let d = ConfidenceDecay {
            strategic_window: 60,
            half_life: 80,
            floor: 0.0,
        };
        // window=60, delta=140 → over=80 → one half-life → 0.5
        let c = confidence_at(140, 60, d);
        assert!((c - 0.5).abs() < 1e-4);
    }

    #[test]
    fn confidence_at_floors() {
        let d = ConfidenceDecay {
            strategic_window: 60,
            half_life: 10,
            floor: 0.2,
        };
        // After many half-lives, raw ~0, floor dominates.
        let c = confidence_at(500, 60, d);
        assert!((c - 0.2).abs() < 1e-5);
    }

    #[test]
    fn effective_window_shrinks_under_volatility() {
        // base=60, α=2, vol=1 → 1/(1+2) = 0.333 → 20
        let w = effective_strategic_window(60, 1.0, 2.0);
        assert_eq!(w, 20);
    }

    #[test]
    fn effective_window_capped_below() {
        // Very high volatility cannot shrink below 25% (=15 of 60).
        let w = effective_strategic_window(60, 100.0, 2.0);
        assert_eq!(w, 15);
    }

    #[test]
    fn effective_window_zero_vol_unchanged() {
        let w = effective_strategic_window(60, 0.0, 2.0);
        assert_eq!(w, 60);
    }
}
