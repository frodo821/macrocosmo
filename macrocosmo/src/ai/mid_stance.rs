//! Mid-layer rule logic — emits Proposals based on game adapter
//! input and the current Stance. Today (#448 PR2c) covers Rule 1
//! (attack hostile + move ruler) and Rule 5a (build shipyard); PR2d
//! extends to Rules 2/3/4/5b/6/7/8.
//!
//! `MidStanceAgent` is parallel to `macrocosmo_ai::IntentDrivenMidTerm`
//! by design (Plan agent micro-decision 3): different responsibility
//! (raw game state vs. parsed Intents). PR4 of #448 unifies them
//! under a single Agent trait.
//!
//! Strict-parity rule: every emit here must mirror the corresponding
//! branch in `super::npc_decision::SimpleNpcPolicy::decide` byte-for-byte
//! (command kind, params, issuer, `at`). The parity test
//! (`tests/ai_layered_parity.rs`) compares canonical command sets
//! between Legacy and Layered modes; introducing a differential here
//! breaks it.

use bevy::prelude::Entity;
use macrocosmo_ai::{Command, CommandValue, Proposal, Stance};

use crate::ai::convert::{to_ai_entity, to_ai_faction, to_ai_system};
use crate::ai::mid_adapter::MidGameAdapter;
use crate::ai::schema::ids::command as cmd_ids;

/// Stateless agent — `decide` is a pure function of `(adapter,
/// stance, faction_id, now)`. State (active operations, region) lives
/// in `macrocosmo_ai::MidTermState` and will be threaded through here
/// once PR4 unifies the Agent trait.
pub struct MidStanceAgent;

