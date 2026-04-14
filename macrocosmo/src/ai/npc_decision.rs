//! NPC decision tick — hook point for pluggable per-faction AI policies (#173).
//!
//! `AiPlugin` registers [`npc_decision_tick`] under [`AiTickSet::Reason`].
//! The production policy today is [`NoOpPolicy`], which does nothing per
//! tick. The trait exists so future issues under #189 can swap in
//! `macrocosmo_ai`-backed policies (campaign / Nash / feasibility) without
//! touching the system wiring.
//!
//! Scope note: this module intentionally carries **no** dependency on the
//! optional `macrocosmo_ai::mock` feature. The dev-dependency in
//! `macrocosmo/Cargo.toml` activates `mock` for the integration test
//! binary only, so callers of the production game crate never pay for the
//! feature.
//!
//! See `docs/plan-173-npc-empire-mock-ai.md` for the rollout plan.
//!
//! [`AiTickSet::Reason`]: super::AiTickSet::Reason

use bevy::prelude::*;

use crate::ai::plugin::AiBusResource;
use crate::player::{Empire, Faction, PlayerEmpire};
use crate::time_system::GameClock;

/// Trait implemented by pluggable NPC decision policies. Stateless policies
/// are encouraged; stateful policies can live in a `Resource` and be read
/// from the tick system.
///
/// Phase 1 (#173): this trait is observed but unused — `npc_decision_tick`
/// calls [`NoOpPolicy::tick`] directly. Future issues will route the call
/// through a `Resource<Box<dyn NpcPolicy>>` so Lua-defined per-empire
/// policies can be swapped in.
pub trait NpcPolicy: Send + Sync + 'static {
    /// Called once per `Update` tick per NPC empire. The return value is
    /// intentionally `()` for now — a future revision will return
    /// `Option<macrocosmo_ai::Command>` once intent-based commands exist.
    fn tick(&mut self, faction: &str, now: i64);
}

/// Default policy: do nothing. Keeps the AI bus quiet so `playthrough`
/// recordings remain deterministic and tests can assert "no commands
/// emitted by NPCs" as a baseline.
#[derive(Default, Debug, Clone, Copy)]
pub struct NoOpPolicy;

impl NpcPolicy for NoOpPolicy {
    fn tick(&mut self, _faction: &str, _now: i64) {
        // intentional no-op (#173)
    }
}

/// System run under [`AiTickSet::Reason`](super::AiTickSet::Reason):
/// walk every NPC empire (`Empire` without `PlayerEmpire`) and invoke the
/// currently-configured policy. Today the policy is [`NoOpPolicy`]; the
/// call exists so that wiring regressions (NPCs not iterated, ordering
/// broken) are caught by the integration tests in
/// `tests/npc_empires_in_player_mode.rs`.
///
/// `_bus` is held to keep the parameter list forward-compatible with the
/// future bus-emitting policy without a schema change.
pub fn npc_decision_tick(
    clock: Res<GameClock>,
    _bus: ResMut<AiBusResource>,
    npcs: Query<&Faction, (With<Empire>, Without<PlayerEmpire>)>,
) {
    let now = clock.elapsed;
    let mut policy = NoOpPolicy;
    for faction in &npcs {
        policy.tick(&faction.id, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_op_policy_is_silent() {
        let mut p = NoOpPolicy;
        p.tick("vesk_hegemony", 0);
        p.tick("aurelian_concord", 100);
        // Reaching here without panic is the assertion.
    }
}
