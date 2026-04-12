//! Startup-time schema declarations for the AI bus.
//!
//! The bus requires every metric / command / evidence kind to be declared
//! before values can be emitted. This module centralises those
//! declarations so downstream systems can assume the schema is available
//! by the time `Update` runs.
//!
//! # Tier 1 catalogue (#198)
//!
//! #198 reinterprets the pre-bus `ValueExpr` / `Condition` atom spec into
//! the current bus architecture. Instead of enum variants, the
//! catalogue is expressed as **string-identified topics** on
//! `AiBus`: metrics (engine-emitted time-series), commands (AI-issued
//! actions), and evidence (faction-on-faction observations). `ai_core`
//! consumes these through generic atoms (`ValueExpr::Metric`,
//! `ValueExpr::DelT`, `ConditionAtom::Compare`, …) so no new atoms are
//! needed here.
//!
//! This module:
//!
//! - Exposes canonical IDs through [`ids::metric`], [`ids::command`],
//!   [`ids::evidence`] so downstream systems reference the same strings
//!   across the codebase.
//! - Declares the Tier 1 topics on `AiBus` at `Startup` so emit logic
//!   added later (#204 FleetCombatCapability, …) finds the topic already
//!   present.
//!
//! The declarations themselves do **not** emit any values — populating
//! the topics is the responsibility of per-capability producer systems
//! registered under `AiTickSet::MetricProduce`. Topics that currently
//! lack a producer are still declared so evaluators see a consistent
//! `Missing` instead of a warning.
//!
//! For the full human-readable catalogue (including meaning, units, and
//! expected producers) see `docs/ai-atom-reference.md`.
//!
//! # Deferred to later issues
//!
//! - **Foreign faction metrics** (Tier 2 — light-speed delayed via
//!   `KnowledgeStore`) are listed in the doc but not declared here;
//!   those topics are per-observer × per-target and will be keyed by a
//!   different naming convention decided alongside #193 standing.
//! - **Composite assessment atoms** (`ThreatLevel`, `ConquerFeasibility`,
//!   …) are Lua-composed from the Tier 1 metrics via `ValueExpr` trees
//!   (#130 binding) and do not warrant dedicated bus topics.
//! - **Historical / trajectory atoms** (`FactionStrengthDeltaOverTime`)
//!   are expressed through `ValueExpr::DelT` / `WindowAvg` on the
//!   existing time-series, so no new topic IDs are required.

use bevy::prelude::*;
use macrocosmo_ai::{
    AiBus, CommandSpec, EvidenceSpec, MetricSpec, MetricType, Retention,
};

use crate::ai::plugin::AiBusResource;

pub mod foreign;
pub mod ids;

/// Declare every metric / command / evidence topic used by the engine.
///
/// Runs once in `Startup` via [`AiPlugin`](crate::ai::AiPlugin).
pub fn declare_all(mut bus: ResMut<AiBusResource>) {
    declare_metrics(&mut bus.0);
    declare_commands(&mut bus.0);
    declare_evidence(&mut bus.0);
}

/// Helper: construct a `MetricSpec` for any `MetricType` (spec only exposes
/// `::gauge` / `::ratio` constructors).
fn spec(kind: MetricType, retention: Retention, description: &'static str) -> MetricSpec {
    MetricSpec {
        kind,
        retention,
        description: description.into(),
    }
}

// --------------------------------------------------------------------------
// Metric declarations — Tier 1
// --------------------------------------------------------------------------

