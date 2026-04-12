//! Record / serialize / replay roundtrip tests for `Playthrough`.

use std::collections::BTreeMap;

use macrocosmo_ai::playthrough::{
    assert_bus_equivalent, replay, run_scenario, EvidencePulse, MetricScript, Scenario,
    SyntheticDynamics,
};
use macrocosmo_ai::playthrough::record::ScenarioConfig;
use macrocosmo_ai::{EvidenceKindId, FactionId, MetricId};

fn config(name: &str, seed: u64) -> ScenarioConfig {
    let mut metric_scripts = BTreeMap::new();
    // Use exactly-representable f64 values so serde_json round-trips without
    // ULP drift. (1.0/0.5/0.25 are all dyadic fractions.)
    metric_scripts.insert(
        MetricId::from("readiness"),
        MetricScript::Constant(0.5),
    );
    // Linear with from=0, to=16, duration=16 → values 0,1,2,...,16 (integers).
    metric_scripts.insert(
        MetricId::from("capacity"),
        MetricScript::Linear {
            from: 0.0,
            to: 16.0,
        },
    );
    metric_scripts.insert(
        MetricId::from("ratio"),
        MetricScript::Monotone {
            from: 1.0,
            slope: 0.125,
        },
    );

    let evidence_pulses = vec![
        EvidencePulse {
            kind: EvidenceKindId::from("incident"),
            observer: FactionId(1),
            target: FactionId(2),
            magnitude: 1.0,
            at: 5,
        },
        EvidencePulse {
            kind: EvidenceKindId::from("incident"),
            observer: FactionId(2),
            target: FactionId(1),
            magnitude: 2.0,
            at: 12,
        },
    ];

    ScenarioConfig {
        name: name.into(),
        seed,
        duration_ticks: 16,
        factions: vec![FactionId(1), FactionId(2)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses,
        },
    }
}

#[test]
fn record_replay_byte_identical() {
    let scenario = Scenario::new(config("roundtrip", 42));
    let pt = run_scenario(&scenario);

    // Serialize → deserialize → re-serialize. The re-serialized bytes must
    // match the original, which is the stable invariant JSON can guarantee
    // (round-tripping f64 as decimal can lose the last ULP, so we compare
    // canonical byte form rather than `PartialEq` on events).
    let bytes = serde_json::to_vec(&pt).expect("serialize");
    let pt2: macrocosmo_ai::playthrough::Playthrough =
        serde_json::from_slice(&bytes).expect("deserialize");
    let bytes2 = serde_json::to_vec(&pt2).expect("reserialize");
    if bytes != bytes2 {
        let s1 = String::from_utf8_lossy(&bytes);
        let s2 = String::from_utf8_lossy(&bytes2);
        // Find first diff index for a digestible error message.
        let diff_idx = s1
            .as_bytes()
            .iter()
            .zip(s2.as_bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(s1.len().min(s2.len()));
        let s1_snip = &s1[diff_idx.saturating_sub(40)..(diff_idx + 40).min(s1.len())];
        let s2_snip = &s2[diff_idx.saturating_sub(40)..(diff_idx + 40).min(s2.len())];
        panic!(
            "bytes differ at {}: ...{}... vs ...{}...",
            diff_idx, s1_snip, s2_snip
        );
    }

    // Structural-but-tolerant checks: version, declarations, event count,
    // and scenario config all survive the roundtrip exactly.
    assert_eq!(pt.version, pt2.version);
    assert_eq!(pt.declarations, pt2.declarations);
    assert_eq!(pt.config, pt2.config);
    assert_eq!(pt.events.len(), pt2.events.len());
}

#[test]
fn replay_equivalence() {
    // Run the scenario once, then replay the recorded playthrough into a
    // fresh bus. The two buses must produce identical snapshots.
    let scenario = Scenario::new(config("equivalence", 123));
    let pt = run_scenario(&scenario);

    let replayed = replay(&pt).expect("replay");

    // Produce an "original" bus by running the scenario a second time (no
    // custom tick_fn, so this is deterministic).
    let scenario2 = Scenario::new(config("equivalence", 123));
    let pt2 = run_scenario(&scenario2);
    let original = replay(&pt2).expect("replay pt2");

    assert_bus_equivalent(&original, &replayed).expect("bus equivalence");
}

#[test]
fn determinism_across_runs() {
    // Same scenario config + seed run twice must produce identical bytes.
    let a = run_scenario(&Scenario::new(config("d", 7)));
    let b = run_scenario(&Scenario::new(config("d", 7)));

    let ba = serde_json::to_vec(&a).expect("a");
    let bb = serde_json::to_vec(&b).expect("b");
    assert_eq!(ba, bb, "two runs must produce identical bytes");
}
