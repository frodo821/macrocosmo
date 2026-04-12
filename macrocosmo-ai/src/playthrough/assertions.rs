//! Property assertion helpers for playthroughs.
//!
//! Each helper returns `Result<(), String>` with a descriptive message on
//! failure so callers (typically tests) can attach it via `.unwrap()` /
//! `.expect()` / `assert!(...).is_ok()` as they see fit.
//!
//! `assert_no_panics` is a marker: if the run reached `finish` and produced
//! a playthrough at all, no panic occurred. It is exposed as a helper purely
//! for symmetry with the roadmap in #196.

use crate::bus::AiBus;
use crate::ids::{CommandKindId, MetricId};

use super::record::{Playthrough, PlaythroughEvent};

/// Direction of a monotonicity check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    NonDecreasing,
    NonIncreasing,
    StrictlyIncreasing,
    StrictlyDecreasing,
}

/// If the playthrough exists, nothing panicked during its production.
pub fn assert_no_panics(_pt: &Playthrough) { /* reaching end = ok */ }

/// Assert the number of recorded command events falls within `[min, max]`.
pub fn assert_command_count(pt: &Playthrough, min: usize, max: usize) -> Result<(), String> {
    let n = pt
        .events
        .iter()
        .filter(|e| matches!(e, PlaythroughEvent::Command(_)))
        .count();
    if n < min || n > max {
        return Err(format!(
            "command count {} outside [{}, {}]",
            n, min, max
        ));
    }
    Ok(())
}

/// Assert a metric's recorded values satisfy a monotonicity direction.
pub fn assert_metric_monotone(
    pt: &Playthrough,
    id: &MetricId,
    dir: Direction,
) -> Result<(), String> {
    let samples: Vec<f64> = pt
        .events
        .iter()
        .filter_map(|e| match e {
            PlaythroughEvent::Metric { id: m, value, .. } if m == id => Some(*value),
            _ => None,
        })
        .collect();

    if samples.is_empty() {
        return Err(format!("metric '{id}' has no samples in playthrough"));
    }

    for w in samples.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ok = match dir {
            Direction::NonDecreasing => b >= a,
            Direction::NonIncreasing => b <= a,
            Direction::StrictlyIncreasing => b > a,
            Direction::StrictlyDecreasing => b < a,
        };
        if !ok {
            return Err(format!(
                "metric '{id}' not {:?}: {} then {}",
                dir, a, b
            ));
        }
    }

    Ok(())
}

/// Assert a given command kind never appears in the event stream.
pub fn assert_no_command_kind(pt: &Playthrough, kind: &CommandKindId) -> Result<(), String> {
    for e in &pt.events {
        if let PlaythroughEvent::Command(sc) = e {
            if sc.kind == *kind {
                return Err(format!(
                    "forbidden command kind '{kind}' issued at tick {}",
                    sc.at
                ));
            }
        }
    }
    Ok(())
}

/// Assert two playthroughs are equivalent (same version, config, declarations
/// and event sequence).
pub fn assert_playthrough_equivalent(a: &Playthrough, b: &Playthrough) -> Result<(), String> {
    if a.version != b.version {
        return Err(format!(
            "version mismatch: {} vs {}",
            a.version, b.version
        ));
    }
    if a.declarations != b.declarations {
        return Err("declarations differ".into());
    }
    if a.events != b.events {
        return Err(format!(
            "event streams differ (len {} vs {})",
            a.events.len(),
            b.events.len()
        ));
    }
    if a.config != b.config {
        return Err("scenario configs differ".into());
    }
    Ok(())
}

/// Assert two buses hold equivalent state via their snapshots.
pub fn assert_bus_equivalent(a: &AiBus, b: &AiBus) -> Result<(), String> {
    let sa = a.snapshot();
    let sb = b.snapshot();
    if sa != sb {
        return Err("bus snapshots differ".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playthrough::record::{Declarations, PlaythroughMeta, ScenarioConfig};
    use crate::playthrough::scenario::SyntheticDynamics;
    use crate::playthrough::SUPPORTED_VERSION;

    fn pt_with_events(events: Vec<PlaythroughEvent>) -> Playthrough {
        Playthrough {
            version: SUPPORTED_VERSION,
            meta: PlaythroughMeta {
                name: "t".into(),
                seed: 0,
                ai_crate_version: "x".into(),
                duration_ticks: 0,
            },
            config: ScenarioConfig {
                name: "t".into(),
                seed: 0,
                duration_ticks: 0,
                factions: Vec::new(),
                dynamics: SyntheticDynamics::default(),
            },
            declarations: Declarations::default(),
            events,
        }
    }

    #[test]
    fn monotone_passes_on_increasing() {
        let m = MetricId::from("m");
        let events = (0..5)
            .map(|i| PlaythroughEvent::Metric {
                id: m.clone(),
                value: i as f64,
                at: i,
            })
            .collect();
        let pt = pt_with_events(events);
        assert!(assert_metric_monotone(&pt, &m, Direction::NonDecreasing).is_ok());
        assert!(assert_metric_monotone(&pt, &m, Direction::StrictlyIncreasing).is_ok());
        assert!(assert_metric_monotone(&pt, &m, Direction::NonIncreasing).is_err());
    }

    #[test]
    fn command_count_range() {
        use crate::command::SerializedCommand;
        use crate::ids::{CommandKindId, FactionId};
        let k = CommandKindId::from("k");
        let sc = SerializedCommand {
            kind: k,
            issuer: FactionId(1),
            target: None,
            params: Default::default(),
            at: 0,
            priority: 0.0,
        };
        let events = vec![PlaythroughEvent::Command(sc.clone()), PlaythroughEvent::Command(sc)];
        let pt = pt_with_events(events);
        assert!(assert_command_count(&pt, 1, 3).is_ok());
        assert!(assert_command_count(&pt, 3, 10).is_err());
    }
}