/// Declare every Tier 1 metric topic. Grouped by category for
/// cross-reference with `docs/ai-atom-reference.md`.
fn declare_metrics(bus: &mut AiBus) {
    use ids::metric as m;

    // 1.1 Military — Self -----------------------------------------------
    //
    // Fleet-wide aggregates of the observer's own ships. Real-time on the
    // producing side — the value is recomputed by a per-frame or
    // event-driven system and re-emitted; `DelT` / `WindowAvg` recover
    // historical perspective from the retention window.
    bus.declare_metric(
        m::my_total_ships(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "count of ships owned by the observer faction (all states)",
        ),
    );
    bus.declare_metric(
        m::my_strength(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "aggregate combat power of owned ships (hp + firepower proxy)",
        ),
    );
    bus.declare_metric(
        m::my_fleet_ready(),
        MetricSpec::ratio(
            Retention::Medium,
            "fraction of owned ships currently operable (0..=1)",
        ),
    );
    bus.declare_metric(
        m::my_armor(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "total armor pool across owned fleet",
        ),
    );
    bus.declare_metric(
        m::my_shields(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "total shield pool across owned fleet",
        ),
    );
    bus.declare_metric(
        m::my_shield_regen_rate(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "total shield regeneration per hexadies across owned fleet",
        ),
    );
    bus.declare_metric(
        m::my_vulnerability_score(),
        MetricSpec::ratio(
            Retention::Medium,
            "fleet-wide damage fraction (0 = pristine, 1 = fully damaged)",
        ),
    );
    bus.declare_metric(
        m::my_has_flagship(),
        MetricSpec::ratio(
            Retention::Medium,
            "1.0 iff a flagship entity exists and is operable, else 0.0",
        ),
    );

    // 1.2 Economy — Production ------------------------------------------
    //
    // Net production = final rate produced across the empire per hexadies.
    // One topic per resource; scope is implicitly "observer empire".
    // System-scoped production uses distinct IDs under `net_production_*_system_*`
    // once declared by the per-system producer (not in Tier 1).
    bus.declare_metric(
        m::net_production_minerals(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide mineral production per hexadies (post-modifiers)",
        ),
    );
    bus.declare_metric(
        m::net_production_energy(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide energy production per hexadies (post-modifiers)",
        ),
    );
    bus.declare_metric(
        m::net_production_food(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide food production per hexadies (post-modifiers)",
        ),
    );
    bus.declare_metric(
        m::net_production_research(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide research flow per hexadies (flow not stock)",
        ),
    );
    bus.declare_metric(
        m::net_production_authority(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide authority generation per hexadies",
        ),
    );
    bus.declare_metric(
        m::food_consumption_rate(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide food consumption per hexadies (population × per-pop demand)",
        ),
    );
    bus.declare_metric(
        m::food_surplus(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "food production minus food consumption (positive = security)",
        ),
    );

    // 1.3 Economy — Stockpiles ------------------------------------------
    //
    // Stockpiles are owned by star systems (see CLAUDE.md
    // "ResourceStockpile on StarSystem"); the empire-wide metric is the
    // sum across owned systems. Counter semantics are not appropriate
    // because stockpiles can decrease — they are gauges.
    bus.declare_metric(
        m::stockpile_minerals(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide mineral stockpile (sum across owned systems)",
        ),
    );
    bus.declare_metric(
        m::stockpile_energy(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide energy stockpile (sum across owned systems)",
        ),
    );
    bus.declare_metric(
        m::stockpile_food(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide food stockpile (sum across owned systems)",
        ),
    );
    bus.declare_metric(
        m::stockpile_authority(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide authority stockpile (may be signed)",
        ),
    );
    bus.declare_metric(
        m::stockpile_ratio_minerals(),
        MetricSpec::ratio(
            Retention::Medium,
            "minerals stockpile / capacity (0..=1, saturated)",
        ),
    );
    bus.declare_metric(
        m::stockpile_ratio_energy(),
        MetricSpec::ratio(
            Retention::Medium,
            "energy stockpile / capacity (0..=1, saturated)",
        ),
    );
    bus.declare_metric(
        m::stockpile_ratio_food(),
        MetricSpec::ratio(
            Retention::Medium,
            "food stockpile / capacity (0..=1, saturated)",
        ),
    );
    bus.declare_metric(
        m::total_authority_debt(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "sum of negative authority balances across the empire (>= 0)",
        ),
    );

    // 1.4 Population ----------------------------------------------------
    bus.declare_metric(
        m::population_total(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide population count",
        ),
    );
    bus.declare_metric(
        m::population_growth_rate(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide population change per hexadies",
        ),
    );
    bus.declare_metric(
        m::population_carrying_capacity(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "empire-wide carrying capacity (sum across habitable colonies)",
        ),
    );
    bus.declare_metric(
        m::population_ratio(),
        MetricSpec::ratio(
            Retention::Medium,
            "population / carrying capacity (>=1 means overpopulated)",
        ),
    );

    // 1.5 Territory -----------------------------------------------------
    bus.declare_metric(
        m::colony_count(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "count of colonies owned by the observer faction",
        ),
    );
    bus.declare_metric(
        m::colonized_system_count(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "count of star systems with sovereignty.owner == observer",
        ),
    );
    bus.declare_metric(
        m::border_system_count(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "owned systems adjacent to another faction's territory",
        ),
    );
    bus.declare_metric(
        m::habitable_systems_known(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "count of known habitable systems (habitability > 0.3) in KnowledgeStore",
        ),
    );
    bus.declare_metric(
        m::colonizable_systems_remaining(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "habitable known systems not currently controlled by anyone",
        ),
    );
    bus.declare_metric(
        m::systems_with_hostiles(),
        spec(
            MetricType::Gauge,
            Retention::Medium,
            "owned or observed systems with detected hostile presence",
        ),
    );

    // 1.6 Technology ----------------------------------------------------
    bus.declare_metric(
        m::tech_total_researched(),
        spec(
            MetricType::Gauge,
            Retention::VeryLong,
            "count of techs the observer has researched (can drop on rollback)",
        ),
    );
    bus.declare_metric(
        m::tech_completion_percent(),
        MetricSpec::ratio(
            Retention::VeryLong,
            "researched / total-available techs (0..=1)",
        ),
    );
    bus.declare_metric(
        m::tech_unlocks_available(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "techs whose prerequisites are met but are not yet researched",
        ),
    );
    bus.declare_metric(
        m::research_output_ratio(),
        MetricSpec::ratio(
            Retention::Medium,
            "current research flow / cost of active research (0..=1 est.)",
        ),
    );

    // 1.7 Infrastructure ------------------------------------------------
    bus.declare_metric(
        m::systems_with_shipyard(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "count of owned systems with a functioning shipyard",
        ),
    );
    bus.declare_metric(
        m::systems_with_port(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "count of owned systems with a functioning port",
        ),
    );
    bus.declare_metric(
        m::max_building_slots(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "total building slots across the empire",
        ),
    );
    bus.declare_metric(
        m::used_building_slots(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "currently occupied building slots across the empire",
        ),
    );
    bus.declare_metric(
        m::free_building_slots(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "max - used building slots (>= 0)",
        ),
    );
    bus.declare_metric(
        m::can_build_ships(),
        MetricSpec::ratio(
            Retention::Medium,
            "1.0 iff at least one owned shipyard is operational, else 0.0",
        ),
    );

    // 1.8 Meta / Time ---------------------------------------------------
    bus.declare_metric(
        m::game_elapsed_time(),
        spec(
            MetricType::Counter,
            Retention::VeryLong,
            "GameClock.elapsed in hexadies (monotonically increasing)",
        ),
    );
    bus.declare_metric(
        m::number_of_allies(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "count of factions with whom the observer has Alliance standing",
        ),
    );
    bus.declare_metric(
        m::number_of_enemies(),
        spec(
            MetricType::Gauge,
            Retention::Long,
            "count of factions with whom the observer is at war",
        ),
    );
}

