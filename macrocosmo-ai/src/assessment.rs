//! Assessment — Tier 3 strategic self-model (issue #194, economic subset).
//!
//! An `Assessment` snapshots "how the AI sees itself right now": economic
//! capacity, tech position, feasibility of its current objective, perceived
//! standings with known rivals, strategic windows detected from trajectory
//! projections, and preconditions over the current objective.
//!
//! The module is deliberately **pure + bus-backed**: the caller owns the
//! `AiBus` and a long-lived `PreconditionTracker`, invokes `build_assessment`
//! on its own cadence (typical: every few ticks), and receives a fresh
//! `Assessment` value which can be cached / diff'd / serialized.
//!
//! # Scope — economic subset
//!
//! This initial cut of #194 ships:
//!
//! - [`EconomicSnapshot`] + [`compute_economic_capacity`] (full)
//! - [`TechPositionSnapshot`] + [`compute_tech_lead`] (full)
//! - [`compute_overall_confidence`] (full)
//! - [`compute_feasibility`] — non-combat objective kinds fully implemented;
//!   combat-heavy kinds (`Conquer`, `DefensivePosture`, `Coalition`) fall back
//!   to `0.7 * precond + 0.3 * econ_cap` pending the fleet / threat subset.
//! - [`build_assessment`] — full orchestration
//! - [`FleetSnapshot`] — `Default` only (combat subset deferred to #190)
//! - [`compute_threat_level`] / [`compute_fleet_readiness`] — return `0.0`
//!   with a TODO for #190.
//!
//! Nash payoff integration is supported via an optional parameter in
//! [`compute_feasibility`]; callers not yet on Nash simply pass `None`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ai_params::AiParamsExt;
use crate::bus::AiBus;
use crate::eval::EvalContext;
use crate::ids::{FactionId, IntentId, MetricId, ObjectiveId};
use crate::objective::Objective;
use crate::precondition::{PreconditionSet, PreconditionSummary, PreconditionTracker};
use crate::projection::{
    self, StrategicWindow, TrajectoryConfig, WindowDetectionConfig, detect_windows, project,
};
use crate::standing::{self, PerceivedStanding, StandingConfig, StandingSubject};
use crate::time::Tick;

// -------------------------------------------------------------------------
// Data types
// -------------------------------------------------------------------------

/// Five-resource vector used by [`EconomicSnapshot::net_production`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceVector {
    pub minerals: f32,
    pub energy: f32,
    pub food: f32,
    pub research: f32,
    pub authority: f32,
}

impl ResourceVector {
    /// Sum of all components (used as a scalar proxy for total production).
    pub fn total_value(&self) -> f32 {
        self.minerals + self.energy + self.food + self.research + self.authority
    }
}

/// Economic-state snapshot. Built by [`build_economic_snapshot`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EconomicSnapshot {
    pub net_production: ResourceVector,
    /// Population growth rate per tick (clamped `[-0.2, 0.5]`).
    pub projected_growth_rate: f32,
    /// Months of runway until the most-constrained stockpile runs out
    /// (`f32::INFINITY` if all net productions are non-negative).
    pub stockpile_months: f32,
    pub colony_count: u32,
    pub population_total: f32,
    /// `1 - HHI(production shares)`. `0.0` when production is zero.
    pub production_diversity: f32,
}

/// Tech-state snapshot. Built by [`build_tech_position_snapshot`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TechPositionSnapshot {
    pub my_tech_level: f32,
    pub research_output: f32,
    pub known_competitor_levels: HashMap<FactionId, f32>,
}

/// Fleet-state snapshot. Placeholder until #190 (combat).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FleetSnapshot {
    // TODO(#190): total_strength, doctrine_match, ...
}

/// Baseline for normalising economic metrics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EconomicBaseline {
    /// "Mature" total per-tick production; used as denominator in the
    /// production component of [`compute_economic_capacity`].
    pub expected_production: f32,
    /// Ticks per month — defaults to 5 (`HEXADIES_PER_MONTH`).
    pub ticks_per_month: Tick,
}

impl Default for EconomicBaseline {
    fn default() -> Self {
        Self {
            expected_production: 500.0,
            // 1 month = 5 hexadies (HEXADIES_PER_MONTH in the engine).
            ticks_per_month: 5,
        }
    }
}

/// Weights used to combine economic sub-scores into `economic_capacity`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EconomicCapacityWeights {
    pub production: f32,
    pub growth: f32,
    pub reserves: f32,
    pub scale: f32,
    pub diversity: f32,
}

impl Default for EconomicCapacityWeights {
    fn default() -> Self {
        Self {
            production: 0.35,
            growth: 0.25,
            reserves: 0.15,
            scale: 0.15,
            diversity: 0.10,
        }
    }
}

