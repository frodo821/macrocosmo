//! Mid-layer rule logic — emits Proposals based on game adapter
//! input and the current Stance. PR2c (#448) covered Rules 1 + 5a;
//! PR2d extends to Rules 3 (colonize), 6 (build_ship composition),
//! 7 (retreat — early-return), and 8 (fortify_system). Rules 2
//! (survey) and 5b (slot fill) are intentionally **not** ported
//! here — they live in `SimpleNpcPolicy` until the Short-per-fleet
//! migration (#449) gives them a proper home.
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
    /// every rule below fires identically across all stances today.
    /// PR3+ adds stance-dependent priority weighting / proposal
    /// filtering (#467 phase 1).
    pub fn decide<A: MidGameAdapter>(
        adapter: &A,
        stance: &Stance,
        _faction_id: &str,
        now: i64,
    ) -> Vec<Proposal> {
        // PR2c/2d: stance is intentionally unused. Silenced
        // explicitly so a future port can find this site by grep.
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
            return proposals;
        }

        // ----- Rule 2 (survey): NOT ported — stays in legacy.
        // Will be owned by the Short layer per-fleet (#449).

        // ----- Rule 3: Colonize surveyed uncolonized systems.
        // Mirrors `SimpleNpcPolicy::decide` Rule 3 exactly: zips
        // `idle_colonizers` against `colonizable_systems` (one ship
        // per target up to whichever runs out first), emits
        // `colonize_system` with `ship_count = 1` + `ship_0`. The
        // adapter's `colonizable_systems` already has the Bug B
        // filter chain applied (no hostile, own Core, no in-flight
        // outbox entry) so we don't re-filter here.
        let idle_colonizers = adapter.idle_colonizers();
        let colonizable = adapter.colonizable_systems();
        if !colonizable.is_empty() && !idle_colonizers.is_empty() {
            for (ship, &target) in idle_colonizers.iter().zip(colonizable.iter()) {
                let cmd = Command::new(cmd_ids::colonize_system(), faction_id, now)
                    .with_param("target_system", CommandValue::System(to_ai_system(target)))
                    .with_param("ship_count", CommandValue::I64(1))
                    .with_param("ship_0", CommandValue::Entity(to_ai_entity(*ship)));
                proposals.push(Proposal::at_system(cmd, to_ai_system(target)));
            }
        }

        // ----- Rule 4 (research_focus): NOT ported — stays in
        // legacy. Research is empire-wide and best handled by a
        // dedicated Mid track once we have one.

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

        // ----- Rule 5b (slot fill): NOT ported — stays in legacy.
        // The "build power_plant / farm / mine based on net
        // production" policy is Short-per-colony scope, not Mid.

        // ----- Rule 6: Fleet composition gap → build_ship.
        // Mirrors `SimpleNpcPolicy::decide` Rule 6 exactly. Gated on
        // `can_build_ships >= 1.0`; then the same three-branch
        // priority order (survey → colony → combat<3) emits at most
        // one `build_ship` proposal per tick.
        if can_build >= 1.0 {
            let comp = adapter.fleet_composition();

            if comp.survey_count == 0 && adapter.has_unsurveyed_targets() {
                let cmd = Command::new(cmd_ids::build_ship(), faction_id, now)
                    .with_param("design_id", CommandValue::Str("explorer_mk1".into()));
                proposals.push(Proposal::faction_wide(cmd));
            } else if comp.colony_count == 0 && adapter.has_colonizable_targets() {
                let cmd = Command::new(cmd_ids::build_ship(), faction_id, now)
                    .with_param("design_id", CommandValue::Str("colony_ship_mk1".into()));
                proposals.push(Proposal::faction_wide(cmd));
            } else if comp.combat_count < 3 {
                let cmd = Command::new(cmd_ids::build_ship(), faction_id, now)
                    .with_param("design_id", CommandValue::Str("patrol_corvette".into()));
                proposals.push(Proposal::faction_wide(cmd));
            }
        }

        // ----- Rule 7: Retreat when fleet is weak.
        // Mirrors `SimpleNpcPolicy::decide` Rule 7 exactly — the
        // strict `> 0.0` lower bound means an unset / never-emitted
        // metric (default 0.0) keeps the policy silent. Early-return
        // skips Rule 8.
        let fleet_ready = adapter.fleet_ready_ratio();
        if fleet_ready > 0.0 && fleet_ready < 0.3 {
            let cmd = Command::new(cmd_ids::retreat(), faction_id, now);
            proposals.push(Proposal::faction_wide(cmd));
            return proposals;
        }

        // ----- Rule 8: Fortify when shipyard exists but few ships.
        // Mirrors `SimpleNpcPolicy::decide` Rule 8 exactly:
        // `can_build_ships >= 1.0 && total_ships < colony_count * 2`.
        let total_ships = adapter.total_ships();
        if can_build >= 1.0 && total_ships < colony_count * 2.0 {
            let cmd = Command::new(cmd_ids::fortify_system(), faction_id, now);
            proposals.push(Proposal::faction_wide(cmd));
        }

        proposals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::convert::from_ai_system;
    use crate::ai::mid_adapter::FleetComposition;

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
        colonizable_systems: Vec<Entity>,
        idle_colonizers: Vec<Entity>,
        fleet_ready: f64,
        total_ships: f64,
        fleet_composition: FleetComposition,
        has_unsurveyed_targets: bool,
        has_colonizable_targets: bool,
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
                colonizable_systems: vec![],
                idle_colonizers: vec![],
                fleet_ready: 0.0,
                total_ships: 0.0,
                fleet_composition: FleetComposition::default(),
                has_unsurveyed_targets: false,
                has_colonizable_targets: false,
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
        fn colonizable_systems(&self) -> &[Entity] {
            &self.colonizable_systems
        }
        fn idle_colonizers(&self) -> Vec<Entity> {
            self.idle_colonizers.clone()
        }
        fn fleet_ready_ratio(&self) -> f64 {
            self.fleet_ready
        }
        fn total_ships(&self) -> f64 {
            self.total_ships
        }
        fn fleet_composition(&self) -> FleetComposition {
            self.fleet_composition
        }
        fn has_unsurveyed_targets(&self) -> bool {
            self.has_unsurveyed_targets
        }
        fn has_colonizable_targets(&self) -> bool {
            self.has_colonizable_targets
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
        // can_build >= 1.0 → Rule 5a silent. Rule 6 is gated on
        // `can_build >= 1.0` AND survey/colony/combat conditions —
        // none hold here (no targets, no ships) so it's also silent.
        // Rule 8 is gated on `total_ships < colony_count * 2`; with
        // total=0 and colony=2, 0 < 4 → Rule 8 fires.
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "build_structure"),
            "can_build_ships >= 1 → no shipyard emit",
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
        // Rule 5a gated; Rule 6 needs can_build >= 1.0 (false); Rule 8
        // also needs can_build >= 1.0 (false). Nothing fires.
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

    // ---- Rule 3 (colonize_system) ----

    #[test]
    fn rule_3_fires_when_colonizable_and_idle_colonizer_present() {
        let target = Entity::from_raw_u32(50).unwrap();
        let colonizer = Entity::from_raw_u32(200).unwrap();
        let stub = StubAdapter {
            colonizable_systems: vec![target],
            idle_colonizers: vec![colonizer],
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        let colonize = proposals
            .iter()
            .find(|p| p.command.kind.as_str() == "colonize_system")
            .expect("Rule 3 must emit colonize_system");
        match colonize.command.params.get("target_system") {
            Some(CommandValue::System(sys_ref)) => assert_eq!(from_ai_system(*sys_ref), target),
            _ => panic!("expected target_system param"),
        }
        match colonize.command.params.get("ship_count") {
            Some(CommandValue::I64(n)) => assert_eq!(*n, 1),
            _ => panic!("expected ship_count=1"),
        }
        match colonize.command.params.get("ship_0") {
            Some(CommandValue::Entity(_)) => {}
            _ => panic!("expected ship_0 entity"),
        }
    }

    #[test]
    fn rule_3_silent_when_no_colonizer() {
        let target = Entity::from_raw_u32(50).unwrap();
        let stub = StubAdapter {
            colonizable_systems: vec![target],
            idle_colonizers: vec![],
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "colonize_system"),
            "no idle colonizer → no colonize emit",
        );
    }

    // ---- Rule 6 (build_ship composition) ----

    #[test]
    fn rule_6_builds_explorer_when_no_survey_and_unsurveyed_present() {
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 1.0,
            // Total ships > colony*2 so Rule 8 stays silent.
            total_ships: 10.0,
            fleet_composition: FleetComposition {
                survey_count: 0,
                colony_count: 0,
                combat_count: 5,
            },
            has_unsurveyed_targets: true,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        let build = proposals
            .iter()
            .find(|p| p.command.kind.as_str() == "build_ship")
            .expect("Rule 6 must emit build_ship");
        match build.command.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "explorer_mk1"),
            _ => panic!("expected design_id"),
        }
    }

    #[test]
    fn rule_6_builds_colony_ship_when_no_colony_and_colonizable_present() {
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 1.0,
            total_ships: 10.0,
            fleet_composition: FleetComposition {
                survey_count: 1,
                colony_count: 0,
                combat_count: 5,
            },
            has_unsurveyed_targets: false,
            has_colonizable_targets: true,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        let build = proposals
            .iter()
            .find(|p| p.command.kind.as_str() == "build_ship")
            .expect("Rule 6 must emit build_ship");
        match build.command.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "colony_ship_mk1"),
            _ => panic!("expected design_id"),
        }
    }

    #[test]
    fn rule_6_builds_corvette_when_combat_below_threshold() {
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 1.0,
            total_ships: 10.0,
            fleet_composition: FleetComposition {
                survey_count: 1,
                colony_count: 1,
                combat_count: 1, // < 3
            },
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        let build = proposals
            .iter()
            .find(|p| p.command.kind.as_str() == "build_ship")
            .expect("Rule 6 must emit build_ship");
        match build.command.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "patrol_corvette"),
            _ => panic!("expected design_id"),
        }
    }

    #[test]
    fn rule_6_silent_when_can_build_below_one() {
        let stub = StubAdapter {
            can_build: 0.0,
            fleet_composition: FleetComposition {
                survey_count: 0,
                colony_count: 0,
                combat_count: 0,
            },
            has_unsurveyed_targets: true,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "build_ship"),
            "can_build < 1 → no build_ship",
        );
    }

    // ---- Rule 7 (retreat — early-return) ----

    #[test]
    fn rule_7_fires_when_fleet_ready_below_threshold() {
        let stub = StubAdapter {
            fleet_ready: 0.2,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].command.kind.as_str(), "retreat");
    }

    #[test]
    fn rule_7_silent_when_fleet_ready_zero() {
        // Strict `> 0.0` lower bound: a value of exactly 0.0 means
        // "no fleet metric ever emitted" and must NOT trigger
        // retreat, matching `SimpleNpcPolicy`.
        let stub = StubAdapter {
            fleet_ready: 0.0,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "retreat"),
            "fleet_ready == 0 → no retreat",
        );
    }

    #[test]
    fn rule_7_silent_when_fleet_ready_at_or_above_threshold() {
        let stub = StubAdapter {
            fleet_ready: 0.3,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "retreat"),
            "fleet_ready >= 0.3 → no retreat",
        );
    }

    #[test]
    fn rule_7_preempts_rule_8_via_early_return() {
        // Conditions for both fire: weak fleet + can_build >= 1 +
        // total_ships < colony_count * 2. SimpleNpcPolicy returns
        // immediately after retreat — Rule 8 must not emit.
        let stub = StubAdapter {
            fleet_ready: 0.2,
            can_build: 1.0,
            total_ships: 1.0,
            colony_count: 3.0,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "fortify_system"),
            "Rule 7 must early-return before Rule 8 runs",
        );
        assert!(
            proposals
                .iter()
                .any(|p| p.command.kind.as_str() == "retreat"),
            "Rule 7 still fires",
        );
    }

    #[test]
    fn rule_7_does_not_preempt_earlier_rules_5a_or_6() {
        // Rule 7 sits *after* Rules 5a / 6 in the legacy ordering,
        // so a weak-fleet empire with shipyard-needs and a build
        // target should still emit those before retreating.
        let stub = StubAdapter {
            fleet_ready: 0.2,
            can_build: 0.0,
            systems_with_core: 1.0,
            colony_count: 1.0,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .any(|p| p.command.kind.as_str() == "build_structure"),
            "Rule 5a fires before Rule 7's early-return",
        );
        assert!(
            proposals
                .iter()
                .any(|p| p.command.kind.as_str() == "retreat"),
            "Rule 7 still fires after Rule 5a",
        );
    }

    // ---- Rule 8 (fortify_system) ----

    #[test]
    fn rule_8_fires_when_shipyard_present_and_few_ships() {
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 3.0,
            total_ships: 1.0,
            // Empty composition + no targets → Rule 6 silent
            // (combat_count < 3 would otherwise fire). Force
            // combat_count >= 3 to avoid that path.
            fleet_composition: FleetComposition {
                survey_count: 1,
                colony_count: 1,
                combat_count: 3,
            },
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .any(|p| p.command.kind.as_str() == "fortify_system"),
            "Rule 8 must emit fortify_system",
        );
    }

    #[test]
    fn rule_8_silent_when_total_ships_at_or_above_threshold() {
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 2.0,
            total_ships: 4.0, // 4 >= 2*2
            fleet_composition: FleetComposition {
                survey_count: 1,
                colony_count: 1,
                combat_count: 3,
            },
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "fortify_system"),
            "total_ships >= colony_count*2 → no fortify",
        );
    }

    #[test]
    fn rule_8_silent_when_can_build_below_one() {
        let stub = StubAdapter {
            can_build: 0.0,
            colony_count: 3.0,
            total_ships: 1.0,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "fortify_system"),
            "can_build < 1 → no fortify",
        );
    }
}
