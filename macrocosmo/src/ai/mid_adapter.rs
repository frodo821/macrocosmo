//! Mid-layer game adapter — bridges Bevy world state into the
//! engine-agnostic Mid logic (#448 PR2b scaffold).
//!
//! [`MidGameAdapter`] is the read-only interface a future
//! `MidStanceAgent` (#448 PR2c+) consumes. Today the trait exists
//! and a Bevy implementation is wired through `npc_decision_tick`
//! behind the [`AiPolicyMode::Layered`] flag, but the Layered branch
//! emits an empty `Vec<Proposal>` — no behavior change vs. the
//! Legacy [`super::npc_decision::SimpleNpcPolicy`] path.
//!
//! The identity arbiter ([`arbitrate`]) strips [`Locality`] and
//! returns the inner [`Command`]s; #467 phase 2 replaces it with a
//! real FCFS arbiter.

use bevy::prelude::*;
use macrocosmo_ai::{Command, Proposal};

/// Runtime gate selecting between the legacy
/// [`super::npc_decision::SimpleNpcPolicy`] (today's NPC behavior,
/// all 8 rules in `npc_decision.rs`) and the new layered
/// `MidStanceAgent` path (#448 PR2c+ — empty today). Default
/// [`AiPolicyMode::Legacy`] so all production paths and existing
/// tests are untouched until the parity period closes in PR3.
#[derive(Resource, Debug, Clone, Copy, Reflect, Default, PartialEq, Eq)]
#[reflect(Resource)]
pub enum AiPolicyMode {
    /// Existing [`super::npc_decision::SimpleNpcPolicy`] path runs
    /// unchanged. Default until PR3 flips the switch.
    #[default]
    Legacy,
    /// New layered `MidStanceAgent` path. PR2b emits zero
    /// proposals; PR2c/2d port the rules from
    /// [`super::npc_decision::SimpleNpcPolicy`] one at a time.
    Layered,
}

/// What the Mid layer can read about the game world. Bevy-agnostic
/// — keeps the future engine-agnostic Mid logic decoupled from
/// `Query` / `Resource` details. [`BevyMidGameAdapter`] provides
/// the concrete Bevy implementation; tests can stub the trait
/// directly.
///
/// Methods deliberately return owned data so the trait stays
/// object-safe and the future `MidStanceAgent` can be tested
/// without an `App`. The cost is one allocation per call site per
/// faction per tick — negligible vs. the per-tick metric scan.
pub trait MidGameAdapter {
    /// Faction the adapter is currently scoped to.
    fn faction(&self) -> Entity;
    // PR2c+ adds methods like:
    //   fn hostile_systems(&self) -> Vec<Entity>;
    //   fn unsurveyed_systems(&self) -> Vec<Entity>;
    //   fn idle_ships(&self) -> Vec<ShipInfo>;
    // For PR2b we leave the trait near-empty — the Layered branch
    // emits no proposals so it has nothing to read yet.
}

/// Bevy implementation of [`MidGameAdapter`]. Today carries only
/// the faction handle; PR2c expands it to wrap the same data
/// `npc_decision_tick`'s
/// [`super::npc_decision::NpcContext`] already exposes.
pub struct BevyMidGameAdapter {
    pub faction: Entity,
}

impl MidGameAdapter for BevyMidGameAdapter {
    fn faction(&self) -> Entity {
        self.faction
    }
}

/// Identity arbiter — strips [`macrocosmo_ai::Locality`], returns
/// the inner [`Command`]s in the order they were proposed. The
/// single-Mid case has no real conflicts, so every proposal
/// trivially commits. #467 phase 2 replaces this with FCFS
/// arbitration (commitment registry, light-speed-delayed
/// [`macrocosmo_ai::ProposalOutcome`]).
pub fn arbitrate(proposals: Vec<Proposal>) -> Vec<Command> {
    proposals.into_iter().map(|p| p.command).collect()
}

/// Layered-mode decision entry-point. PR2b is a no-op gate —
/// Layered mode emits zero proposals until PR2c starts porting
/// rules. Keeping this function present (rather than inlining the
/// `match` in `npc_decision_tick`) gives PR2c a clean extension
/// point.
pub fn layered_decide_noop(_adapter: &BevyMidGameAdapter) -> Vec<Proposal> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use macrocosmo_ai::{CommandKindId, FactionId, Locality, SystemRef};

    #[test]
    fn ai_policy_mode_defaults_to_legacy() {
        assert_eq!(AiPolicyMode::default(), AiPolicyMode::Legacy);
    }

    #[test]
    fn arbitrate_strips_locality_and_preserves_order() {
        let issuer = FactionId(7);
        let cmd_a = Command::new(CommandKindId::from("a"), issuer, 10);
        let cmd_b = Command::new(CommandKindId::from("b"), issuer, 11);
        let cmd_c = Command::new(CommandKindId::from("c"), issuer, 12);

        let proposals = vec![
            Proposal::faction_wide(cmd_a.clone()),
            Proposal::at_system(cmd_b.clone(), SystemRef(42)),
            Proposal {
                command: cmd_c.clone(),
                locality: Locality::FactionWide,
            },
        ];

        let commands = arbitrate(proposals);

        assert_eq!(commands.len(), 3, "arbitrate must preserve every proposal");
        assert_eq!(commands[0], cmd_a, "order must match input order");
        assert_eq!(commands[1], cmd_b);
        assert_eq!(commands[2], cmd_c);
    }

    #[test]
    fn layered_decide_noop_returns_empty() {
        let adapter = BevyMidGameAdapter {
            faction: Entity::from_raw_u32(1).unwrap(),
        };
        assert!(
            layered_decide_noop(&adapter).is_empty(),
            "PR2b Layered branch must emit zero proposals"
        );
    }

    #[test]
    fn bevy_mid_game_adapter_exposes_faction() {
        let e = Entity::from_raw_u32(99).unwrap();
        let adapter = BevyMidGameAdapter { faction: e };
        assert_eq!(adapter.faction(), e);
    }
}
