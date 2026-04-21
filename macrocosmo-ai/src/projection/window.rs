//! Strategic Window detection over a set of projected trajectories.
//!
//! A [`StrategicWindow`] is an inter-metric pattern that suggests a
//! time-bounded strategic opportunity (or risk). This module implements the
//! four detection kinds from issue #191 in a form reduced to pure
//! [`Trajectory`] inputs — no game state:
//!
//! - **Offensive**: the gap `mine - theirs` is positive now and monotonically
//!   shrinking until it hits zero (the enemy catches up).
//! - **Defensive**: the gap is already negative and widening (we fall
//!   further behind).
//! - **Growth**: a metric exhibits a sustained positive slope run —
//!   compounding investment is paying off.
//! - **ThresholdRace**: a specific metric crosses a configured threshold at
//!   some future tick — the "first-past-the-post" race metric.
//!
//! Intensity / confidence blend fit quality with gap magnitude. Windows
//! below [`WindowDetectionConfig::min_intensity`] are filtered out.

use ahash::AHashMap;
use serde::{Deserialize, Serialize};

use crate::ids::MetricId;
use crate::time::Tick;

use super::Trajectory;

/// Pair of metrics representing "mine" vs "theirs" for comparative windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricPair {
    pub mine: MetricId,
    pub theirs: MetricId,
}

/// Threshold to watch for a ThresholdRace window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThresholdGate {
    pub metric: MetricId,
    pub threshold: f64,
}

/// Configuration for a [`detect_windows`] run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowDetectionConfig {
    /// Windows weaker than this intensity are dropped. Default: `0.2`.
    pub min_intensity: f32,
    /// Minimum number of consecutive strictly-positive-slope samples for
    /// a [`WindowKind::Growth`] detection. Default: `3`.
    pub growth_monotone_span: Tick,
    /// Threshold gates to test for [`WindowKind::ThresholdRace`].
    pub threshold_gates: Vec<ThresholdGate>,
    /// Mine-vs-theirs pairs for Offensive / Defensive windows.
    pub pairs: Vec<MetricPair>,
}

impl Default for WindowDetectionConfig {
    fn default() -> Self {
        Self {
            min_intensity: 0.2,
            growth_monotone_span: 3,
            threshold_gates: Vec::new(),
            pairs: Vec::new(),
        }
    }
}

/// Kinds of Strategic Window this module detects.
///
/// Carries the involved metric ids so downstream code (Intent issuance,
/// UI, tests) can attribute the window to specific topics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WindowKind {
    /// We lead and the lead is shrinking — attack before the gap closes.
    Offensive { mine: MetricId, theirs: MetricId },
    /// We lag and the gap is widening — must be addressed before crossover.
    Defensive {
        mine: MetricId,
        theirs: MetricId,
        crossover_at: Tick,
    },
    /// Sustained compounding growth run on a single metric.
    Growth {
        metric: MetricId,
        compound_return: f32,
    },
    /// Metric crosses a configured threshold at a known tick.
    ThresholdRace {
        metric: MetricId,
        threshold: f64,
        reached_at: Tick,
    },
}

/// Why the window opened. Phase-1 rationales map 1:1 with [`WindowKind`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WindowRationale {
    Peaking { metric: MetricId },
    GapClosing { gap_now: f64, gap_at_close: f64 },
    CompoundEffect { at: Tick },
    ThresholdCrossing { at: Tick },
}

/// A detected strategic window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategicWindow {
    pub kind: WindowKind,
    pub opens_at: Tick,
    pub peak_at: Tick,
    pub closes_at: Tick,
    pub intensity: f32,
    pub confidence: f32,
    pub rationale: WindowRationale,
}

