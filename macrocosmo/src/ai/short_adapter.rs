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
    ///
    /// #469: kept for diagnostics / fallback only — Rule 2 emission
    /// now consumes [`Self::survey_assignments`] (pre-paired
    /// `(ship, target)` tuples computed with ship-relative ETA).
    fn unsurveyed_targets(&self) -> &[Entity];

    /// #469: Greedy `(ship, target)` assignments produced by
    /// `npc_decision_tick` for this fleet using ship-relative ETA
    /// ranking. `ShortStanceAgent`'s Fleet branch emits one
    /// `survey_system` command per pair, replacing the legacy
    /// `surveyors.zip(targets)` pattern that pre-#469 ignored ship
    /// position and FTL geometry.
    ///
    /// Empty slice for non-Fleet scopes and for Fleet scopes with no
    /// reachable targets after dedup.
    fn survey_assignments(&self) -> &[(Entity, Entity)];

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

    /// Hotfix-3 resource gate: `true` when the empire's combined
    /// stockpile can fund `building_id`'s minerals + energy cost
    /// RIGHT NOW. Soft gate — permits deficit spending as long as
    /// the stockpile is non-zero. Used by Rule 5b (slot fill —
    /// `power_plant` / `farm` / `mine`) so a minerals-starved
    /// colony stops stacking orders the build queue cannot drain.
    ///
    /// **Default `true`** = preserves StubAdapter behaviour.
    fn can_afford_building(&self, _building_id: &str) -> bool {
        true
    }
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
    /// #469: Pre-paired `(ship, target)` greedy assignments produced
    /// by `npc_decision_tick` for this fleet. Empty slice for non-Fleet
    /// scopes and for Fleet scopes with no reachable targets.
    pub survey_assignments: &'a [(Entity, Entity)],
    /// Pre-resolved metrics for ColonizedSystem scope (zero for
    /// non-ColonizedSystem scopes).
    pub free_building_slots: f64,
    pub net_production_energy: f64,
    pub net_production_food: f64,
    /// Hotfix-3: sum of `ResourceStockpile.minerals` across the
    /// empire's owned systems. Consumed by
    /// [`ShortGameAdapter::can_afford_building`] in Rule 5b. Zero
    /// when the empire has no owned systems (defensive).
    pub current_minerals: macrocosmo_core::amount::Amt,
    /// Hotfix-3: sum of `ResourceStockpile.energy`. See
    /// [`Self::current_minerals`].
    pub current_energy: macrocosmo_core::amount::Amt,
    /// Hotfix-3: building registry borrow. `None` only in test
    /// setups that never load the Lua registry; an unknown
    /// `building_id` returns `true` from the gate so a typo does
    /// not silently suppress an emission (the handler will warn).
    pub building_registry: Option<&'a crate::colony::BuildingRegistry>,
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

    fn survey_assignments(&self) -> &[(Entity, Entity)] {
        self.survey_assignments
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

    fn can_afford_building(&self, building_id: &str) -> bool {
        let Some(registry) = self.building_registry else {
            return true;
        };
        let Some(def) = registry.get(building_id) else {
            return true;
        };
        self.current_minerals >= def.minerals_cost && self.current_energy >= def.energy_cost
    }
}
