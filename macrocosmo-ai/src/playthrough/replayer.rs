//! Replay a `Playthrough` into an `AiBus`.
//!
//! Replay is deterministic: every declaration is re-declared, every event is
//! re-emitted in order. The resulting bus is equivalent (via `snapshot`) to
//! the bus state at the end of the original run.

use thiserror::Error;

use crate::bus::AiBus;
use crate::command::Command;
use crate::warning::WarningMode;

use super::record::{Playthrough, PlaythroughEvent, SUPPORTED_VERSION};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReplayError {
    #[error("unsupported playthrough schema version {0} (expected {1})")]
    UnsupportedVersion(u32, u32),
}

/// Replay a playthrough into a fresh bus. The bus is created in
/// `WarningMode::Silent` — we are re-applying events that the original bus
/// already accepted, so warnings would be noise.
pub fn replay(pt: &Playthrough) -> Result<AiBus, ReplayError> {
    if pt.version != SUPPORTED_VERSION {
        return Err(ReplayError::UnsupportedVersion(pt.version, SUPPORTED_VERSION));
    }

    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);

    for (id, spec) in &pt.declarations.metrics {
        bus.declare_metric(id.clone(), spec.clone());
    }
    for (kind, spec) in &pt.declarations.commands {
        bus.declare_command(kind.clone(), spec.clone());
    }
    for (kind, spec) in &pt.declarations.evidence {
        bus.declare_evidence(kind.clone(), spec.clone());
    }

    for event in &pt.events {
        match event {
            PlaythroughEvent::Metric { id, value, at } => bus.emit(id, *value, *at),
            PlaythroughEvent::Command(sc) => bus.emit_command(Command::from(sc.clone())),
            PlaythroughEvent::Evidence(ev) => bus.emit_evidence(ev.clone()),
        }
    }

    Ok(bus)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playthrough::record::{
        Declarations, PlaythroughMeta, ScenarioConfig,
    };
    use crate::playthrough::scenario::SyntheticDynamics;

    fn mk(version: u32) -> Playthrough {
        Playthrough {
            version,
            meta: PlaythroughMeta {
                name: "t".into(),
                seed: 0,
                ai_crate_version: env!("CARGO_PKG_VERSION").into(),
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
            events: Vec::new(),
        }
    }

    #[test]
    fn empty_playthrough_replays_ok() {
        let pt = mk(SUPPORTED_VERSION);
        let bus = replay(&pt).expect("replay");
        assert!(bus.snapshot().metrics.is_empty());
    }

    #[test]
    fn version_mismatch_errors() {
        let pt = mk(SUPPORTED_VERSION + 1);
        let err = replay(&pt).unwrap_err();
        assert_eq!(
            err,
            ReplayError::UnsupportedVersion(SUPPORTED_VERSION + 1, SUPPORTED_VERSION)
        );
    }
}