// --------------------------------------------------------------------------
// Command declarations — Tier 1
// --------------------------------------------------------------------------

/// Declare every Tier 1 command kind. Command payloads carry structured
/// data; the command *kind* is just a category string.
fn declare_commands(bus: &mut AiBus) {
    use ids::command as c;

    // Military
    bus.declare_command(
        c::attack_target(),
        CommandSpec::new("engage a hostile target system or fleet"),
    );
    bus.declare_command(
        c::reposition(),
        CommandSpec::new("move a fleet to a tactical position"),
    );
    bus.declare_command(
        c::retreat(),
        CommandSpec::new("withdraw a fleet from engagement"),
    );
    bus.declare_command(
        c::blockade(),
        CommandSpec::new("impose a blockade on a target system"),
    );
    bus.declare_command(
        c::fortify_system(),
        CommandSpec::new("build defensive infrastructure in an owned system"),
    );

    // Expansion / infrastructure
    bus.declare_command(
        c::colonize_system(),
        CommandSpec::new("dispatch a colony ship to establish a new colony"),
    );
    bus.declare_command(
        c::build_ship(),
        CommandSpec::new("queue a ship design for construction at a shipyard"),
    );
    bus.declare_command(
        c::build_structure(),
        CommandSpec::new("queue a building / structure at a target system or planet"),
    );
    bus.declare_command(
        c::survey_system(),
        CommandSpec::new("dispatch a surveyor to an unexplored system"),
    );

    // Research
    bus.declare_command(
        c::research_focus(),
        CommandSpec::new("switch empire research focus to a target branch or tech"),
    );

    // Diplomacy
    bus.declare_command(
        c::declare_war(),
        CommandSpec::new("formally declare war on a target faction"),
    );
    bus.declare_command(
        c::seek_peace(),
        CommandSpec::new("initiate peace negotiations with a target faction"),
    );
    bus.declare_command(
        c::propose_alliance(),
        CommandSpec::new("offer alliance to a target faction"),
    );
    bus.declare_command(
        c::establish_relation(),
        CommandSpec::new("establish or change a diplomatic relation state"),
    );
}

// --------------------------------------------------------------------------
// Evidence declarations — Tier 1
// --------------------------------------------------------------------------