impl EconomicCapacityWeights {
    /// Build weights from an `AiParamsExt` provider. Unknown keys fall back
    /// to [`Default`].
    pub fn from_params<P: AiParamsExt + ?Sized>(p: &P) -> Self {
        let d = Self::default();
        Self {
            production: p.ai_param_f64("economic_capacity.production", d.production as f64) as f32,
            growth: p.ai_param_f64("economic_capacity.growth", d.growth as f64) as f32,
            reserves: p.ai_param_f64("economic_capacity.reserves", d.reserves as f64) as f32,
            scale: p.ai_param_f64("economic_capacity.scale", d.scale as f64) as f32,
            diversity: p.ai_param_f64("economic_capacity.diversity", d.diversity as f64) as f32,
        }
    }
}

/// Weights used to combine the two tech-lead sub-scores (vs. max rival, vs.
/// average rival).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TechLeadWeights {
    pub vs_max: f32,
    pub vs_avg: f32,
}

impl Default for TechLeadWeights {
    fn default() -> Self {
        Self {
            vs_max: 0.6,
            vs_avg: 0.4,
        }
    }
}

impl TechLeadWeights {
    pub fn from_params<P: AiParamsExt + ?Sized>(p: &P) -> Self {
        let d = Self::default();
        Self {
            vs_max: p.ai_param_f64("tech_lead.vs_max", d.vs_max as f64) as f32,
            vs_avg: p.ai_param_f64("tech_lead.vs_avg", d.vs_avg as f64) as f32,
        }
    }
}

/// Composite configuration for one [`build_assessment`] call.
#[derive(Debug, Clone)]
pub struct AssessmentConfig {
    pub baseline: EconomicBaseline,
    pub economic_weights: EconomicCapacityWeights,
    pub tech_weights: TechLeadWeights,
    pub window_detection: WindowDetectionConfig,
    pub standing_config: StandingConfig,
    pub trajectory_config: TrajectoryConfig,
    /// Half-life (ticks) for knowledge freshness decay in confidence.
    pub knowledge_freshness_halflife: Tick,
    /// Blend factor for Nash payoff in [`compute_feasibility`]. `0.3` means
    /// `0.7 * heuristic + 0.3 * nash`.
    pub nash_blend: f64,
}

impl Default for AssessmentConfig {
    fn default() -> Self {
        Self {
            baseline: EconomicBaseline::default(),
            economic_weights: EconomicCapacityWeights::default(),
            tech_weights: TechLeadWeights::default(),
            window_detection: WindowDetectionConfig::default(),
            standing_config: StandingConfig::default(),
            trajectory_config: TrajectoryConfig::default(),
            knowledge_freshness_halflife: 60,
            nash_blend: 0.3,
        }
    }
}

/// Top-level strategic self-model produced by [`build_assessment`].
///
/// All `f32` sub-scores live in `[0.0, 1.0]` unless otherwise documented.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Assessment {
    pub economic: EconomicSnapshot,
    pub fleet: FleetSnapshot,
    pub tech_position: TechPositionSnapshot,
    pub perceived_standings: HashMap<FactionId, PerceivedStanding>,
    /// Per-faction threat contribution. Placeholder until #190 lands.
    pub threat_breakdown: HashMap<FactionId, f32>,
    pub feasibility: f32,
    pub threat_level: f32,
    pub economic_capacity: f32,
    pub fleet_readiness: f32,
    pub tech_lead: f32,
    /// Snapshot of the caller's [`PreconditionTracker`] at build time.
    #[serde(skip)]
    pub precondition_tracker: PreconditionTracker,
    pub objective_precondition_summary: PreconditionSummary,
    pub intent_precondition_summaries: HashMap<IntentId, PreconditionSummary>,
    pub strategic_windows: Vec<StrategicWindow>,
    pub confidence: f32,
    pub last_updated_at: Tick,
    pub last_nash_at: Option<Tick>,
}

// -------------------------------------------------------------------------
// Objective kind routing
// -------------------------------------------------------------------------

/// Coarse grouping used by [`compute_feasibility`] to choose a formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveKind {
    TechLeader,
    Expand,
    EconomicDominance,
    Survive,
    Conquer,
    DefensivePosture,
    Coalition,
    Unknown,
}

/// Map an [`ObjectiveId`] to its [`ObjectiveKind`] by prefix / exact match.
pub fn objective_kind(id: &ObjectiveId) -> ObjectiveKind {
    match id.as_str() {
        "tech_leader" => ObjectiveKind::TechLeader,
        "expand" => ObjectiveKind::Expand,
        "economic_dominance" => ObjectiveKind::EconomicDominance,
        "survive" => ObjectiveKind::Survive,
        "conquer" => ObjectiveKind::Conquer,
        "defensive_posture" => ObjectiveKind::DefensivePosture,
        "coalition" => ObjectiveKind::Coalition,
        _ => ObjectiveKind::Unknown,
    }
}

/// Penalty multiplier applied to a weighted-satisfaction score when a
/// CRITICAL precondition is violated. Current policy is a blanket `0.5`.
pub fn critical_violation_penalty(_objective: &ObjectiveId) -> f32 {
    0.5
}

