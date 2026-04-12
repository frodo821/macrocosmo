//! Perceived Standing — evidence-based standing inference (#193).
//!
//! A `PerceivedStanding` is a pure inference over evidence already on the bus
//! (`StandingEvidence`). It does not mutate bus state. The `compute` function
//! aggregates per-kind base weights, evidence half-life decay, personality
//! decay, and ambiguity/polarity biases into a single `inferred_standing`
//! value clamped to `[-1.0, 1.0]`, plus a `confidence` score.
//!
//! # Phase 1 scope
//!
//! Only `StandingSubject::ObserverSelf` is functionally meaningful. For
//! `Other(_)` / `World`, `compute` returns a neutral (`0.0`) standing with
//! zero-evidence confidence. The `subject` field on `StandingEvidence` is
//! future work (tracked under #193 follow-ups) — evidence on the bus today is
//! implicitly "how the observer feels about target", i.e. ObserverSelf.
//!
//! # Aggregation algorithm
//!
//! For each evidence entry `e` with `e.observer == observer && e.target == target`:
//!
//! 1. `raw = e.current_magnitude(now)` — existing exponential half-life decay.
//! 2. `kind_cfg = config.kinds.get(&e.kind).unwrap_or(&config.default_kind_config)`.
//! 3. `base = raw * kind_cfg.base_weight`.
//! 4. If `kind_cfg.ambiguous && interpretation_key.is_some()`, multiply by
//!    `1.0 + params.ai_param_f64(key, 0.0)` (ambiguity bias).
//! 5. Polarity bias:
//!    - Negative base: multiply by `1.0 + params.aggressiveness() * config.hostile_bias_factor`.
//!    - Positive base: multiply by `1.0 + params.defensive_bias() * config.friendly_bias_factor`.
//! 6. If `config.use_personality_decay`:
//!    - Negative contribution: scale by `exp(-params.grudge_persistence() * age)`.
//!    - Positive contribution: scale by `exp(-params.friendship_persistence() * age)`.
//! 7. Keep the contribution only if `|contribution| >= config.min_contribution`.
//! 8. `inferred_standing = sum_of_contributions.clamp(-1.0, 1.0)`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ai_params::AiParamsExt;
use crate::bus::AiBus;
use crate::ids::{EvidenceKindId, FactionId};
use crate::time::Tick;

/// Whose feelings the standing inference is about.
///
/// Phase 1 only implements `ObserverSelf`. The other two variants are
/// accepted for forward-compat and currently produce a neutral result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StandingSubject {
    /// "how target feels about observer".
    ObserverSelf,
    /// "how target feels about the given third-party faction".
    Other(FactionId),
    /// "target's general attitude toward the world".
    World,
}

/// Thresholds used by `StandingLevel::from_score`. Game declares values via
/// `StandingConfig`; sensible defaults are provided so tests work without
/// game-side setup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandingLevelThresholds {
    pub hostile_to_wary: f64,
    pub wary_to_neutral: f64,
    pub neutral_to_cordial: f64,
    pub cordial_to_friendly: f64,
    pub friendly_to_allied: f64,
}

impl Default for StandingLevelThresholds {
    fn default() -> Self {
        Self {
            hostile_to_wary: -0.6,
            wary_to_neutral: -0.2,
            neutral_to_cordial: 0.2,
            cordial_to_friendly: 0.5,
            friendly_to_allied: 0.8,
        }
    }
}

/// Coarse bucket over a standing score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StandingLevel {
    Hostile,
    Wary,
    Neutral,
    Cordial,
    Friendly,
    Allied,
}

impl StandingLevel {
    pub fn from_score(score: f64, thresholds: &StandingLevelThresholds) -> Self {
        if score < thresholds.hostile_to_wary {
            StandingLevel::Hostile
        } else if score < thresholds.wary_to_neutral {
            StandingLevel::Wary
        } else if score < thresholds.neutral_to_cordial {
            StandingLevel::Neutral
        } else if score < thresholds.cordial_to_friendly {
            StandingLevel::Cordial
        } else if score < thresholds.friendly_to_allied {
            StandingLevel::Friendly
        } else {
            StandingLevel::Allied
        }
    }
}