/// Detect all [`StrategicWindow`]s over a set of projected trajectories.
///
/// Windows whose intensity falls below [`WindowDetectionConfig::min_intensity`]
/// are filtered. The result is sorted by intensity (descending).
pub fn detect_windows(
    trajectories: &AHashMap<MetricId, Trajectory>,
    now: Tick,
    config: &WindowDetectionConfig,
) -> Vec<StrategicWindow> {
    let mut out = Vec::new();

    // Offensive / Defensive from metric pairs.
    for pair in &config.pairs {
        let (Some(mine), Some(theirs)) =
            (trajectories.get(&pair.mine), trajectories.get(&pair.theirs))
        else {
            continue;
        };
        out.extend(detect_pair(pair, mine, theirs, now));
    }

    // Growth from any metric with a strict monotone positive run.
    for (metric, tr) in trajectories.iter() {
        if let Some(w) = detect_growth(metric, tr, config.growth_monotone_span) {
            out.push(w);
        }
    }

    // Threshold race: first crossing for each configured gate.
    for gate in &config.threshold_gates {
        if let Some(tr) = trajectories.get(&gate.metric) {
            if let Some(w) = detect_threshold(&gate.metric, gate.threshold, tr) {
                out.push(w);
            }
        }
    }

    out.retain(|w| w.intensity >= config.min_intensity);
    out.sort_by(|a, b| {
        b.intensity
            .partial_cmp(&a.intensity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Build the `mine[i] - theirs[i]` series. Returns `None` when the shapes
/// don't line up (different sample counts) — the caller should skip the
/// pair.
fn gap_series(mine: &Trajectory, theirs: &Trajectory) -> Option<Vec<(Tick, f64, f32)>> {
    if mine.samples.len() != theirs.samples.len() {
        return None;
    }
    let out: Vec<_> = mine
        .samples
        .iter()
        .zip(theirs.samples.iter())
        .zip(mine.confidence.iter().zip(theirs.confidence.iter()))
        .map(|((a, b), (ca, cb))| (a.at, a.value - b.value, ca.min(*cb)))
        .collect();
    Some(out)
}

fn detect_pair(
    pair: &MetricPair,
    mine: &Trajectory,
    theirs: &Trajectory,
    now: Tick,
) -> Option<StrategicWindow> {
    let gaps = gap_series(mine, theirs)?;
    if gaps.is_empty() {
        return None;
    }
    let g0 = gaps[0].1;
    let g_n = gaps[gaps.len() - 1].1;

    if g0 > 0.0 && g_n <= g0 {
        // Offensive candidate: strictly-closing gap. Walk until gap hits 0.
        let mut closes_at = gaps[gaps.len() - 1].0;
        let mut strictly_closing = true;
        for win in gaps.windows(2) {
            if win[1].1 > win[0].1 {
                strictly_closing = false;
                break;
            }
            if win[1].1 <= 0.0 && win[0].1 > 0.0 {
                closes_at = win[1].0;
                break;
            }
        }
        if !strictly_closing {
            return None;
        }
        let intensity = normalise(g0, g0.abs().max(1.0));
        let confidence = gaps.iter().map(|(_, _, c)| *c).fold(1.0_f32, f32::min);
        return Some(StrategicWindow {
            kind: WindowKind::Offensive {
                mine: pair.mine.clone(),
                theirs: pair.theirs.clone(),
            },
            opens_at: now,
            peak_at: now,
            closes_at,
            intensity,
            confidence,
            rationale: WindowRationale::GapClosing {
                gap_now: g0,
                gap_at_close: g_n,
            },
        });
    }

    if g0 < 0.0 && g_n < g0 {
        // Defensive: already behind and falling further behind. Crossover
        // is the point where gap would hit a "critical" threshold. With no
        // such threshold baked in, report `closes_at` = last sample.
        let closes_at = gaps[gaps.len() - 1].0;
        let crossover_at = closes_at; // Phase-1 proxy.
        let intensity = normalise(-g0, g0.abs().max(1.0));
        let confidence = gaps.iter().map(|(_, _, c)| *c).fold(1.0_f32, f32::min);
        return Some(StrategicWindow {
            kind: WindowKind::Defensive {
                mine: pair.mine.clone(),
                theirs: pair.theirs.clone(),
                crossover_at,
            },
            opens_at: now,
            peak_at: now,
            closes_at,
            intensity,
            confidence,
            rationale: WindowRationale::GapClosing {
                gap_now: g0,
                gap_at_close: g_n,
            },
        });
    }

    None
}

fn detect_growth(metric: &MetricId, tr: &Trajectory, min_span: Tick) -> Option<StrategicWindow> {
    if tr.samples.len() < 2 {
        return None;
    }
    // Find the longest strict-monotone-increasing run at the start.
    let mut count = 0usize;
    for win in tr.samples.windows(2) {
        if win[1].value > win[0].value {
            count += 1;
        } else {
            break;
        }
    }
    if (count as Tick) < min_span {
        return None;
    }

    let first = &tr.samples[0];
    let last = &tr.samples[count];
    let growth = if first.value.abs() < f64::EPSILON {
        0.0
    } else {
        ((last.value - first.value) / first.value).max(0.0) as f32
    };

    let confidence = tr
        .confidence
        .iter()
        .take(count + 1)
        .copied()
        .fold(1.0_f32, f32::min);

    Some(StrategicWindow {
        kind: WindowKind::Growth {
            metric: metric.clone(),
            compound_return: growth,
        },
        opens_at: first.at,
        peak_at: last.at,
        closes_at: last.at,
        intensity: growth.clamp(0.0, 1.0),
        confidence,
        rationale: WindowRationale::Peaking {
            metric: metric.clone(),
        },
    })
}

fn detect_threshold(metric: &MetricId, threshold: f64, tr: &Trajectory) -> Option<StrategicWindow> {
    if tr.samples.is_empty() {
        return None;
    }
    let starting_above = tr.samples[0].value >= threshold;
    for (i, s) in tr.samples.iter().enumerate().skip(1) {
        let now_above = s.value >= threshold;
        if now_above != starting_above {
            // Crossed.
            let confidence = tr.confidence.get(i).copied().unwrap_or(1.0);
            let intensity = confidence; // threshold race intensity is confidence-driven
            return Some(StrategicWindow {
                kind: WindowKind::ThresholdRace {
                    metric: metric.clone(),
                    threshold,
                    reached_at: s.at,
                },
                opens_at: tr.samples[0].at,
                peak_at: s.at,
                closes_at: s.at,
                intensity,
                confidence,
                rationale: WindowRationale::ThresholdCrossing { at: s.at },
            });
        }
    }
    None
}

/// Map an unbounded magnitude to `[0, 1]` via `x / (x + ref)`. Soft and
/// monotone.
fn normalise(value: f64, reference: f64) -> f32 {
    let v = value.abs();
    let r = reference.abs().max(f64::EPSILON);
    let scaled = v / (v + r);
    scaled.clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::model::ProjectionModel;
    use crate::time::TimestampedValue;

    fn tr(points: &[(Tick, f64)]) -> Trajectory {
        Trajectory {
            samples: points
                .iter()
                .map(|(t, v)| TimestampedValue::new(*t, *v))
                .collect(),
            confidence: vec![1.0; points.len()],
            model: ProjectionModel::Linear {
                slope: 0.0,
                intercept: 0.0,
                r_squared: 1.0,
            },
        }
    }

    #[test]
    fn detect_offensive_on_closing_gap() {
        let mine = tr(&[(0, 10.0), (5, 8.0), (10, 4.0), (15, 0.0)]);
        let theirs = tr(&[(0, 0.0), (5, 2.0), (10, 3.0), (15, 4.0)]);
        let mut trajectories = AHashMap::new();
        trajectories.insert(MetricId::from("mine"), mine);
        trajectories.insert(MetricId::from("theirs"), theirs);
        let cfg = WindowDetectionConfig {
            pairs: vec![MetricPair {
                mine: MetricId::from("mine"),
                theirs: MetricId::from("theirs"),
            }],
            ..Default::default()
        };
        let ws = detect_windows(&trajectories, 0, &cfg);
        assert!(
            ws.iter()
                .any(|w| matches!(w.kind, WindowKind::Offensive { .. })),
            "no offensive window detected: {ws:?}"
        );
    }

    #[test]
    fn detect_defensive_on_widening_deficit() {
        let mine = tr(&[(0, 0.0), (5, 0.0), (10, 0.0), (15, 0.0)]);
        let theirs = tr(&[(0, 1.0), (5, 3.0), (10, 6.0), (15, 10.0)]);
        let mut trajectories = AHashMap::new();
        trajectories.insert(MetricId::from("mine"), mine);
        trajectories.insert(MetricId::from("theirs"), theirs);
        let cfg = WindowDetectionConfig {
            pairs: vec![MetricPair {
                mine: MetricId::from("mine"),
                theirs: MetricId::from("theirs"),
            }],
            ..Default::default()
        };
        let ws = detect_windows(&trajectories, 0, &cfg);
        assert!(
            ws.iter()
                .any(|w| matches!(w.kind, WindowKind::Defensive { .. }))
        );
    }

    #[test]
    fn detect_growth_on_monotone_run() {
        let t = tr(&[(0, 1.0), (5, 1.5), (10, 2.2), (15, 3.0), (20, 4.0)]);
        let mut trajectories = AHashMap::new();
        trajectories.insert(MetricId::from("research"), t);
        let cfg = WindowDetectionConfig {
            min_intensity: 0.0,
            growth_monotone_span: 3,
            ..Default::default()
        };
        let ws = detect_windows(&trajectories, 0, &cfg);
        assert!(
            ws.iter()
                .any(|w| matches!(w.kind, WindowKind::Growth { .. }))
        );
    }

    #[test]
    fn detect_threshold_race_on_cross() {
        let t = tr(&[(0, 10.0), (5, 20.0), (10, 45.0), (15, 90.0)]);
        let mut trajectories = AHashMap::new();
        trajectories.insert(MetricId::from("m"), t);
        let cfg = WindowDetectionConfig {
            min_intensity: 0.0,
            threshold_gates: vec![ThresholdGate {
                metric: MetricId::from("m"),
                threshold: 50.0,
            }],
            ..Default::default()
        };
        let ws = detect_windows(&trajectories, 0, &cfg);
        assert!(
            ws.iter()
                .any(|w| matches!(&w.kind, WindowKind::ThresholdRace { reached_at, .. } if *reached_at == 15)),
            "no threshold race detected: {ws:?}"
        );
    }

    #[test]
    fn min_intensity_filters_weak() {
        let mine = tr(&[(0, 1.0), (5, 0.9), (10, 0.8)]);
        let theirs = tr(&[(0, 0.0), (5, 0.1), (10, 0.2)]);
        let mut trajectories = AHashMap::new();
        trajectories.insert(MetricId::from("mine"), mine);
        trajectories.insert(MetricId::from("theirs"), theirs);
        let cfg = WindowDetectionConfig {
            min_intensity: 0.99,
            pairs: vec![MetricPair {
                mine: MetricId::from("mine"),
                theirs: MetricId::from("theirs"),
            }],
            ..Default::default()
        };
        let ws = detect_windows(&trajectories, 0, &cfg);
        assert!(ws.is_empty(), "expected filter: got {ws:?}");
    }
}
