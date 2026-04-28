//! `ShortAgent` Component — per-fleet / per-colonized-system tactical
//! execution agent (#449 PR2c).
//!
//! Cardinality:
//! * One `ShortAgent { scope: ShortScope::Fleet(fleet) }` per `Fleet`
//!   owned by an empire (skip wild / hostile fleets that have no
//!   `Owner::Empire`).
//! * One `ShortAgent { scope: ShortScope::ColonizedSystem(system) }`
//!   per StarSystem in which an empire holds a colony.
//!
//! `managed_by` points at the `MidAgent` entity whose `Region` covers
//! the agent's scope (resolved through the system's
//! `RegionMembership`).
//!
//! State migration: PR2c moves the engine-agnostic `PlanState`
//! (decomposition queue) off `OrchestratorState.plan_states` and onto
//! the `ShortAgent` Component itself, completing the state-on-Component
//! direction begun in PR2a (`EmpireLongTermState`) and PR2b
//! (`MidAgent.state`). Same `#[reflect(ignore)]` pattern as those — the
//! ai-core types stay opaque to reflection (`ai-core-isolation.yml` keeps
//! `macrocosmo-ai` Bevy-free) but persistence travels through postcard.

use bevy::prelude::*;
use macrocosmo_ai::PlanState;

/// Per-execution-unit tactical agent. Lifetime matches its scope: a
/// `ShortAgent` whose Fleet or colonized system disappears is despawned
/// by [`super::short_agent_runtime::despawn_orphaned_short_agents`].
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct ShortAgent {
    /// Owning `MidAgent` entity (the Mid attached to the `Region` that
    /// covers `scope`). Mirrors the `MidAgent.region` ↔ `Region.mid_agent`
    /// pattern from PR2b; resolved at spawn time and re-resolved if the
    /// scope's `RegionMembership` changes (region split / merge — future
    /// PR).
    pub managed_by: Entity,
    /// What this agent executes against — a fleet, or a colonized
    /// system.
    pub scope: ShortScope,
    /// Engine-agnostic execution state (queued primitive commands per
    /// `(macro_kind, objective)` slot, drained one-per-tick by
    /// `CampaignReactiveShort::tick`). Migrated out of
    /// `OrchestratorState.plan_states` in PR2c (state-on-Component).
    #[reflect(ignore)]
    pub state: PlanState,
    /// Player toggle: when `true`, the AI core may freely tick this
    /// agent (= NPC default). When `false`, only player commands flow
    /// through (= player empire default). PR3's per-agent UI (#452)
    /// flips this.
    pub auto_managed: bool,
}

/// What a `ShortAgent` is bound to. The `Entity` payload is the Fleet
/// entity (for `Fleet`) or the StarSystem entity (for `ColonizedSystem`).
/// Bevy's `Reflect` derives an `Entity` field via the standard reflection
/// path used elsewhere in the codebase (e.g. `MidAgent.region`,
/// `RegionMembership.region`).
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortScope {
    /// One `ShortAgent` per owned Fleet.
    Fleet(Entity),
    /// One `ShortAgent` per StarSystem in which we hold a colony.
    ColonizedSystem(Entity),
}