/// Per-kind interpretation config: base weight applied to the evidence's
/// magnitude, and optional ambiguity routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceKindConfig {
    /// Multiplier applied to `current_magnitude(now)`. Sign controls polarity
    /// (negative = hostile, positive = friendly).
    pub base_weight: f64,
    /// If `true`, the final contribution is scaled by
    /// `1.0 + params.ai_param_f64(interpretation_key, 0.0)` when an
    /// `interpretation_key` is set.
    pub ambiguous: bool,
    /// Optional AI-params key consulted for ambiguity scaling.
    pub interpretation_key: Option<String>,
}

impl Default for EvidenceKindConfig {
    fn default() -> Self {
        Self {
            base_weight: 0.0,
            ambiguous: false,
            interpretation_key: None,
        }
    }
}

/// Game-tunable aggregator config. The game fills `kinds` with per-kind base
/// weights; ai_core ships only neutral defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandingConfig {
    /// Per-evidence-kind interpretation config.
    pub kinds: HashMap<EvidenceKindId, EvidenceKindConfig>,
    /// Fallback used when an evidence kind is absent from `kinds`.
    pub default_kind_config: EvidenceKindConfig,
    /// Evidence count at which the count-component of confidence saturates.
    pub confidence_saturation_count: f64,
    /// Half-life (ticks) used in the freshness confidence term. Must be > 0.
    pub confidence_age_halflife: Tick,
    /// Optional window (ticks) — only evidence within `[now - lookback, now]`
    /// is considered. `None` = consider everything still on the bus.
    pub lookback: Option<Tick>,
    /// Contributions whose absolute value is below this threshold are
    /// dropped (both from the sum and from the breakdown).
    pub min_contribution: f64,
    /// Hostile-polarity bias factor. `contribution *= 1.0 + aggressiveness * factor`.
    pub hostile_bias_factor: f64,
    /// Friendly-polarity bias factor. `contribution *= 1.0 + defensive_bias * factor`.
    pub friendly_bias_factor: f64,
    /// Toggle the grudge/friendship-persistence secondary decay.
    pub use_personality_decay: bool,
    /// Thresholds for `StandingLevel::from_score`.
    pub level_thresholds: StandingLevelThresholds,
}

impl Default for StandingConfig {
    fn default() -> Self {
        Self {
            kinds: HashMap::new(),
            default_kind_config: EvidenceKindConfig::default(),
            confidence_saturation_count: 20.0,
            confidence_age_halflife: 100,
            lookback: None,
            min_contribution: 1e-9,
            hostile_bias_factor: 0.3,
            friendly_bias_factor: 0.3,
            use_personality_decay: true,
            level_thresholds: StandingLevelThresholds::default(),
        }
    }
}

/// Result of `compute` — a snapshot of the inferred standing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerceivedStanding {
    pub observer: FactionId,
    pub target: FactionId,
    pub subject: StandingSubject,
    /// Clamped to `[-1.0, 1.0]`.
    pub inferred_standing: f64,
    /// Clamped to `[0.0, 1.0]`.
    pub confidence: f64,
    pub evidence_count: usize,
    pub computed_at: Tick,
}

/// One evidence entry's signed contribution to the final standing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceContribution {
    pub kind: EvidenceKindId,
    pub signed_contribution: f64,
    pub at: Tick,
}

/// Pure inference. See module docs for the aggregation algorithm.
pub fn compute<P: AiParamsExt + ?Sized>(
    bus: &AiBus,
    observer: FactionId,
    target: FactionId,
    subject: StandingSubject,
    now: Tick,
    config: &StandingConfig,
    params: &P,
) -> PerceivedStanding {
    compute_with_breakdown(bus, observer, target, subject, now, config, params).0
}

