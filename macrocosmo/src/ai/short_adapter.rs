//! Short-layer game adapter — bridges Bevy world state into the
//! engine-agnostic Short logic (#449 PR2d).
//!
//! [`ShortGameAdapter`] is the read-only interface
//! [`super::short_stance::ShortStanceAgent`] consumes. With #449 PR2d,
//! Rules 2 (survey) and 5b (slot fill) move out of `MidStanceAgent`
//! and onto per-`ShortAgent` reasoning: a `Fleet`-scope agent decides
//! Rule 2 over its own ships, a `ColonizedSystem`-scope agent decides
//! Rule 5b over its own colony.
//!
//! The trait is engine-agnostic in spirit — [`BevyShortAgentAdapter`]
//! is the concrete production impl that proxies into a per-tick
//! [`super::npc_decision::NpcContext`] plus precomputed Bevy data
//! (idle_surveyor set, fleet ship list, colony slot count).

use bevy::prelude::Entity;

use crate::ai::short_agent::ShortScope;

/// What the Short layer can read about the game world. Both
/// fleet-scope and colonized-system-scope methods live on the same
/// trait so the same `ShortStanceAgent::decide` entry point can branch
/// off `scope()` without duplicate trait machinery.
///
/// Methods irrelevant to the active scope return empty / zero — the
/// agent's `match` on [`ShortScope`] gates which methods get
/// consulted.
pub trait ShortGameAdapter {
    /// Owning empire entity (= the agent's Mid → Region → empire chain).
    fn empire(&self) -> Entity;
    /// What this agent is bound to (Fleet vs ColonizedSystem).
    fn scope(&self) -> &ShortScope;

    // ---- Fleet scope ----

    /// Idle surveyors that belong to this fleet (ships in
    /// [`super::npc_decision::NpcContext::ships`] with `is_idle &&
    /// can_survey` AND `Ship.fleet == Some(self.scope.fleet)`).
    /// Empire-level dedup against in-flight `survey_system` commands
    /// (Bug A union of `PendingAssignment` + `AiCommandOutbox`) is
    /// applied **upstream** in `npc_decision_tick`; this slice is the
    /// final input.
    fn idle_surveyors(&self) -> &[Entity];

    /// Survey targets reachable from this fleet's region. Already
    /// ranked by [`super::npc_decision::rank_survey_targets`] and
    /// deduped against in-flight `survey_system` work — Mid's PR2c
    /// upstream still owns this filter chain (`pending_survey_targets`
    /// in `npc_decision.rs`), so the per-fleet ShortAgent simply
    /// slices into the same set.
    fn unsurveyed_targets(&self) -> &[Entity];

    // ---- ColonizedSystem scope ----

    /// Free building slot count for this colony. Today proxied through
    /// the empire-wide `free_building_slots` bus metric — single-colony
    /// empires (the sentinel test) see colony==empire, multi-colony
    /// empires see the empire total. Per-colony narrowing lands when
    /// `BuildingsBuilder` exposes a per-system metric (future PR).
    fn free_building_slots(&self) -> f64;
    /// Net energy production for this colony. Same proxy story as
    /// [`Self::free_building_slots`].
    fn net_production_energy(&self) -> f64;
    /// Net food production for this colony. Same proxy story as
    /// [`Self::free_building_slots`].
    fn net_production_food(&self) -> f64;
}

/// Bevy implementation of [`ShortGameAdapter`]. Constructed
/// per-`ShortAgent` per tick from the shared `NpcContext` produced by
/// `npc_decision_tick` plus per-scope data resolved at call time.
///
/// Lifetimes are explicit so the borrow checker stays happy: the
/// caller (`run_short_agents`) owns the underlying `NpcContext`,
/// `AiBus`, and the precomputed slices, and the adapter just hands
/// out views.
pub struct BevyShortAgentAdapter<'a> {
    pub empire: Entity,
    pub scope: ShortScope,
    /// Pre-filtered idle surveyor list scoped to this fleet (empty for
    /// non-Fleet scopes). Owned by the caller so `idle_surveyors()`
    /// returns a borrow.
    pub idle_surveyors: &'a [Entity],
    /// Survey targets reachable by this fleet (empty for non-Fleet
    /// scopes). Same upstream dedup as the Mid-side `unsurveyed_systems`
    /// (`pending_survey_targets` filter applied in `npc_decision_tick`).
    pub unsurveyed_targets: &'a [Entity],
    /// Pre-resolved metrics for ColonizedSystem scope (zero for
    /// non-ColonizedSystem scopes).
    pub free_building_slots: f64,
    pub net_production_energy: f64,
    pub net_production_food: f64,
}

impl<'a> ShortGameAdapter for BevyShortAgentAdapter<'a> {
    fn empire(&self) -> Entity {
        self.empire
    }

    fn scope(&self) -> &ShortScope {
        &self.scope
    }

    fn idle_surveyors(&self) -> &[Entity] {
        self.idle_surveyors
    }

    fn unsurveyed_targets(&self) -> &[Entity] {
        self.unsurveyed_targets
    }

    fn free_building_slots(&self) -> f64 {
        self.free_building_slots
    }

    fn net_production_energy(&self) -> f64 {
        self.net_production_energy
    }

    fn net_production_food(&self) -> f64 {
        self.net_production_food
    }
}