impl MidStanceAgent {
    /// Run the mid-layer ruleset for one faction on one tick. Emits
    /// [`Proposal`]s the arbiter ([`super::mid_adapter::arbitrate`])
    /// converts to bare [`Command`]s.
    ///
    /// The `stance` parameter is accepted but not yet consulted —
    /// Rules 1 and 5a fire identically across all stances today.
    /// PR3+ adds stance-dependent priority weighting / proposal
    /// filtering (#467 phase 1).
    pub fn decide<A: MidGameAdapter>(
        adapter: &A,
        stance: &Stance,
        _faction_id: &str,
        now: i64,
    ) -> Vec<Proposal> {
        // PR2c: stance is intentionally unused. Silenced explicitly
        // so a future port can find this site by grep.
        let _ = stance;

        let mut proposals = Vec::new();
        let faction_entity = adapter.faction();
        let faction_id = to_ai_faction(faction_entity);

        // ----- Rule 1: Attack hostiles + follow-up move_ruler.
        // Mirrors `SimpleNpcPolicy::decide` Rule 1 exactly: requires
        // both a known hostile system and at least one idle combat
        // ship; targets `hostile_systems[0]`; param shape is
        // `target_system` + `ship_count` + `ship_<i>` per ship.
        let idle_combat = adapter.idle_combat_ships();
        if let Some(&target) = adapter.hostile_systems().first()
            && !idle_combat.is_empty()
        {
            let mut cmd = Command::new(cmd_ids::attack_target(), faction_id, now)
                .with_param("target_system", CommandValue::System(to_ai_system(target)))
                .with_param("ship_count", CommandValue::I64(idle_combat.len() as i64));
            for (i, &ship) in idle_combat.iter().enumerate() {
                cmd = cmd.with_param(
                    format!("ship_{i}"),
                    CommandValue::Entity(to_ai_entity(ship)),
                );
            }
            // attack_target is system-targeted; carry the locality so
            // the future FCFS arbiter (#467 phase 2) can detect
            // cross-Mid contention on the same system.
            proposals.push(Proposal::at_system(cmd, to_ai_system(target)));

            // 1b. Follow-up: move the Ruler to the attack target if
            // not already aboard. Same locality as the attack — the
            // pair commits or rejects together.
            if adapter.ruler_movable() {
                let ruler_cmd = Command::new(cmd_ids::move_ruler(), faction_id, now)
                    .with_param("target_system", CommandValue::System(to_ai_system(target)));
                proposals.push(Proposal::at_system(ruler_cmd, to_ai_system(target)));
            }

            // Legacy `SimpleNpcPolicy::decide` early-returns after
            // Rule 1 — combat takes priority over every later rule.
            // PR2c preserves that; PR2d will revisit when Rules 2-8
            // land.
            return proposals;
        }

        // ----- Rule 5a: System building (shipyard).
        // Mirrors `SimpleNpcPolicy::decide` Rule 5a exactly: gated on
        // `can_build_ships < 1.0 && systems_with_core > 0 &&
        // colony_count > 0`. The handler-side dedup absorbs per-tick
        // re-emission while the queue drains.
        let can_build = adapter.can_build_ships();
        let systems_with_core = adapter.systems_with_core();
        let colony_count = adapter.colony_count();
        if can_build < 1.0 && systems_with_core > 0.0 && colony_count > 0.0 {
            let cmd = Command::new(cmd_ids::build_structure(), faction_id, now)
                .with_param("building_id", CommandValue::Str("shipyard".into()));
            // `build_structure` has no `target_system` param — the
            // handler picks an owned system. Faction-wide locality
            // until #449 introduces region routing.
            proposals.push(Proposal::faction_wide(cmd));
        }

        proposals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::convert::from_ai_system;

    /// In-memory stub of [`MidGameAdapter`] for unit tests. Mirrors
    /// the call sites `MidStanceAgent::decide` needs without dragging
    /// in Bevy queries.
    struct StubAdapter {
        faction: Entity,
        hostile_systems: Vec<Entity>,
        idle_combat: Vec<Entity>,
        ruler_movable: bool,
        can_build: f64,
        systems_with_core: f64,
        colony_count: f64,
    }

    impl StubAdapter {
        fn empty() -> Self {
            Self {
                faction: Entity::from_raw_u32(1).unwrap(),
                hostile_systems: vec![],
                idle_combat: vec![],
                ruler_movable: false,
                can_build: 0.0,
                systems_with_core: 0.0,
                colony_count: 0.0,
            }
        }
    }

    impl MidGameAdapter for StubAdapter {
        fn faction(&self) -> Entity {
            self.faction
        }
        fn hostile_systems(&self) -> &[Entity] {
            &self.hostile_systems
        }
        fn idle_combat_ships(&self) -> Vec<Entity> {
            self.idle_combat.clone()
        }
        fn ruler_movable(&self) -> bool {
            self.ruler_movable
        }
        fn can_build_ships(&self) -> f64 {
            self.can_build
        }
        fn systems_with_core(&self) -> f64 {
            self.systems_with_core
        }
        fn colony_count(&self) -> f64 {
            self.colony_count
        }
    }

    #[test]
    fn rule_1_fires_when_hostile_and_idle_combat_present() {
        let target = Entity::from_raw_u32(42).unwrap();
        let ship = Entity::from_raw_u32(100).unwrap();
        let stub = StubAdapter {
            hostile_systems: vec![target],
            idle_combat: vec![ship],
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert_eq!(proposals.len(), 1, "no ruler → only attack proposal");
        assert_eq!(proposals[0].command.kind.as_str(), "attack_target");
        match proposals[0].command.params.get("target_system") {
            Some(CommandValue::System(sys_ref)) => assert_eq!(from_ai_system(*sys_ref), target),
            _ => panic!("expected target_system param"),
        }
    }

    #[test]
    fn rule_1_silent_when_hostile_systems_empty() {
        let stub = StubAdapter {
            idle_combat: vec![Entity::from_raw_u32(100).unwrap()],
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "attack_target"),
            "no hostile → no attack",
        );
    }

    #[test]
    fn rule_1_emits_move_ruler_when_ruler_movable() {
        let target = Entity::from_raw_u32(42).unwrap();
        let ship = Entity::from_raw_u32(100).unwrap();
        let stub = StubAdapter {
            hostile_systems: vec![target],
            idle_combat: vec![ship],
            ruler_movable: true,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert_eq!(proposals.len(), 2, "attack + move_ruler");
        assert_eq!(proposals[0].command.kind.as_str(), "attack_target");
        assert_eq!(proposals[1].command.kind.as_str(), "move_ruler");
    }

    #[test]
    fn rule_5a_fires_when_no_shipyard_and_core_and_colony_present() {
        let stub = StubAdapter {
            can_build: 0.0,
            systems_with_core: 1.0,
            colony_count: 2.0,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].command.kind.as_str(), "build_structure");
        match proposals[0].command.params.get("building_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "shipyard"),
            _ => panic!("expected building_id=shipyard"),
        }
    }

    #[test]
    fn rule_5a_silent_when_shipyard_already_present() {
        let stub = StubAdapter {
            can_build: 1.0,
            systems_with_core: 1.0,
            colony_count: 2.0,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals.is_empty(),
            "can_build_ships >= 1 → no shipyard emit"
        );
    }

    #[test]
    fn rule_5a_silent_when_no_core() {
        let stub = StubAdapter {
            can_build: 0.0,
            systems_with_core: 0.0,
            colony_count: 2.0,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(proposals.is_empty(), "no core → no shipyard emit");
    }

    #[test]
    fn rule_1_preempts_rule_5a_via_early_return() {
        // Both conditions hit: Rule 1 should fire and Rule 5a should
        // be silent (matching `SimpleNpcPolicy::decide`'s
        // `return commands;` after Rule 1).
        let target = Entity::from_raw_u32(42).unwrap();
        let ship = Entity::from_raw_u32(100).unwrap();
        let stub = StubAdapter {
            hostile_systems: vec![target],
            idle_combat: vec![ship],
            can_build: 0.0,
            systems_with_core: 1.0,
            colony_count: 2.0,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "build_structure"),
            "Rule 1 must early-return before Rule 5a runs",
        );
    }
}