/// Same as `compute`, but also returns a breakdown of surviving contributions,
/// sorted by descending absolute value (ties broken by original order).
pub fn compute_with_breakdown<P: AiParamsExt + ?Sized>(
    bus: &AiBus,
    observer: FactionId,
    target: FactionId,
    subject: StandingSubject,
    now: Tick,
    config: &StandingConfig,
    params: &P,
) -> (PerceivedStanding, Vec<EvidenceContribution>) {
    // Phase 1: only ObserverSelf is functional.
    if !matches!(subject, StandingSubject::ObserverSelf) {
        return (
            PerceivedStanding {
                observer,
                target,
                subject,
                inferred_standing: 0.0,
                confidence: 0.3,
                evidence_count: 0,
                computed_at: now,
            },
            Vec::new(),
        );
    }

    let lookback = config.lookback.unwrap_or(Tick::MAX);

    // Gather matching evidence.
    let mut matching: Vec<&crate::evidence::StandingEvidence> = bus
        .evidence_for(observer, now, lookback)
        .filter(|e| e.target == target)
        .collect();
    // evidence_for does not guarantee time ordering across kinds; sort for
    // deterministic breakdown when ties occur.
    matching.sort_by_key(|e| e.at);

    if matching.is_empty() {
        return (
            PerceivedStanding {
                observer,
                target,
                subject,
                inferred_standing: 0.0,
                confidence: 0.3,
                evidence_count: 0,
                computed_at: now,
            },
            Vec::new(),
        );
    }

    let mut contributions: Vec<EvidenceContribution> = Vec::with_capacity(matching.len());
    let mut sum = 0.0_f64;
    let mut freshness_accum = 0.0_f64;

    for e in &matching {
        let kind_cfg = config
            .kinds
            .get(&e.kind)
            .unwrap_or(&config.default_kind_config);

        let raw = e.current_magnitude(now);
        let mut contribution = raw * kind_cfg.base_weight;

        // Ambiguity interpretation bias.
        if kind_cfg.ambiguous
            && let Some(key) = kind_cfg.interpretation_key.as_deref()
        {
            contribution *= 1.0 + params.ai_param_f64(key, 0.0);
        }

        // Polarity bias.
        if contribution < 0.0 {
            contribution *= 1.0 + params.aggressiveness() * config.hostile_bias_factor;
        } else if contribution > 0.0 {
            contribution *= 1.0 + params.defensive_bias() * config.friendly_bias_factor;
        }

        // Personality (grudge / friendship) secondary decay.
        if config.use_personality_decay {
            let age = (now - e.at).max(0) as f64;
            let rate = if contribution < 0.0 {
                params.grudge_persistence()
            } else {
                params.friendship_persistence()
            };
            contribution *= (-rate * age).exp();
        }

        // Freshness component of confidence: use the evidence half-life decay,
        // independent of whether the contribution survives min_contribution.
        let age = (now - e.at).max(0) as f64;
        let hl = (config.confidence_age_halflife.max(1)) as f64;
        freshness_accum += 0.5_f64.powf(age / hl);

        if contribution.abs() >= config.min_contribution {
            sum += contribution;
            contributions.push(EvidenceContribution {
                kind: e.kind.clone(),
                signed_contribution: contribution,
                at: e.at,
            });
        }
    }

    let inferred_standing = sum.clamp(-1.0, 1.0);

    // Confidence: 60% count component, 40% freshness component.
    let n = matching.len() as f64;
    let sat = config.confidence_saturation_count.max(1.0);
    let count_score = (n / sat).min(1.0);
    let freshness = (freshness_accum / n).clamp(0.0, 1.0);
    let confidence = (count_score * 0.6 + freshness * 0.4).clamp(0.0, 1.0);

    // Sort breakdown by descending absolute contribution.
    contributions.sort_by(|a, b| {
        b.signed_contribution
            .abs()
            .partial_cmp(&a.signed_contribution.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    (
        PerceivedStanding {
            observer,
            target,
            subject,
            inferred_standing,
            confidence,
            evidence_count: matching.len(),
            computed_at: now,
        },
        contributions,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::StandingEvidence;
    use crate::retention::Retention;
    use crate::spec::EvidenceSpec;
    use crate::warning::WarningMode;

    /// Minimal AiParams stub for tests. Reads overrides from a HashMap; falls
    /// back to the trait-default for unknown keys.
    #[derive(Default)]
    struct StubParams {
        map: HashMap<String, f64>,
    }

    impl StubParams {
        fn with(mut self, k: &str, v: f64) -> Self {
            self.map.insert(k.to_string(), v);
            self
        }
    }

    impl AiParamsExt for StubParams {
        fn ai_param_f64(&self, key: &str, default: f64) -> f64 {
            self.map.get(key).copied().unwrap_or(default)
        }
    }

    fn attack_kind() -> EvidenceKindId {
        EvidenceKindId::from("direct_attack")
    }

    fn gift_kind() -> EvidenceKindId {
        EvidenceKindId::from("gift_given")
    }

    fn buildup_kind() -> EvidenceKindId {
        EvidenceKindId::from("military_buildup")
    }

    fn make_bus() -> AiBus {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        bus.declare_evidence(attack_kind(), EvidenceSpec::new(Retention::Long, "atk"));
        bus.declare_evidence(gift_kind(), EvidenceSpec::new(Retention::Long, "gft"));
        bus.declare_evidence(buildup_kind(), EvidenceSpec::new(Retention::Long, "bd"));
        bus
    }

    fn config_with_default_weights() -> StandingConfig {
        let mut cfg = StandingConfig::default();
        cfg.kinds.insert(
            attack_kind(),
            EvidenceKindConfig {
                base_weight: -0.5,
                ambiguous: false,
                interpretation_key: None,
            },
        );
        cfg.kinds.insert(
            gift_kind(),
            EvidenceKindConfig {
                base_weight: 0.3,
                ambiguous: false,
                interpretation_key: None,
            },
        );
        cfg.kinds.insert(
            buildup_kind(),
            EvidenceKindConfig {
                base_weight: -0.1,
                ambiguous: true,
                interpretation_key: Some("paranoia".into()),
            },
        );
        // Disable personality decay by default in tests — individual tests
        // re-enable it as needed.
        cfg.use_personality_decay = false;
        cfg
    }

    #[test]
    fn empty_evidence_returns_neutral_and_default_confidence() {
        let bus = make_bus();
        let cfg = config_with_default_weights();
        let p = StubParams::default();
        let r = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            100,
            &cfg,
            &p,
        );
        assert_eq!(r.inferred_standing, 0.0);
        assert!((r.confidence - 0.3).abs() < 1e-9);
        assert_eq!(r.evidence_count, 0);
    }

    #[test]
    fn single_hostile_evidence_produces_negative_score() {
        let mut bus = make_bus();
        bus.emit_evidence(StandingEvidence::new(
            attack_kind(),
            FactionId(1),
            FactionId(2),
            1.0,
            50,
        ));
        let cfg = config_with_default_weights();
        let p = StubParams::default();
        let r = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            50,
            &cfg,
            &p,
        );
        assert!(r.inferred_standing < 0.0);
        // With default aggressiveness=0.5, hostile_bias_factor=0.3:
        // base = 1.0 * -0.5 = -0.5
        // polarity bias: -0.5 * (1 + 0.5 * 0.3) = -0.5 * 1.15 = -0.575
        assert!((r.inferred_standing + 0.575).abs() < 1e-9);
    }

    #[test]
    fn decay_halflife_applied_correctly() {
        let mut bus = make_bus();
        bus.emit_evidence(
            StandingEvidence::new(attack_kind(), FactionId(1), FactionId(2), 1.0, 0)
                .with_halflife(100),
        );
        let mut cfg = config_with_default_weights();
        // Disable polarity bias by using neutral stub params (aggressiveness=0).
        cfg.hostile_bias_factor = 0.0;
        let p = StubParams::default().with("aggressiveness", 0.0);
        // At t=100, current_magnitude halves from 1.0 -> 0.5.
        let r = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            100,
            &cfg,
            &p,
        );
        // base = 0.5 * -0.5 = -0.25
        assert!(
            (r.inferred_standing + 0.25).abs() < 1e-9,
            "got {}",
            r.inferred_standing
        );
    }

    #[test]
    fn per_kind_weight_affects_score() {
        let mut bus = make_bus();
        bus.emit_evidence(StandingEvidence::new(
            gift_kind(),
            FactionId(1),
            FactionId(2),
            1.0,
            10,
        ));
        let mut cfg = config_with_default_weights();
        cfg.friendly_bias_factor = 0.0;
        let p = StubParams::default().with("defensive_bias", 0.0);
        let r = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            10,
            &cfg,
            &p,
        );
        // base = 1.0 * 0.3 = 0.3, no biases/decay.
        assert!((r.inferred_standing - 0.3).abs() < 1e-9);
    }

    #[test]
    fn aggressive_faction_amplifies_hostile_evidence() {
        let mut bus = make_bus();
        bus.emit_evidence(StandingEvidence::new(
            attack_kind(),
            FactionId(1),
            FactionId(2),
            1.0,
            10,
        ));
        let cfg = config_with_default_weights();
        let calm = StubParams::default().with("aggressiveness", 0.0);
        let fierce = StubParams::default().with("aggressiveness", 1.0);
        let r_calm = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            10,
            &cfg,
            &calm,
        );
        let r_fierce = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            10,
            &cfg,
            &fierce,
        );
        assert!(r_fierce.inferred_standing < r_calm.inferred_standing);
    }

    #[test]
    fn grudge_persistence_retains_negative_longer() {
        let mut bus = make_bus();
        bus.emit_evidence(StandingEvidence::new(
            attack_kind(),
            FactionId(1),
            FactionId(2),
            1.0,
            0,
        ));
        let mut cfg = config_with_default_weights();
        cfg.use_personality_decay = true;
        cfg.hostile_bias_factor = 0.0;
        // Default grudge_persistence = 0.005; friendship_persistence = 0.02.
        let p_long = StubParams::default()
            .with("aggressiveness", 0.0)
            .with("grudge_persistence", 0.001);
        let p_short = StubParams::default()
            .with("aggressiveness", 0.0)
            .with("grudge_persistence", 0.05);
        let r_long = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            200,
            &cfg,
            &p_long,
        );
        let r_short = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            200,
            &cfg,
            &p_short,
        );
        // Both negative; long grudge → more negative (larger magnitude).
        assert!(r_long.inferred_standing < r_short.inferred_standing);
        assert!(r_long.inferred_standing < 0.0);
        assert!(r_short.inferred_standing < 0.0);
    }

    #[test]
    fn confidence_saturates_with_evidence_count() {
        let mut bus = make_bus();
        let cfg = config_with_default_weights();
        let p = StubParams::default();
        // Emit plenty of evidence at `now` so freshness=1.0.
        for t in 0..30 {
            bus.emit_evidence(StandingEvidence::new(
                attack_kind(),
                FactionId(1),
                FactionId(2),
                0.01,
                t,
            ));
        }
        let r = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            30,
            &cfg,
            &p,
        );
        // With 30 > saturation=20, count_score = 1.0. Freshness is close to
        // 1.0 for small ages; confidence ≈ 0.6 + 0.4 * freshness -> near 1.0.
        assert!(r.confidence > 0.9, "got {}", r.confidence);
        assert!(r.confidence <= 1.0);
    }

    #[test]
    fn extreme_evidence_clamps_to_pm1() {
        let mut bus = make_bus();
        for t in 0..50 {
            bus.emit_evidence(StandingEvidence::new(
                attack_kind(),
                FactionId(1),
                FactionId(2),
                10.0,
                t,
            ));
        }
        let cfg = config_with_default_weights();
        let p = StubParams::default();
        let r = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            50,
            &cfg,
            &p,
        );
        assert!(r.inferred_standing >= -1.0);
        assert!((r.inferred_standing + 1.0).abs() < 1e-9);
    }

    #[test]
    fn breakdown_ordered_by_abs_contribution() {
        let mut bus = make_bus();
        bus.emit_evidence(StandingEvidence::new(
            gift_kind(),
            FactionId(1),
            FactionId(2),
            0.1,
            10,
        ));
        bus.emit_evidence(StandingEvidence::new(
            attack_kind(),
            FactionId(1),
            FactionId(2),
            1.0,
            20,
        ));
        let cfg = config_with_default_weights();
        let p = StubParams::default();
        let (_r, breakdown) = compute_with_breakdown(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            20,
            &cfg,
            &p,
        );
        assert_eq!(breakdown.len(), 2);
        assert!(
            breakdown[0].signed_contribution.abs() >= breakdown[1].signed_contribution.abs(),
            "not sorted desc: {:?}",
            breakdown
        );
        assert_eq!(breakdown[0].kind, attack_kind());
    }

    #[test]
    fn min_contribution_filters_from_breakdown() {
        let mut bus = make_bus();
        // Tiny magnitude gift + meaningful attack.
        bus.emit_evidence(StandingEvidence::new(
            gift_kind(),
            FactionId(1),
            FactionId(2),
            0.001,
            5,
        ));
        bus.emit_evidence(StandingEvidence::new(
            attack_kind(),
            FactionId(1),
            FactionId(2),
            1.0,
            10,
        ));
        let mut cfg = config_with_default_weights();
        cfg.min_contribution = 0.01; // filters gift (0.001 * 0.3 = 0.0003)
        let p = StubParams::default();
        let (_r, breakdown) = compute_with_breakdown(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            10,
            &cfg,
            &p,
        );
        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0].kind, attack_kind());
    }

    #[test]
    fn subject_other_and_world_return_neutral_phase1() {
        let mut bus = make_bus();
        bus.emit_evidence(StandingEvidence::new(
            attack_kind(),
            FactionId(1),
            FactionId(2),
            1.0,
            10,
        ));
        let cfg = config_with_default_weights();
        let p = StubParams::default();
        let r_other = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::Other(FactionId(3)),
            10,
            &cfg,
            &p,
        );
        assert_eq!(r_other.inferred_standing, 0.0);
        assert_eq!(r_other.evidence_count, 0);
        let r_world = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::World,
            10,
            &cfg,
            &p,
        );
        assert_eq!(r_world.inferred_standing, 0.0);
    }

    #[test]
    fn standing_level_from_score_buckets() {
        let t = StandingLevelThresholds::default();
        assert_eq!(StandingLevel::from_score(-1.0, &t), StandingLevel::Hostile);
        assert_eq!(StandingLevel::from_score(-0.5, &t), StandingLevel::Wary);
        assert_eq!(StandingLevel::from_score(0.0, &t), StandingLevel::Neutral);
        assert_eq!(StandingLevel::from_score(0.3, &t), StandingLevel::Cordial);
        assert_eq!(StandingLevel::from_score(0.6, &t), StandingLevel::Friendly);
        assert_eq!(StandingLevel::from_score(0.9, &t), StandingLevel::Allied);
    }

    #[test]
    fn ambiguous_kind_interpretation_uses_key() {
        let mut bus = make_bus();
        bus.emit_evidence(StandingEvidence::new(
            buildup_kind(),
            FactionId(1),
            FactionId(2),
            1.0,
            10,
        ));
        let mut cfg = config_with_default_weights();
        cfg.hostile_bias_factor = 0.0;
        let paranoid = StubParams::default()
            .with("aggressiveness", 0.0)
            .with("paranoia", 1.0);
        let trusting = StubParams::default()
            .with("aggressiveness", 0.0)
            .with("paranoia", 0.0);
        let r_par = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            10,
            &cfg,
            &paranoid,
        );
        let r_trust = compute(
            &bus,
            FactionId(1),
            FactionId(2),
            StandingSubject::ObserverSelf,
            10,
            &cfg,
            &trusting,
        );
        // Paranoid faction views buildup as more hostile.
        assert!(r_par.inferred_standing < r_trust.inferred_standing);
    }
}