/// Declare every Tier 1 evidence kind. Evidence retention is generally
/// longer than metric retention because standing decay operates on
/// event-level history (see #193).
fn declare_evidence(bus: &mut AiBus) {
    use ids::evidence as e;

    // Hostile (positive base_weight in StandingConfig — shifts toward distrust)
    bus.declare_evidence(
        e::direct_attack(),
        EvidenceSpec::new(
            Retention::VeryLong,
            "target faction directly attacked observer's assets",
        ),
    );
    bus.declare_evidence(
        e::system_seized(),
        EvidenceSpec::new(
            Retention::VeryLong,
            "target faction took control of a system formerly owned by the observer",
        ),
    );
    bus.declare_evidence(
        e::border_incursion(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction ship observed near the observer's border",
        ),
    );
    bus.declare_evidence(
        e::hostile_buildup_near(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction strength rose sharply close to the observer",
        ),
    );
    bus.declare_evidence(
        e::blockade_imposed(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction sustained a blockade on an observer-owned system",
        ),
    );
    bus.declare_evidence(
        e::hostile_engagement(),
        EvidenceSpec::new(
            Retention::VeryLong,
            "observer engaged the target faction in combat",
        ),
    );
    bus.declare_evidence(
        e::fleet_loss(),
        EvidenceSpec::new(
            Retention::VeryLong,
            "observer lost fleet assets (attributable to target)",
        ),
    );

    // Friendly (negative base_weight — shifts toward trust)
    bus.declare_evidence(
        e::gift_given(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction transferred resources or tech to observer",
        ),
    );
    bus.declare_evidence(
        e::trade_agreement_established(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction signed a formal trade agreement with observer",
        ),
    );
    bus.declare_evidence(
        e::alliance_with_observer(),
        EvidenceSpec::new(
            Retention::VeryLong,
            "target faction is in a standing alliance with the observer",
        ),
    );
    bus.declare_evidence(
        e::support_against_enemy(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction joined the observer's war or attacked a shared enemy",
        ),
    );
    bus.declare_evidence(
        e::military_withdrawal(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction pulled strength back from the observer's border",
        ),
    );

    // Ambiguous (interpretation modulated by StandingConfig::ambiguous=true)
    bus.declare_evidence(
        e::major_military_buildup(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction's strength grew sharply; intent unclear",
        ),
    );
    bus.declare_evidence(
        e::border_colonization(),
        EvidenceSpec::new(
            Retention::Long,
            "target faction colonised a system close to the observer's territory",
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use macrocosmo_ai::WarningMode;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // declare_all is a Bevy system that takes `ResMut<AiBusResource>`.
        // Build a bus with Silent mode first so re-declare in any nested
        // test won't pollute test output.
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        app.add_systems(Startup, declare_all);
        app.update();
        app
    }

    #[test]
    fn tier1_metrics_are_declared() {
        let a = app();
        let bus = a.world().resource::<AiBusResource>();
        // Spot-check one from each category.
        assert!(bus.has_metric(&ids::metric::my_strength()));
        assert!(bus.has_metric(&ids::metric::net_production_minerals()));
        assert!(bus.has_metric(&ids::metric::stockpile_energy()));
        assert!(bus.has_metric(&ids::metric::population_total()));
        assert!(bus.has_metric(&ids::metric::colony_count()));
        assert!(bus.has_metric(&ids::metric::tech_total_researched()));
        assert!(bus.has_metric(&ids::metric::systems_with_shipyard()));
        assert!(bus.has_metric(&ids::metric::game_elapsed_time()));
    }

    #[test]
    fn tier1_commands_are_declared() {
        let a = app();
        let bus = a.world().resource::<AiBusResource>();
        assert!(bus.has_command_kind(&ids::command::attack_target()));
        assert!(bus.has_command_kind(&ids::command::colonize_system()));
        assert!(bus.has_command_kind(&ids::command::research_focus()));
        assert!(bus.has_command_kind(&ids::command::declare_war()));
    }

    #[test]
    fn tier1_evidence_are_declared() {
        let a = app();
        let bus = a.world().resource::<AiBusResource>();
        assert!(bus.has_evidence_kind(&ids::evidence::direct_attack()));
        assert!(bus.has_evidence_kind(&ids::evidence::gift_given()));
        assert!(bus.has_evidence_kind(&ids::evidence::major_military_buildup()));
    }

    #[test]
    fn metric_ids_are_stable_across_calls() {
        // `MetricId::from(&str)` allocates new `Arc<str>` each time but the
        // underlying string is equal, so id equality must hold.
        assert_eq!(ids::metric::my_strength(), ids::metric::my_strength());
        assert_eq!(
            ids::metric::my_strength().as_str(),
            "my_strength",
        );
    }
}
