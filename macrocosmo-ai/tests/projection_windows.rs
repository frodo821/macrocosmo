//! Integration tests for `macrocosmo_ai::projection::window` — Strategic
//! Window detection over projected trajectories.
//!
//! We construct synthetic self-pairs (mine_a, mine_b) so the AI core can
//! be tested without referring to real faction metrics; end-to-end
//! coverage with foreign-faction slots lives in the macrocosmo crate.

use ahash::AHashMap;

use macrocosmo_ai::{
    AiBus, MetricId, MetricPair, MetricSpec, ProjectionFidelity, Retention, ThresholdGate,
    TrajectoryConfig, WarningMode, WindowDetectionConfig, WindowKind, detect_windows, project,
};

fn bus_with_two(a: &MetricId, b: &MetricId) -> AiBus {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    bus.declare_metric(a.clone(), MetricSpec::gauge(Retention::VeryLong, "a"));
    bus.declare_metric(b.clone(), MetricSpec::gauge(Retention::VeryLong, "b"));
    bus
}

fn cfg() -> TrajectoryConfig {
    TrajectoryConfig {
        horizon: 30,
        step: 5,
        history_window: 100,
        fidelity: ProjectionFidelity::Standard,
        ..Default::default()
    }
}

#[test]
fn detect_offensive_window_on_closing_gap() {
    // mine starts ahead but slope is lower than theirs → gap closes.
    let mine = MetricId::from("mine");
    let theirs = MetricId::from("theirs");
    let mut bus = bus_with_two(&mine, &theirs);
    // mine starts at 100, rises slowly (slope=0.2).
    // theirs starts at 10, rises fast (slope=2.0) — will overtake.
    for i in 0..4 {
        let at = i * 5;
        bus.emit(&mine, 100.0 + 0.2 * at as f64, at);
        bus.emit(&theirs, 10.0 + 2.0 * at as f64, at);
    }
    let trs = project(&bus, &[mine.clone(), theirs.clone()], &cfg(), 15, &[]);
    let config = WindowDetectionConfig {
        min_intensity: 0.0,
        pairs: vec![MetricPair {
            mine: mine.clone(),
            theirs: theirs.clone(),
        }],
        ..Default::default()
    };
    let ws = detect_windows(&trs, 15, &config);
    assert!(
        ws.iter()
            .any(|w| matches!(w.kind, WindowKind::Offensive { .. })),
        "no offensive window: {ws:?}"
    );
}

#[test]
fn detect_defensive_window_on_crossover() {
    // mine stays flat, theirs accelerates away → widening deficit.
    let mine = MetricId::from("mine");
    let theirs = MetricId::from("theirs");
    let mut bus = bus_with_two(&mine, &theirs);
    for i in 0..5 {
        let at = i * 5;
        bus.emit(&mine, 10.0, at);
        bus.emit(&theirs, 20.0 + 10.0 * at as f64, at);
    }
    let trs = project(&bus, &[mine.clone(), theirs.clone()], &cfg(), 20, &[]);
    let config = WindowDetectionConfig {
        min_intensity: 0.0,
        pairs: vec![MetricPair { mine, theirs }],
        ..Default::default()
    };
    let ws = detect_windows(&trs, 20, &config);
    assert!(
        ws.iter()
            .any(|w| matches!(w.kind, WindowKind::Defensive { .. })),
        "no defensive window: {ws:?}"
    );
}

#[test]
fn detect_growth_window_on_monotone_run() {
    let id = MetricId::from("growth");
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::VeryLong, "g"));
    for i in 0..5 {
        let at = i * 5;
        bus.emit(&id, 1.0 + 0.5 * at as f64, at);
    }
    let trs = project(&bus, &[id.clone()], &cfg(), 20, &[]);
    let config = WindowDetectionConfig {
        min_intensity: 0.0,
        growth_monotone_span: 3,
        ..Default::default()
    };
    let ws = detect_windows(&trs, 20, &config);
    assert!(
        ws.iter()
            .any(|w| matches!(w.kind, WindowKind::Growth { .. }))
    );
}

#[test]
fn detect_threshold_race_on_cross() {
    let id = MetricId::from("stockpile");
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::VeryLong, "s"));
    for i in 0..4 {
        let at = i * 5;
        bus.emit(&id, 10.0 + 5.0 * at as f64, at); // 10, 35, 60, 85
    }
    let trs = project(&bus, &[id.clone()], &cfg(), 15, &[]);
    let config = WindowDetectionConfig {
        min_intensity: 0.0,
        threshold_gates: vec![ThresholdGate {
            metric: id.clone(),
            threshold: 150.0,
        }],
        ..Default::default()
    };
    let ws = detect_windows(&trs, 15, &config);
    assert!(
        ws.iter()
            .any(|w| matches!(&w.kind, WindowKind::ThresholdRace { .. })),
        "no threshold race detected: {ws:?}"
    );
}

#[test]
fn window_min_intensity_filters_weak() {
    // Tiny gap that closes → offensive candidate with tiny intensity.
    let mine = MetricId::from("mine");
    let theirs = MetricId::from("theirs");
    let mut bus = bus_with_two(&mine, &theirs);
    for i in 0..4 {
        let at = i * 5;
        bus.emit(&mine, 1.0 + 0.01 * at as f64, at);
        bus.emit(&theirs, 0.0 + 0.02 * at as f64, at);
    }
    let trs = project(&bus, &[mine.clone(), theirs.clone()], &cfg(), 15, &[]);
    // Extremely high min_intensity filters the weak window.
    // growth_monotone_span is set very large to suppress the incidental
    // Growth detection on the tiny series.
    let config = WindowDetectionConfig {
        min_intensity: 0.99,
        growth_monotone_span: 1_000,
        pairs: vec![MetricPair { mine, theirs }],
        ..Default::default()
    };
    let ws = detect_windows(&trs, 15, &config);
    assert!(ws.is_empty(), "expected filter: got {ws:?}");
}

#[test]
fn detect_windows_empty_when_no_trajectories() {
    let trs: AHashMap<MetricId, _> = AHashMap::new();
    let ws = detect_windows(&trs, 0, &WindowDetectionConfig::default());
    assert!(ws.is_empty());
}
