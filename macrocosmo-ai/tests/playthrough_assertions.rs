//! Property assertion helper coverage.

use std::collections::BTreeMap;
use std::sync::Arc;

use macrocosmo_ai::playthrough::record::ScenarioConfig;
use macrocosmo_ai::playthrough::{
    Direction, MetricScript, Scenario, SyntheticDynamics, assert_bus_equivalent,
    assert_command_count, assert_metric_monotone, assert_no_command_kind, assert_no_panics,
    assert_playthrough_equivalent, replay, run_scenario,
};
use macrocosmo_ai::{
    AiBus, Command, CommandKindId, CommandSpec, FactionId, MetricId, MetricSpec, Retention,
    WarningMode,
};

fn trivial_config(name: &str) -> ScenarioConfig {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(
        MetricId::from("monotone_metric"),
        MetricScript::Monotone {
            from: 0.0,
            slope: 0.1,
        },
    );
    metric_scripts.insert(MetricId::from("flat_metric"), MetricScript::Constant(0.5));
    ScenarioConfig {
        name: name.into(),
        seed: 1,
        duration_ticks: 10,
        factions: vec![FactionId(1)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses: std::collections::BTreeMap::new(),
        },
    }
}

#[test]
fn no_panics_always_succeeds_after_run() {
    let pt = run_scenario(&Scenario::new(trivial_config("np")));
    assert_no_panics(&pt);
}

#[test]
fn metric_monotone_strict_passes_and_fails() {
    let pt = run_scenario(&Scenario::new(trivial_config("mm")));
    let m = MetricId::from("monotone_metric");
    assert!(assert_metric_monotone(&pt, &m, Direction::StrictlyIncreasing).is_ok());

    let flat = MetricId::from("flat_metric");
    // Constant is non-decreasing and non-increasing, but not strict.
    assert!(assert_metric_monotone(&pt, &flat, Direction::NonDecreasing).is_ok());
    assert!(assert_metric_monotone(&pt, &flat, Direction::NonIncreasing).is_ok());
    assert!(assert_metric_monotone(&pt, &flat, Direction::StrictlyIncreasing).is_err());
    assert!(assert_metric_monotone(&pt, &flat, Direction::StrictlyDecreasing).is_err());

    // Unknown metric => error.
    assert!(
        assert_metric_monotone(&pt, &MetricId::from("missing"), Direction::NonDecreasing).is_err()
    );
}

#[test]
fn command_count_passes_and_fails() {
    // No commands emitted by the default scenario.
    let pt = run_scenario(&Scenario::new(trivial_config("cc_none")));
    assert!(assert_command_count(&pt, 0, 0).is_ok());
    assert!(assert_command_count(&pt, 1, 3).is_err());

    // Scenario that emits 3 commands via tick_fn.
    let scenario = Scenario::new(trivial_config("cc_some")).with_tick_fn(Arc::new(|rb, t| {
        if t < 3 {
            let k = CommandKindId::from("cmd");
            rb.declare_command(k.clone(), CommandSpec::new("c"));
            rb.emit_command(Command::new(k, FactionId(1), t));
        }
    }));
    let pt2 = run_scenario(&scenario);
    assert!(assert_command_count(&pt2, 3, 3).is_ok());
    assert!(assert_command_count(&pt2, 0, 2).is_err());
}

#[test]
fn no_command_kind_passes_and_fails() {
    let k_forbidden = CommandKindId::from("forbidden");
    let k_ok = CommandKindId::from("ok");

    // Scenario emits only `ok` commands.
    let k_ok_clone = k_ok.clone();
    let scenario = Scenario::new(trivial_config("nck_ok")).with_tick_fn(Arc::new(move |rb, t| {
        if t == 0 {
            rb.declare_command(k_ok_clone.clone(), CommandSpec::new("c"));
        }
        if t < 2 {
            rb.emit_command(Command::new(k_ok_clone.clone(), FactionId(1), t));
        }
    }));
    let pt = run_scenario(&scenario);
    assert!(assert_no_command_kind(&pt, &k_forbidden).is_ok());
    assert!(assert_no_command_kind(&pt, &k_ok).is_err());
}

#[test]
fn playthrough_equivalent_detects_differences() {
    let pt1 = run_scenario(&Scenario::new(trivial_config("eq")));
    let pt2 = run_scenario(&Scenario::new(trivial_config("eq")));
    assert!(assert_playthrough_equivalent(&pt1, &pt2).is_ok());

    let pt3 = run_scenario(&Scenario::new(trivial_config("eq_different")));
    assert!(assert_playthrough_equivalent(&pt1, &pt3).is_err());
}

#[test]
fn bus_equivalent_detects_differences() {
    let pt1 = run_scenario(&Scenario::new(trivial_config("be")));
    let pt2 = run_scenario(&Scenario::new(trivial_config("be")));
    let bus_a = replay(&pt1).unwrap();
    let bus_b = replay(&pt2).unwrap();
    assert!(assert_bus_equivalent(&bus_a, &bus_b).is_ok());

    // Diverging bus.
    let mut bus_c = AiBus::with_warning_mode(WarningMode::Silent);
    bus_c.declare_metric(
        MetricId::from("ghost"),
        MetricSpec::gauge(Retention::Short, "x"),
    );
    assert!(assert_bus_equivalent(&bus_a, &bus_c).is_err());
}