// -------------------------------------------------------------------------
// Compute functions (pure)
// -------------------------------------------------------------------------

/// Combine economic sub-scores into a scalar `economic_capacity ∈ [0, 1]`.
pub fn compute_economic_capacity(
    s: &EconomicSnapshot,
    baseline: &EconomicBaseline,
    w: &EconomicCapacityWeights,
) -> f32 {
    let expected = baseline.expected_production.max(f32::EPSILON);
    // Production: ratio of net production to expected; clamped [0, 2] and
    // halved so it lands in [0, 1].
    let production = (s.net_production.total_value() / expected).clamp(0.0, 2.0) * 0.5;
    // Growth: scale growth-rate so a mature 10%/tick lands at 1.0.
    let growth = (s.projected_growth_rate * 10.0).clamp(0.0, 1.0);
    // Reserves: 12 "months" of runway saturates.
    let reserves = if s.stockpile_months.is_finite() {
        (s.stockpile_months / 12.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    // Scale: colony-count saturation.
    let scale = (s.colony_count as f32 / 10.0).clamp(0.0, 1.0);
    let diversity = s.production_diversity.clamp(0.0, 1.0);

    (production * w.production
        + growth * w.growth
        + reserves * w.reserves
        + scale * w.scale
        + diversity * w.diversity)
        .clamp(0.0, 1.0)
}

/// `tech_lead ∈ [0, 1]`: `0.5` is neutral (on par with rivals).
pub fn compute_tech_lead(s: &TechPositionSnapshot, w: &TechLeadWeights) -> f32 {
    let others: Vec<f32> = s.known_competitor_levels.values().copied().collect();
    if others.is_empty() {
        return 0.5;
    }
    let max_o = others.iter().copied().fold(0.0_f32, f32::max);
    let avg_o = others.iter().sum::<f32>() / others.len() as f32;
    let vs_max = if max_o > 0.0 {
        (s.my_tech_level / max_o).clamp(0.0, 2.0) * 0.5
    } else {
        1.0
    };
    let vs_avg = if avg_o > 0.0 {
        (s.my_tech_level / avg_o).clamp(0.0, 2.0) * 0.5
    } else {
        1.0
    };
    (vs_max * w.vs_max + vs_avg * w.vs_avg).clamp(0.0, 1.0)
}

/// Placeholder for #190. Always `0.0`.
pub fn compute_threat_level(
    _bus: &AiBus,
    _me: FactionId,
    _perceived: &HashMap<FactionId, PerceivedStanding>,
) -> f32 {
    // TODO(#190): weight standings by estimated rival strength, neighbour distance,
    // fleet deltas; for now a zero placeholder keeps `threat_level` numeric.
    0.0
}

/// Placeholder for #190. Always `0.0`.
pub fn compute_fleet_readiness(_bus: &AiBus, _me: FactionId, _fleet: &FleetSnapshot) -> f32 {
    // TODO(#190): integrate ship rosters, doctrine match, repair state.
    0.0
}

/// Combine knowledge freshness, standing confidence, enemy-estimate confidence
/// and trajectory confidence into a scalar confidence in `[0, 1]`.
pub fn compute_overall_confidence(
    bus: &AiBus,
    assessment: &Assessment,
    config: &AssessmentConfig,
    now: Tick,
) -> f32 {
    let k = knowledge_freshness(bus, now, config.knowledge_freshness_halflife);
    let s = standing_confidence(&assessment.perceived_standings);
    // TODO(#190): replace constant with a real estimate-confidence once fleet
    // / threat snapshots land.
    let e = 0.5_f32;
    let t = trajectory_confidence(bus, now, &config.trajectory_config);
    (0.4 * k + 0.2 * s + 0.2 * e + 0.2 * t).clamp(0.0, 1.0)
}

/// Compute the feasibility of an objective given the current [`Assessment`].
///
/// If `nash_payoff` is `Some`, the final score is blended as
/// `base * (1 - nash_blend) + nash_payoff * nash_blend` (both in `[0, 1]`).
pub fn compute_feasibility(
    objective: &Objective,
    assessment: &Assessment,
    nash_payoff: Option<f64>,
    nash_blend: f64,
) -> f32 {
    let precond = &assessment.objective_precondition_summary;
    if precond.has_critical_violation() {
        return (precond.weighted_satisfaction * critical_violation_penalty(&objective.id))
            .clamp(0.0, 1.0);
    }
    let a = assessment;
    let base = match objective_kind(&objective.id) {
        ObjectiveKind::TechLeader => {
            a.tech_lead * 0.45
                + (a.economic.net_production.research / 100.0).min(1.0) * 0.30
                + precond.weighted_satisfaction * 0.25
        }
        ObjectiveKind::Expand => {
            a.economic_capacity * 0.25
                + (a.economic.projected_growth_rate * 10.0).clamp(0.0, 1.0) * 0.20
                + 0.30 // expansion_room placeholder 1.0 * 0.30
                + precond.weighted_satisfaction * 0.25
        }
        ObjectiveKind::EconomicDominance => {
            a.economic_capacity * 0.60 + a.tech_lead * 0.20 + precond.weighted_satisfaction * 0.20
        }
        ObjectiveKind::Survive => precond.weighted_satisfaction * 0.7 + a.economic_capacity * 0.3,
        // Combat-heavy objectives fall back to precond + economy until #190.
        ObjectiveKind::Conquer | ObjectiveKind::DefensivePosture | ObjectiveKind::Coalition => {
            precond.weighted_satisfaction * 0.7 + a.economic_capacity * 0.3
        }
        ObjectiveKind::Unknown => precond.weighted_satisfaction,
    };
    let base = base.clamp(0.0, 1.0);
    let blended = match nash_payoff {
        Some(p) => base as f64 * (1.0 - nash_blend) + p * nash_blend,
        None => base as f64,
    };
    blended.clamp(0.0, 1.0) as f32
}

// -------------------------------------------------------------------------
// Snapshot builders
// -------------------------------------------------------------------------

/// Read the Tier 1 self-metrics and compose an [`EconomicSnapshot`].
///
/// Missing metrics default to `0.0`. The projected growth rate is fit from
/// up to 60 ticks of `population_total` history if available; otherwise we
/// fall back to the `population_growth_rate` metric and finally `0.0`.
pub fn build_economic_snapshot(
    bus: &AiBus,
    _me: FactionId,
    baseline: &EconomicBaseline,
    now: Tick,
) -> EconomicSnapshot {
    let mg = |name: &str| bus.current(&MetricId::from(name)).unwrap_or(0.0) as f32;

    let net_production = ResourceVector {
        minerals: mg("net_production_minerals"),
        energy: mg("net_production_energy"),
        food: mg("net_production_food"),
        research: mg("net_production_research"),
        authority: mg("net_production_authority"),
    };

    let population_total = mg("population_total");
    let colony_count = mg("colony_count").max(0.0) as u32;

    // Growth rate: try a linear fit on population history, else fall back to
    // the emitted population_growth_rate metric, else 0.
    let population_metric = MetricId::from("population_total");
    let history: Vec<_> = bus.window(&population_metric, now, 60).cloned().collect();
    let projected_growth_rate = if history.len() >= 2 {
        if let Some(fit) = projection::fit_linear(&history, now) {
            let denom = population_total.abs().max(1.0);
            (fit.slope as f32 / denom).clamp(-0.2, 0.5)
        } else {
            mg("population_growth_rate").clamp(-0.2, 0.5)
        }
    } else {
        mg("population_growth_rate").clamp(-0.2, 0.5)
    };

    let stockpile_months = compute_stockpile_months(&net_production, bus, baseline.ticks_per_month);

    let production_diversity = compute_production_diversity(&net_production);

    EconomicSnapshot {
        net_production,
        projected_growth_rate,
        stockpile_months,
        colony_count,
        population_total,
        production_diversity,
    }
}

/// Read Tier 1 self-tech metrics and per-faction foreign slots to build a
/// [`TechPositionSnapshot`].
///
/// - `my_tech_level` ← `tech_total_researched`
/// - `research_output` ← `net_production_research`
/// - `known_competitor_levels[f]` ← `foreign.research_output.faction_<f>`
pub fn build_tech_position_snapshot(
    bus: &AiBus,
    me: FactionId,
    known_factions: &[FactionId],
) -> TechPositionSnapshot {
    let my_tech_level = bus
        .current(&MetricId::from("tech_total_researched"))
        .unwrap_or(0.0) as f32;
    let research_output = bus
        .current(&MetricId::from("net_production_research"))
        .unwrap_or(0.0) as f32;
    let mut known_competitor_levels: HashMap<FactionId, f32> = HashMap::new();
    for &f in known_factions {
        if f == me {
            continue;
        }
        let id = MetricId::from(format!("foreign.research_output.faction_{}", f.0));
        if let Some(v) = bus.current(&id) {
            known_competitor_levels.insert(f, v as f32);
        }
    }
    TechPositionSnapshot {
        my_tech_level,
        research_output,
        known_competitor_levels,
    }
}

// -------------------------------------------------------------------------
// Orchestration
// -------------------------------------------------------------------------

/// Build a complete [`Assessment`] from the current bus state.
///
/// The `tracker` argument is the caller's long-lived precondition history; we
/// advance it with the current evaluation and embed a clone in the returned
/// `Assessment` (so callers can cheaply snapshot history alongside the
/// strategic state).
pub fn build_assessment<P: AiParamsExt>(
    bus: &AiBus,
    me: FactionId,
    known_factions: &[FactionId],
    objective: &Objective,
    precondition_set: &PreconditionSet,
    tracker: &mut PreconditionTracker,
    now: Tick,
    config: &AssessmentConfig,
    params: &P,
) -> Assessment {
    let economic = build_economic_snapshot(bus, me, &config.baseline, now);
    let tech_position = build_tech_position_snapshot(bus, me, known_factions);
    let fleet = FleetSnapshot::default();

    // Perceived standings toward each known rival.
    let mut perceived_standings: HashMap<FactionId, PerceivedStanding> = HashMap::new();
    for &other in known_factions {
        if other == me {
            continue;
        }
        let s = standing::compute(
            bus,
            me,
            other,
            StandingSubject::ObserverSelf,
            now,
            &config.standing_config,
            params,
        );
        perceived_standings.insert(other, s);
    }

    // Strategic windows from projections of the metrics we care about.
    let target_metrics = gather_trajectory_metric_ids(me, known_factions);
    let trajectories = project(bus, &target_metrics, &config.trajectory_config, now, &[]);
    let strategic_windows = detect_windows(&trajectories, now, &config.window_detection);

    // Preconditions.
    let eval_ctx = EvalContext::new(bus, now)
        .with_faction(me)
        .with_standing_config(&config.standing_config)
        .with_ai_params(params);
    let detailed_results = precondition_set.evaluate_detailed(&eval_ctx);
    tracker.record(&detailed_results, now);
    let objective_precondition_summary = PreconditionSummary::from_results(&detailed_results, now);
    let intent_precondition_summaries: HashMap<IntentId, PreconditionSummary> = HashMap::new();

    // Assemble intermediate Assessment so we can run derived computations.
    let mut a = Assessment {
        economic,
        fleet,
        tech_position,
        perceived_standings,
        threat_breakdown: HashMap::new(),
        feasibility: 0.0,
        threat_level: 0.0,
        economic_capacity: 0.0,
        fleet_readiness: 0.0,
        tech_lead: 0.0,
        precondition_tracker: tracker.clone(),
        objective_precondition_summary,
        intent_precondition_summaries,
        strategic_windows,
        confidence: 0.0,
        last_updated_at: now,
        last_nash_at: None,
    };
    a.economic_capacity =
        compute_economic_capacity(&a.economic, &config.baseline, &config.economic_weights);
    a.tech_lead = compute_tech_lead(&a.tech_position, &config.tech_weights);
    a.fleet_readiness = compute_fleet_readiness(bus, me, &a.fleet);
    a.threat_level = compute_threat_level(bus, me, &a.perceived_standings);
    a.feasibility = compute_feasibility(objective, &a, None, config.nash_blend);
    a.confidence = compute_overall_confidence(bus, &a, config, now);
    a
}

/// Metrics whose trajectories feed [`detect_windows`] during assessment.
/// Exposed for tests and for callers who want to pre-project externally.
pub fn gather_trajectory_metric_ids(_me: FactionId, known_factions: &[FactionId]) -> Vec<MetricId> {
    let mut out = vec![
        MetricId::from("net_production_minerals"),
        MetricId::from("net_production_energy"),
        MetricId::from("net_production_food"),
        MetricId::from("net_production_research"),
        MetricId::from("net_production_authority"),
        MetricId::from("population_total"),
        MetricId::from("tech_total_researched"),
        MetricId::from("colony_count"),
    ];
    for &f in known_factions {
        out.push(MetricId::from(format!(
            "foreign.research_output.faction_{}",
            f.0
        )));
        out.push(MetricId::from(format!("foreign.strength.faction_{}", f.0)));
    }
    out
}

// -------------------------------------------------------------------------
// Internals
// -------------------------------------------------------------------------

fn compute_stockpile_months(
    net_production: &ResourceVector,
    bus: &AiBus,
    ticks_per_month: Tick,
) -> f32 {
    let pairs: [(f32, &str); 3] = [
        (net_production.minerals, "stockpile_minerals"),
        (net_production.energy, "stockpile_energy"),
        (net_production.food, "stockpile_food"),
    ];
    let tpm = ticks_per_month.max(1) as f32;
    let mut worst = f32::INFINITY;
    for (prod, stock_id) in pairs {
        if prod < 0.0 {
            let stock = bus
                .current(&MetricId::from(stock_id))
                .unwrap_or(0.0)
                .max(0.0) as f32;
            let months = stock / (-prod) / tpm;
            if months < worst {
                worst = months;
            }
        }
    }
    worst
}

fn compute_production_diversity(v: &ResourceVector) -> f32 {
    let shares = [
        v.minerals.max(0.0),
        v.energy.max(0.0),
        v.food.max(0.0),
        v.research.max(0.0),
        v.authority.max(0.0),
    ];
    let total: f32 = shares.iter().copied().sum();
    if total <= f32::EPSILON {
        return 0.0;
    }
    let hhi: f32 = shares.iter().map(|s| (s / total).powi(2)).sum();
    (1.0 - hhi).clamp(0.0, 1.0)
}

/// Mean exp-decay of the core self-metrics' age since last emit.
fn knowledge_freshness(bus: &AiBus, now: Tick, halflife: Tick) -> f32 {
    let core_ids = [
        "net_production_minerals",
        "net_production_energy",
        "net_production_food",
        "net_production_research",
        "population_total",
        "colony_count",
        "tech_total_researched",
    ];
    let hl = halflife.max(1) as f64;
    let mut acc = 0.0_f64;
    let mut n = 0.0_f64;
    for id in core_ids {
        if let Some(at) = bus.latest_at(&MetricId::from(id)) {
            let age = (now - at).max(0) as f64;
            acc += 0.5_f64.powf(age / hl);
            n += 1.0;
        }
    }
    if n == 0.0 {
        return 0.5;
    }
    (acc / n) as f32
}

fn standing_confidence(map: &HashMap<FactionId, PerceivedStanding>) -> f32 {
    if map.is_empty() {
        return 0.5;
    }
    let sum: f64 = map.values().map(|p| p.confidence).sum();
    (sum / map.len() as f64).clamp(0.0, 1.0) as f32
}

fn trajectory_confidence(bus: &AiBus, now: Tick, cfg: &TrajectoryConfig) -> f32 {
    let tr = projection::project_metric(
        bus,
        &MetricId::from("net_production_minerals"),
        cfg,
        now,
        &[],
    );
    tr.confidence.first().copied().unwrap_or(0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::Condition;
    use crate::feasibility::{FeasibilityFormula, FeasibilityTerm};
    use crate::objective::{Objective, PreconditionSet as ObjPreconditionSet, SuccessCriteria};
    use crate::precondition::{PreconditionItem, PreconditionSet, severity};
    use crate::retention::Retention;
    use crate::spec::MetricSpec;
    use crate::value_expr::ValueExpr;
    use crate::warning::WarningMode;

    #[derive(Default)]
    struct StubParams;
    impl AiParamsExt for StubParams {
        fn ai_param_f64(&self, _key: &str, default: f64) -> f64 {
            default
        }
    }

    fn obj(id: &str) -> Objective {
        Objective::new(
            ObjectiveId::from(id),
            ObjPreconditionSet::always(),
            SuccessCriteria::new(Condition::Always),
            FeasibilityFormula::WeightedSum(vec![FeasibilityTerm::new(
                1.0,
                ValueExpr::Literal(0.0),
            )]),
        )
    }

    #[test]
    fn economic_capacity_scales_with_production() {
        let low = EconomicSnapshot {
            net_production: ResourceVector {
                minerals: 10.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let high = EconomicSnapshot {
            net_production: ResourceVector {
                minerals: 500.0,
                energy: 500.0,
                food: 500.0,
                research: 500.0,
                authority: 500.0,
            },
            colony_count: 5,
            production_diversity: 1.0,
            projected_growth_rate: 0.05,
            stockpile_months: 6.0,
            population_total: 100.0,
        };
        let b = EconomicBaseline::default();
        let w = EconomicCapacityWeights::default();
        assert!(compute_economic_capacity(&high, &b, &w) > compute_economic_capacity(&low, &b, &w));
    }

    #[test]
    fn economic_capacity_zero_when_empty_stockpile() {
        let s = EconomicSnapshot::default();
        let b = EconomicBaseline::default();
        let w = EconomicCapacityWeights::default();
        // stockpile_months = 0.0 (all net productions are 0 → worst stays INFINITY,
        // but we special-case finite only). Default has stockpile_months=0.0
        // which is finite → contributes 0. Net production 0, growth 0, scale 0.
        // Diversity 0. Total should be 0.
        let cap = compute_economic_capacity(&s, &b, &w);
        assert!((cap - 0.0).abs() < 1e-6, "got {cap}");
    }

    #[test]
    fn economic_capacity_saturates_at_2x_baseline() {
        let baseline = EconomicBaseline::default();
        let w = EconomicCapacityWeights::default();
        let s1 = EconomicSnapshot {
            net_production: ResourceVector {
                minerals: baseline.expected_production * 2.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let s2 = EconomicSnapshot {
            net_production: ResourceVector {
                minerals: baseline.expected_production * 10.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let c1 = compute_economic_capacity(&s1, &baseline, &w);
        let c2 = compute_economic_capacity(&s2, &baseline, &w);
        // With identical everything-else and production clamped to 2x, scores match.
        assert!((c1 - c2).abs() < 1e-6);
    }

    #[test]
    fn tech_lead_neutral_without_rivals() {
        let s = TechPositionSnapshot {
            my_tech_level: 100.0,
            research_output: 10.0,
            known_competitor_levels: HashMap::new(),
        };
        let lead = compute_tech_lead(&s, &TechLeadWeights::default());
        assert!((lead - 0.5).abs() < 1e-6);
    }

    #[test]
    fn tech_lead_positive_when_ahead() {
        let mut rivals = HashMap::new();
        rivals.insert(FactionId(1), 40.0);
        rivals.insert(FactionId(2), 60.0);
        let s = TechPositionSnapshot {
            my_tech_level: 120.0,
            research_output: 0.0,
            known_competitor_levels: rivals,
        };
        let neutral = TechPositionSnapshot {
            my_tech_level: 50.0,
            research_output: 0.0,
            known_competitor_levels: {
                let mut m = HashMap::new();
                m.insert(FactionId(1), 50.0);
                m.insert(FactionId(2), 50.0);
                m
            },
        };
        let w = TechLeadWeights::default();
        assert!(compute_tech_lead(&s, &w) > compute_tech_lead(&neutral, &w));
    }

    #[test]
    fn tech_lead_negative_when_behind() {
        let mut rivals = HashMap::new();
        rivals.insert(FactionId(1), 200.0);
        let behind = TechPositionSnapshot {
            my_tech_level: 10.0,
            research_output: 0.0,
            known_competitor_levels: rivals,
        };
        let w = TechLeadWeights::default();
        let lead = compute_tech_lead(&behind, &w);
        assert!(lead < 0.5, "got {lead}");
    }

    #[test]
    fn confidence_aggregates_inputs() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        for m in [
            "net_production_minerals",
            "net_production_energy",
            "net_production_food",
            "net_production_research",
            "population_total",
            "colony_count",
            "tech_total_researched",
        ] {
            bus.declare_metric(MetricId::from(m), MetricSpec::gauge(Retention::Long, m));
            bus.emit(&MetricId::from(m), 1.0, 100);
        }
        let mut a = Assessment::default();
        a.perceived_standings.insert(
            FactionId(1),
            PerceivedStanding {
                observer: FactionId(0),
                target: FactionId(1),
                subject: StandingSubject::ObserverSelf,
                inferred_standing: 0.0,
                confidence: 0.7,
                evidence_count: 5,
                computed_at: 100,
            },
        );
        let cfg = AssessmentConfig::default();
        let conf = compute_overall_confidence(&bus, &a, &cfg, 100);
        assert!(
            conf > 0.3 && conf <= 1.0,
            "expected [0.3, 1.0]-ish, got {conf}"
        );
    }

    #[test]
    fn feasibility_critical_violation_penalizes() {
        let o = obj("economic_dominance");
        let mut a = Assessment::default();
        a.economic_capacity = 1.0;
        a.objective_precondition_summary = PreconditionSummary {
            total: 1,
            satisfied: 0,
            weighted_satisfaction: 0.8,
            critical_violations: vec!["boom".into()],
            evaluated_at: 0,
        };
        let f = compute_feasibility(&o, &a, None, 0.3);
        // 0.8 * 0.5 penalty = 0.4
        assert!((f - 0.4).abs() < 1e-6, "got {f}");
    }

    #[test]
    fn feasibility_tech_leader_uses_tech_and_research() {
        let o = obj("tech_leader");
        let mut a = Assessment::default();
        a.tech_lead = 1.0;
        a.economic.net_production.research = 50.0; // 0.5 normalised
        a.objective_precondition_summary = PreconditionSummary {
            total: 1,
            satisfied: 1,
            weighted_satisfaction: 1.0,
            critical_violations: vec![],
            evaluated_at: 0,
        };
        let f = compute_feasibility(&o, &a, None, 0.0);
        // 1.0*0.45 + 0.5*0.30 + 1.0*0.25 = 0.45 + 0.15 + 0.25 = 0.85
        assert!((f - 0.85).abs() < 1e-5, "got {f}");
    }

    #[test]
    fn feasibility_combat_kind_collapses_to_econ_and_precond() {
        let o = obj("conquer");
        let mut a = Assessment::default();
        a.economic_capacity = 0.5;
        a.objective_precondition_summary = PreconditionSummary {
            total: 1,
            satisfied: 1,
            weighted_satisfaction: 0.8,
            critical_violations: vec![],
            evaluated_at: 0,
        };
        let f = compute_feasibility(&o, &a, None, 0.0);
        // 0.8*0.7 + 0.5*0.3 = 0.56 + 0.15 = 0.71
        assert!((f - 0.71).abs() < 1e-5, "got {f}");
    }

    #[test]
    fn feasibility_nash_blend_shifts_score() {
        let o = obj("economic_dominance");
        let mut a = Assessment::default();
        a.economic_capacity = 0.2;
        a.objective_precondition_summary = PreconditionSummary {
            total: 1,
            satisfied: 1,
            weighted_satisfaction: 0.5,
            critical_violations: vec![],
            evaluated_at: 0,
        };
        let no_blend = compute_feasibility(&o, &a, None, 0.3);
        let with_blend = compute_feasibility(&o, &a, Some(1.0), 0.3);
        assert!(with_blend > no_blend);
    }

    #[test]
    fn placeholders_default_serde_round_trip() {
        let a = Assessment::default();
        let s = serde_json::to_string(&a).expect("serialize");
        let b: Assessment = serde_json::from_str(&s).expect("deserialize");
        assert!((a.feasibility - b.feasibility).abs() < 1e-9);
    }

    #[test]
    fn objective_kind_routes_ids() {
        assert_eq!(
            objective_kind(&ObjectiveId::from("tech_leader")),
            ObjectiveKind::TechLeader
        );
        assert_eq!(
            objective_kind(&ObjectiveId::from("expand")),
            ObjectiveKind::Expand
        );
        assert_eq!(
            objective_kind(&ObjectiveId::from("unknown_thing")),
            ObjectiveKind::Unknown
        );
    }

    #[test]
    fn build_economic_snapshot_reads_bus_metrics() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        for (m, v) in [
            ("net_production_minerals", 100.0),
            ("net_production_energy", 80.0),
            ("net_production_food", 50.0),
            ("net_production_research", 20.0),
            ("net_production_authority", 5.0),
            ("population_total", 1000.0),
            ("colony_count", 4.0),
        ] {
            bus.declare_metric(MetricId::from(m), MetricSpec::gauge(Retention::Long, m));
            bus.emit(&MetricId::from(m), v, 0);
        }
        let s = build_economic_snapshot(&bus, FactionId(0), &EconomicBaseline::default(), 0);
        assert!((s.net_production.minerals - 100.0).abs() < 1e-6);
        assert_eq!(s.colony_count, 4);
        assert!(s.production_diversity > 0.0);
    }

    #[test]
    fn build_tech_position_snapshot_reads_foreign_slots() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        bus.declare_metric(
            MetricId::from("tech_total_researched"),
            MetricSpec::gauge(Retention::Long, "t"),
        );
        bus.emit(&MetricId::from("tech_total_researched"), 100.0, 0);
        bus.declare_metric(
            MetricId::from("net_production_research"),
            MetricSpec::gauge(Retention::Long, "r"),
        );
        bus.emit(&MetricId::from("net_production_research"), 10.0, 0);

        let f1 = MetricId::from("foreign.research_output.faction_1");
        bus.declare_metric(f1.clone(), MetricSpec::gauge(Retention::Long, "f1"));
        bus.emit(&f1, 50.0, 0);
        let snap = build_tech_position_snapshot(&bus, FactionId(0), &[FactionId(1)]);
        assert!((snap.my_tech_level - 100.0).abs() < 1e-6);
        assert_eq!(
            snap.known_competitor_levels.get(&FactionId(1)).copied(),
            Some(50.0)
        );
    }

    #[test]
    fn build_assessment_full_orchestration() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        for (m, v) in [
            ("net_production_minerals", 200.0),
            ("net_production_energy", 150.0),
            ("net_production_food", 120.0),
            ("net_production_research", 40.0),
            ("net_production_authority", 10.0),
            ("population_total", 5000.0),
            ("colony_count", 6.0),
            ("tech_total_researched", 80.0),
            ("stockpile_minerals", 300.0),
            ("stockpile_energy", 300.0),
            ("stockpile_food", 300.0),
        ] {
            bus.declare_metric(MetricId::from(m), MetricSpec::gauge(Retention::Long, m));
            bus.emit(&MetricId::from(m), v, 100);
        }
        let o = obj("economic_dominance");
        let pset = PreconditionSet::new(vec![PreconditionItem::new(
            "alive",
            severity::MAJOR,
            Condition::Always,
        )]);
        let mut tracker = PreconditionTracker::new();
        let cfg = AssessmentConfig::default();
        let a = build_assessment(
            &bus,
            FactionId(0),
            &[],
            &o,
            &pset,
            &mut tracker,
            100,
            &cfg,
            &StubParams::default(),
        );
        assert!((0.0..=1.0).contains(&a.economic_capacity));
        assert!((0.0..=1.0).contains(&a.tech_lead));
        assert!((0.0..=1.0).contains(&a.feasibility));
        assert!((0.0..=1.0).contains(&a.confidence));
        assert_eq!(a.last_updated_at, 100);
        assert_eq!(a.objective_precondition_summary.total, 1);
        assert!(a.objective_precondition_summary.weighted_satisfaction >= 0.0);
    }

    #[test]
    fn production_diversity_zero_on_single_resource() {
        let v = ResourceVector {
            minerals: 100.0,
            ..Default::default()
        };
        assert!((compute_production_diversity(&v) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn production_diversity_high_when_balanced() {
        let v = ResourceVector {
            minerals: 20.0,
            energy: 20.0,
            food: 20.0,
            research: 20.0,
            authority: 20.0,
        };
        let d = compute_production_diversity(&v);
        // HHI = 5 * (0.2)^2 = 0.2 → diversity 0.8
        assert!((d - 0.8).abs() < 1e-6);
    }
}
