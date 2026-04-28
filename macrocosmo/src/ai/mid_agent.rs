//! `MidAgent` Component — sub-empire tactical agent (#449 PR2b).
//!
//! Cardinality: **one `MidAgent` per `Region` per Empire**. PR2a
//! spawns exactly one `Region` per Empire (anchored at the empire's
//! `HomeSystem`); PR2b spawns exactly one `MidAgent` per `Region`. The
//! initial wiring is therefore "1 empire → 1 region → 1 mid-agent",
//! which preserves the legacy single-empire-wide Mid behavior — every
//! existing NPC integration test still observes the same emit pattern.
//! Multi-Region splits in #449 PR2c+ activate N MidAgents per empire
//! automatically.
//!
//! State migration: the engine-agnostic `macrocosmo_ai::MidTermState`
//! used to live on `OrchestratorState.mid_state` (single-instance,
//! orchestrator-side). PR2b moves it onto the `MidAgent` Component so
//! per-region state is naturally addressable from the ECS. Same
//! `#[reflect(ignore)]` pattern as `EmpireLongTermState` /
//! `AiBusResource` — the inner state stays opaque to reflection
//! (`ai-core-isolation.yml` keeps `macrocosmo-ai` Bevy-free), but
//! persistence travels through postcard.

use bevy::prelude::*;
use macrocosmo_ai::MidTermState;

/// Tactical agent attached to a `Region`. Owns the engine-agnostic
/// `MidTermState` (stance + active operations) for that region and
/// flags whether NPC reasoning is allowed to drive it.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct MidAgent {
    /// Backref to the owning `Region` entity. Keep in sync with
    /// `Region.mid_agent` — the Region is populated with
    /// `Some(this_entity)` immediately after the `MidAgent` is
    /// spawned in `setup::spawn_initial_region_for_faction`.
    pub region: Entity,
    /// Engine-agnostic tactical state. `MidTermState.region_id` (the
    /// `arc_str_id` form used inside `macrocosmo-ai`) stays `None`
    /// here — the ECS-side `region: Entity` field above is the only
    /// region back-reference the integration layer needs. PR2 of the
    /// AI trait unification (#448 PR4 follow-up) decides whether to
    /// thread the string id through.
    #[reflect(ignore)]
    pub state: MidTermState,
    /// Player toggle: when `true`, NPC reasoning may freely drive the
    /// agent's region (= legacy NPC behavior); when `false`, only
    /// player commands apply (= player empire today). The PR3
    /// in-game UI (#452) flips this on a per-region basis.
    pub auto_managed: bool,
}
