//! Statistical helpers for fitting [`ProjectionModel`]s to metric history.

use crate::time::{Tick, TimestampedValue};

/// Output of [`fit_linear`]: `y = slope * (t − ref_t) + intercept`.
///
/// `ref_t` is the anchor tick — typically `now`. `r_squared ∈ [0, 1]`
/// records the fraction of variance explained by the fit; it is `1.0` when
/// the points lie exactly on a line and `0.0` when all y values are equal
/// (degenerate fit) or the fit is meaningless.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearFit {
    pub slope: f64,
    pub intercept: f64,
    pub r_squared: f64,
    pub ref_t: Tick,
}

/// Least-squares linear fit of the given samples anchored at `ref_t`.
///
/// Returns `None` for fewer than two samples. With exactly two samples the
/// fit is exact (`r_squared = 1.0`). A degenerate (all-equal y, or
/// all-equal t) input yields `Some` with `slope = 0` and `r_squared = 0`.
pub fn fit_linear(samples: &[TimestampedValue], ref_t: Tick) -> Option<LinearFit> {
    if samples.len() < 2 {
        return None;
    }

    let n = samples.len() as f64;
    let mean_x: f64 = samples
        .iter()
        .map(|s| (s.at - ref_t) as f64)
        .sum::<f64>()
        / n;
    let mean_y: f64 = samples.iter().map(|s| s.value).sum::<f64>() / n;

    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syy = 0.0;
    for s in samples {
        let dx = (s.at - ref_t) as f64 - mean_x;
        let dy = s.value - mean_y;
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }

    // Degenerate — all samples share the same tick or values.
    if sxx < f64::EPSILON {
        return Some(LinearFit {
            slope: 0.0,
            intercept: mean_y,
            r_squared: 0.0,
            ref_t,
        });
    }

    let slope = sxy / sxx;
    let intercept = mean_y - slope * mean_x;
    let r_squared = if syy < f64::EPSILON {
        // All y equal. If the line we fit happens to be flat at that value
        // (slope ≈ 0), call that a "perfect" degenerate fit. The numeric
        // variance is 0, so r² is undefined mathematically — we return 1
        // so callers don't discard an otherwise-correct constant line.
        if slope.abs() < f64::EPSILON {
            1.0
        } else {
            0.0
        }
    } else {
        // Coefficient of determination via explained / total variance.
        let ss_res: f64 = samples
            .iter()
            .map(|s| {
                let pred = slope * ((s.at - ref_t) as f64) + intercept;
                (s.value - pred).powi(2)
            })
            .sum();
        (1.0 - ss_res / syy).clamp(0.0, 1.0)
    };

    Some(LinearFit {
        slope,
        intercept,
        r_squared,
        ref_t,
    })
}

/// Sample standard deviation of residuals from a linear fit.
///
/// This is our volatility proxy. Returns `0.0` when fewer than two samples
/// exist or when the fit is `None`. Higher values shrink the
/// effective strategic window.
pub fn volatility(samples: &[TimestampedValue], fit: Option<LinearFit>) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let Some(fit) = fit else {
        return 0.0;
    };

    let mean_y: f64 = samples.iter().map(|s| s.value).sum::<f64>() / samples.len() as f64;
    let total_variance: f64 = samples
        .iter()
        .map(|s| (s.value - mean_y).powi(2))
        .sum::<f64>()
        / samples.len() as f64;

    let residual_variance: f64 = samples
        .iter()
        .map(|s| {
            let dx = (s.at - fit.ref_t) as f64;
            let pred = fit.slope * dx + fit.intercept;
            (s.value - pred).powi(2)
        })
        .sum::<f64>()
        / samples.len() as f64;

    // Volatility = residual std-dev normalised by total std-dev so it is
    // scale-free. When residuals equal total variance (slope fits nothing)
    // this returns ~1.0; on a perfect line, 0.
    let total_sd = total_variance.sqrt();
    if total_sd < f64::EPSILON {
        0.0
    } else {
        (residual_variance.sqrt() / total_sd).clamp(0.0, 1.0)
    }
}

