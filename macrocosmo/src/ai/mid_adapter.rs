//! Mid-layer game adapter — bridges Bevy world state into the
//! engine-agnostic Mid logic (#448 PR2b scaffold).
//!
//! [`MidGameAdapter`] is the read-only interface
//! [`super::mid_stance::MidStanceAgent`] consumes. PR2c ports Rules 1
//! and 5a from [`super::npc_decision::SimpleNpcPolicy`] behind the
//! [`AiPolicyMode::Layered`] flag while a parity test
//! (`tests/ai_layered_parity.rs`) keeps both paths in lock-step.
//!
//! The identity arbiter ([`arbitrate`]) strips [`Locality`] and
//! returns the inner [`Command`]s; #467 phase 2 replaces it with a
//! real FCFS arbiter.

use bevy::prelude::*;
use macrocosmo_ai::{Command, FactionId, Proposal, Stance};

use super::npc_decision::NpcContext;
use crate::ai::convert::to_ai_faction;
use crate::ai::plugin::AiBusResource;

/// Runtime gate selecting between the legacy
/// [`super::npc_decision::SimpleNpcPolicy`] (today's NPC behavior,
/// all 8 rules in `npc_decision.rs`) and the new layered
/// `MidStanceAgent` path (#448 PR2c+ — Rules 1 + 5a today). Default
/// [`AiPolicyMode::Legacy`] so all production paths and existing
/// tests are untouched until the parity period closes in PR3.
#[derive(Resource, Debug, Clone, Copy, Reflect, Default, PartialEq, Eq)]
#[reflect(Resource)]
pub enum AiPolicyMode {
    /// Existing [`super::npc_decision::SimpleNpcPolicy`] path runs
    /// unchanged. Default until PR3 flips the switch.
    #[default]
    Legacy,
    /// New layered `MidStanceAgent` path. PR2c emits Rules 1 + 5a
    /// only; PR2d ports the remaining rules. Behind the parity test
    /// until PR3.
    Layered,
}

/// What the Mid layer can read about the game world. Bevy-agnostic
/// in spirit — the trait stays decoupled from `Query` / `Resource`
/// internals, so future engine-agnostic Mid logic can be tested with
/// stub adapters. [`BevyMidGameAdapter`] is the concrete production
/// impl.
///
/// All read-only. Returning slices keeps the trait allocation-free
/// at the call site for the common "iterate this list" pattern.
pub trait MidGameAdapter {
    /// Faction the adapter is currently scoped to.
    fn faction(&self) -> Entity;

    /// Systems known-hostile from this faction's perspective. Mirrors
    /// `NpcContext.hostile_systems`. Used by Rule 1 (attack target).
    fn hostile_systems(&self) -> &[Entity];

    /// Idle ship entities currently flagged as `is_combat`. Rule 1
    /// fires only when both `hostile_systems` and this list are
    /// non-empty.
    fn idle_combat_ships(&self) -> Vec<Entity>;

    /// `true` when the empire's Ruler is **not** aboard a ship —
    /// i.e. eligible for a `move_ruler` follow-up emit. Mirrors
    /// `NpcContext.ruler_aboard == false && ruler_entity.is_some()`.
    fn ruler_movable(&self) -> bool;

    /// Per-faction `can_build_ships` metric (0.0 / 1.0 today). Rule
    /// 5a fires only when this is below 1.0 — i.e. the empire still
    /// lacks a usable shipyard.
    fn can_build_ships(&self) -> f64;

    /// Per-faction `systems_with_core` metric. Rule 5a's #370 gate:
    /// without a Core deployed somewhere, the build_structure handler
    /// has nowhere to emplace the shipyard, so we keep the policy
    /// silent.
    fn systems_with_core(&self) -> f64;

    /// Per-faction `colony_count` metric. Rule 5a additionally
    /// requires at least one colony — a Core-only empire with no
    /// colony is not yet ready for a shipyard.
    fn colony_count(&self) -> f64;
}

/// Bevy implementation of [`MidGameAdapter`]. Wraps a borrow of the
/// per-tick `NpcContext` and the bus so Rule ports can read the same
/// signals the legacy `SimpleNpcPolicy::decide` reads, without
/// duplicating the ECS scan.
///
/// Lifetimes are explicit because `NpcContext` and `AiBus` are both
/// owned higher up the call stack (in `npc_decision_tick`); the
/// adapter just hands views into them.
pub struct BevyMidGameAdapter<'a> {
    pub faction: Entity,
    pub context: &'a NpcContext,
    pub bus: &'a macrocosmo_ai::AiBus,
    /// Pre-computed `idle_combat` set (matches the same expression
    /// `SimpleNpcPolicy::decide` builds). Borrowed so the adapter
    /// stays cheap to construct per-empire.
    pub idle_combat: &'a [Entity],
}

impl<'a> BevyMidGameAdapter<'a> {
    /// Convenience: faction id derived from the faction entity. Used
    /// by `MidStanceAgent::decide` to look up per-faction metrics on
    /// the bus.
    pub fn faction_id(&self) -> FactionId {
        to_ai_faction(self.faction)
    }
}

impl<'a> MidGameAdapter for BevyMidGameAdapter<'a> {
    fn faction(&self) -> Entity {
        self.faction
    }

    fn hostile_systems(&self) -> &[Entity] {
        &self.context.hostile_systems
    }

    fn idle_combat_ships(&self) -> Vec<Entity> {
        self.idle_combat.to_vec()
    }

    fn ruler_movable(&self) -> bool {
        !self.context.ruler_aboard && self.context.ruler_entity.is_some()
    }

    fn can_build_ships(&self) -> f64 {
        self.bus
            .current(&crate::ai::schema::ids::metric::for_faction(
                "can_build_ships",
                self.faction_id(),
            ))
            .unwrap_or(0.0)
    }

    fn systems_with_core(&self) -> f64 {
        self.bus
            .current(&crate::ai::schema::ids::metric::for_faction(
                "systems_with_core",
                self.faction_id(),
            ))
            .unwrap_or(0.0)
    }

    fn colony_count(&self) -> f64 {
        self.bus
            .current(&crate::ai::schema::ids::metric::for_faction(
                "colony_count",
                self.faction_id(),
            ))
            .unwrap_or(0.0)
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

/// Layered-mode decision entry-point. PR2c routes through
/// [`super::mid_stance::MidStanceAgent::decide`]; the `Stance`
/// argument is `Stance::default()` until PR3 wires
/// [`macrocosmo_ai::MidTermState`] persistence into the tick system
/// (Plan agent micro-decision 5).
pub fn layered_decide<A: MidGameAdapter>(
    adapter: &A,
    stance: &Stance,
    faction_id: &str,
    now: i64,
) -> Vec<Proposal> {
    super::mid_stance::MidStanceAgent::decide(adapter, stance, faction_id, now)
}

/// Helper used by `npc_decision_tick`'s Layered branch to materialise
/// the bus reference (the system holds a `ResMut<AiBusResource>` and
/// would otherwise need an explicit re-borrow at the call site).
pub fn bus_from_resource(bus: &AiBusResource) -> &macrocosmo_ai::AiBus {
    &bus.0
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
}