/// Heuristic saturation detector.
///
/// Given samples and a linear fit, check whether the tail residuals
/// systematically lie on one side of the line — a sign the signal is
/// approaching an asymptote. Returns `true` iff the last `tail` residuals
/// share a sign and their mean magnitude exceeds `threshold_fraction`
/// times the overall residual standard deviation.
pub fn detect_saturation(
    samples: &[TimestampedValue],
    fit: LinearFit,
    tail: usize,
    threshold_fraction: f64,
) -> bool {
    if samples.len() < tail + 2 {
        return false;
    }

    let residuals: Vec<f64> = samples
        .iter()
        .map(|s| {
            let dx = (s.at - fit.ref_t) as f64;
            let pred = fit.slope * dx + fit.intercept;
            s.value - pred
        })
        .collect();

    let rss: f64 = residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len() as f64;
    let rstd = rss.sqrt();
    if rstd < f64::EPSILON {
        return false;
    }

    let tail_slice = &residuals[residuals.len() - tail..];
    let first_sign = tail_slice[0].signum();
    if first_sign == 0.0 {
        return false;
    }
    let all_same_sign = tail_slice
        .iter()
        .all(|r| r.signum() == first_sign || *r == 0.0);
    if !all_same_sign {
        return false;
    }

    let mean_abs: f64 = tail_slice.iter().map(|r| r.abs()).sum::<f64>() / tail as f64;
    mean_abs > threshold_fraction * rstd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(at: Tick, value: f64) -> TimestampedValue {
        TimestampedValue::new(at, value)
    }

    #[test]
    fn fit_linear_requires_two_samples() {
        assert!(fit_linear(&[], 0).is_none());
        assert!(fit_linear(&[s(0, 1.0)], 0).is_none());
    }

    #[test]
    fn fit_linear_recovers_known_slope() {
        // y = 2*(t-10) + 5, ref=10
        let data: Vec<_> = (0..5).map(|i| s(10 + i * 5, 2.0 * (i * 5) as f64 + 5.0)).collect();
        let fit = fit_linear(&data, 10).unwrap();
        assert!((fit.slope - 2.0).abs() < 1e-9);
        assert!((fit.intercept - 5.0).abs() < 1e-9);
        assert!(fit.r_squared > 0.99);
    }

    #[test]
    fn fit_linear_r_squared_perfect_line() {
        let data = vec![s(0, 0.0), s(10, 10.0), s(20, 20.0)];
        let fit = fit_linear(&data, 0).unwrap();
        assert!(fit.r_squared > 0.999);
    }

    #[test]
    fn fit_linear_constant_series_is_flat() {
        let data = vec![s(0, 5.0), s(10, 5.0), s(20, 5.0)];
        let fit = fit_linear(&data, 0).unwrap();
        assert!(fit.slope.abs() < 1e-9);
        assert!((fit.intercept - 5.0).abs() < 1e-9);
        // Constant line — caller may treat r²=1 as "perfect flat fit".
        assert!((fit.r_squared - 1.0).abs() < 1e-9);
    }

    #[test]
    fn volatility_zero_on_line() {
        let data: Vec<_> = (0..5).map(|i| s(i * 5, i as f64 * 3.0)).collect();
        let fit = fit_linear(&data, 0);
        let v = volatility(&data, fit);
        assert!(v < 1e-6, "v = {v}");
    }

    #[test]
    fn volatility_positive_on_noise() {
        let data = vec![
            s(0, 0.0),
            s(5, 2.0),
            s(10, 1.0),
            s(15, 4.0),
            s(20, 3.0),
            s(25, 6.0),
        ];
        let fit = fit_linear(&data, 0);
        let v = volatility(&data, fit);
        assert!(v > 0.1, "expected non-trivial volatility, got {v}");
    }

    #[test]
    fn saturating_detects_logistic_tail() {
        // Logistic-ish curve: rapid rise that plateaus near 100.
        let data: Vec<TimestampedValue> = (0..10)
            .map(|i| {
                let t = (i * 5) as f64;
                let y = 100.0 * (1.0 - (-0.08 * t).exp());
                s(i * 5, y)
            })
            .collect();
        let fit = fit_linear(&data, 0).unwrap();
        // The tail (last 3 samples) all sit below the best-fit line — a
        // classic signature of approaching the asymptote.
        assert!(detect_saturation(&data, fit, 3, 0.2));
    }

    #[test]
    fn saturating_false_on_linear() {
        let data: Vec<_> = (0..8).map(|i| s(i * 5, i as f64 * 2.0)).collect();
        let fit = fit_linear(&data, 0).unwrap();
        assert!(!detect_saturation(&data, fit, 3, 0.2));
    }
}
