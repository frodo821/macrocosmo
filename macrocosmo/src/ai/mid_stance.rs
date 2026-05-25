//! Mid-layer rule logic — emits Proposals based on game adapter
//! input and the current Stance. Implements Rules 1 (attack), 3
//! (colonize), 5a (shipyard), 6 (build_ship composition), 7 (retreat
//! — early-return), and 8 (fortify_system). Rule 4 (research_focus)
//! is intentionally not handled here — research is empire-wide and
//! best handled by a dedicated Mid track once we have one.
//!
//! Rules 2 (survey) and 5b (slot fill) moved to
//! [`super::short_stance::ShortStanceAgent`] in #449 PR2d (cutover):
//! a `Fleet`-scope ShortAgent runs Rule 2 over its own ships, a
//! `ColonizedSystem`-scope ShortAgent runs Rule 5b over its own
//! colony. Mid no longer emits these.
//!
//! `MidStanceAgent` is parallel to `macrocosmo_ai::IntentDrivenMidTerm`
//! by design (Plan agent micro-decision 3): different responsibility
//! (raw game state vs. parsed Intents). PR4 of #448 unifies them
//! under a single Agent trait.

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
        // Requires both a known hostile system and at least one idle
        // combat ship; targets `hostile_systems[0]`; param shape is
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

            // Combat takes priority over every later rule — early
            // return so Rules 2-8 stay silent this tick.
            return proposals;
        }

        // ----- Rule 2 (survey): MOVED to ShortStanceAgent (Fleet
        // scope) in #449 PR2d. Per-fleet `ShortAgent` zips its own
        // idle surveyors against the region's unsurveyed targets and
        // emits the same `survey_system` Command shape. Bug A dedup
        // (`pending_survey_targets` = `PendingAssignment` ∪
        // outbox-resident commands) is still applied in
        // `npc_decision_tick` upstream, so the per-fleet Short sees
        // a pre-filtered target list.

        // ----- Rule 3: Colonize surveyed uncolonized systems.
        // Zips `idle_colonizers` against `colonizable_systems` (one
        // ship per target up to whichever runs out first), emits
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

        // ----- Rule 3.5: Expansion frontier — deploy a Core to
        // surveyed-but-not-owned systems.
        //
        // #444 hotfix. Without this rule a starter empire whose
        // `Region.member_systems` is `{capital}` (the only colonised,
        // already-cored system) has every later rule starve:
        //   * Rule 3's `colonizable_systems` is empty because the
        //     region's only system is already colonised.
        //   * The Mid emits nothing, the Short layer has no campaigns,
        //     so `deploy_deliverable` is never produced and the empire
        //     never plants a Core anywhere new.
        //
        // Rule 3.5 closes the loop: for each surveyed-but-not-owned
        // frontier system the adapter exposes (pre-filtered by
        // `npc_decision_tick` against own-Core / pending-deploy /
        // hostile / current region membership), pair it with an idle
        // courier and emit `deploy_deliverable(infrastructure_core)`.
        // The dispatcher's eager macro expansion (#444 fold-in)
        // turns this into the 4-step build/load/reposition/unload
        // chain so the ship actually moves.
        //
        // Courier double-use is prevented upstream:
        // `npc_decision_tick` partitions `idle_colonizers` and
        // `idle_couriers` so Rule 3 and Rule 3.5 never see the same
        // ship within one tick.
        let idle_couriers = adapter.idle_couriers();
        let frontier = adapter.expansion_frontier_systems();
        // Hotfix-3 resource gate: `deploy_deliverable` decomposes
        // into `build_deliverable(infrastructure_core)` at the
        // colony, so the same stockpile check applies. Pre-hotfix a
        // starving empire flooded Cores onto the frontier list.
        if !frontier.is_empty()
            && !idle_couriers.is_empty()
            && adapter.can_afford_design("infrastructure_core")
        {
            for (ship, &target) in idle_couriers.iter().zip(frontier.iter()) {
                let cmd = Command::new(cmd_ids::deploy_deliverable(), faction_id, now)
                    .with_param(
                        "definition_id",
                        CommandValue::Str("infrastructure_core".into()),
                    )
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
        // Gated on `can_build_ships < 1.0 && systems_with_core > 0
        // && colony_count > 0`. The handler-side dedup absorbs
        // per-tick re-emission while the queue drains.
        let can_build = adapter.can_build_ships();
        let systems_with_core = adapter.systems_with_core();
        let colony_count = adapter.colony_count();
        if can_build < 1.0
            && systems_with_core > 0.0
            && colony_count > 0.0
            // Hotfix-3 resource gate: don't emit shipyard build
            // proposals an empty stockpile cannot pay for. Pre-hotfix
            // the system-building dedup absorbed the per-tick
            // re-emission, but only after the slot was already
            // booked — a minerals-starved empire still left the
            // shipyard order frozen in the queue forever, starving
            // every later rule that needs the shipyard.
            && adapter.can_afford_building("shipyard")
        {
            let cmd = Command::new(cmd_ids::build_structure(), faction_id, now)
                .with_param("building_id", CommandValue::Str("shipyard".into()));
            // `build_structure` has no `target_system` param — the
            // handler picks an owned system. Faction-wide locality
            // until #449 introduces region routing.
            proposals.push(Proposal::faction_wide(cmd));
        }

        // ----- Rule 5b (slot fill): MOVED to ShortStanceAgent
        // (ColonizedSystem scope) in #449 PR2d. Per-colonized-system
        // `ShortAgent` runs the same three-branch priority
        // (power_plant / farm / mine) and emits the same
        // `Proposal::faction_wide(build_structure)` shape — handler
        // routing is unchanged.

        // ----- Rule 6: Fleet composition gap → build_ship.
        // Gated on `can_build_ships >= 1.0`; then a three-branch
        // priority order (survey → colony → combat<3) emits at most
        // one `build_ship` proposal per tick.
        if can_build >= 1.0 {
            let comp = adapter.fleet_composition();

            // Hotfix-3: priority order is `survey → colony → combat<3`.
            // The branch picks the highest-priority *unmet need*, then
            // applies the resource gate. Failing the gate means
            // "wait, don't spend" — we do NOT fall through to a
            // cheaper branch because the cheaper need wasn't the one
            // we identified. Mirrors how a starving human player
            // would defer the build rather than switch designs.
            //
            // Pre-hotfix the gate condition didn't exist, so this
            // structural shape (separate need-detection then gate)
            // is new. The legacy fall-through behaviour
            // (survey_count != 0 → check colony → check combat) is
            // preserved for the need-detection step itself.
            let pick = if comp.survey_count == 0 && adapter.has_unsurveyed_targets() {
                Some("explorer_mk1")
            } else if comp.colony_count == 0 && adapter.has_colonizable_targets() {
                Some("colony_ship_mk1")
            } else if comp.combat_count < 3 {
                Some("patrol_corvette")
            } else {
                None
            };
            if let Some(design_id) = pick
                && adapter.can_afford_design(design_id)
            {
                let cmd = Command::new(cmd_ids::build_ship(), faction_id, now)
                    .with_param("design_id", CommandValue::Str(design_id.into()));
                proposals.push(Proposal::faction_wide(cmd));
            }
        }

        // ----- Rule 7: Retreat when fleet is weak.
        // The strict `> 0.0` lower bound means an unset /
        // never-emitted metric (default 0.0) keeps the policy
        // silent. Early-return skips Rule 8.
        let fleet_ready = adapter.fleet_ready_ratio();
        if fleet_ready > 0.0 && fleet_ready < 0.3 {
            let cmd = Command::new(cmd_ids::retreat(), faction_id, now);
            proposals.push(Proposal::faction_wide(cmd));
            return proposals;
        }

        // ----- Rule 8: Fortify when shipyard exists but few ships.
        // Gated on `can_build_ships >= 1.0 && total_ships <
        // colony_count * 2`.
        //
        // #532 F2 resource gate: PR #531 added affordability gates to
        // Rules 3.5 / 5a / 5b / 6 but **missed Rule 8**. Pre-fix this
        // emitted `fortify_system` (no `design_id`), and the handler's
        // auto-pick had no affordability check — a bankrupt empire
        // with a shipyard and `total_ships < colony_count * 2` would
        // still queue one unaffordable combat ship per Reason tick.
        //
        // Fix: the adapter picks the cheapest affordable
        // direct-buildable combat design (`affordable_fortify_design`,
        // pending-aware via `can_afford_design`), then we emit
        // `build_ship{design_id}` directly. When no affordable combat
        // design exists the rule is silent — a bankrupt empire
        // shouldn't queue ships at all. Mirrors Rule 5a / Rule 6's
        // "adapter-decides-then-Mid-emits" pattern.
        //
        // The `fortify_system` command kind + handler remain reachable
        // for non-Rule-8 callers (e.g. future Lua policy scripts);
        // only Rule 8's choice of command kind changes.
        let total_ships = adapter.total_ships();
        if can_build >= 1.0
            && total_ships < colony_count * 2.0
            && let Some(design_id) = adapter.affordable_fortify_design()
        {
            let cmd = Command::new(cmd_ids::build_ship(), faction_id, now)
                .with_param("design_id", CommandValue::Str(design_id.into()));
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
        /// Hotfix-3 resource gate. `None` = trait default (always
        /// affordable). `Some(set)` = explicit allow-list of
        /// `design_id` strings the stub considers fundable.
        affordable_designs: Option<std::collections::HashSet<String>>,
        affordable_buildings: Option<std::collections::HashSet<String>>,
        /// #532 F2: Rule 8's adapter-side pick. `Some(id)` = the stub
        /// surfaces a concrete combat design id for the rule to emit;
        /// `None` = rule must stay silent (no affordable combat design).
        /// Defaults to `Some("patrol_corvette")` so legacy Rule 8 tests
        /// that don't care about F2 semantics keep firing.
        fortify_design: Option<String>,
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
                affordable_designs: None,
                affordable_buildings: None,
                fortify_design: Some("patrol_corvette".into()),
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
        fn can_afford_design(&self, design_id: &str) -> bool {
            match &self.affordable_designs {
                Some(set) => set.contains(design_id),
                None => true,
            }
        }
        fn can_afford_building(&self, building_id: &str) -> bool {
            match &self.affordable_buildings {
                Some(set) => set.contains(building_id),
                None => true,
            }
        }
        fn affordable_fortify_design(&self) -> Option<String> {
            self.fortify_design.clone()
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
        // be silent (Rule 1's early-return skips later rules).
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

    // ---- Rule 2 (survey_system) ----
    //
    // Removed in #449 PR2d (cutover). Per-fleet `ShortStanceAgent`
    // now owns survey emission; see `ai::short_stance::tests` and
    // `tests/short_rules_cutover_sentinel.rs` for coverage.

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

    // ---- Rule 5b (slot fill / building) ----
    //
    // Removed in #449 PR2d (cutover). Per-colony `ShortStanceAgent`
    // (`ColonizedSystem` scope) now owns slot-fill emission; see
    // `ai::short_stance::tests` and
    // `tests/short_rules_cutover_sentinel.rs` for coverage.

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
        // retreat.
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
        // total_ships < colony_count * 2. Rule 7 returns immediately
        // after retreat — Rule 8 must not emit.
        //
        // Force combat_count >= 3 so Rule 6's third branch (build a
        // corvette when combat < 3) is silent and the only path to a
        // `build_ship` proposal would be Rule 8. Then assert no
        // build_ship + retreat present.
        let stub = StubAdapter {
            fleet_ready: 0.2,
            can_build: 1.0,
            total_ships: 1.0,
            colony_count: 3.0,
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
                .all(|p| p.command.kind.as_str() != "build_ship"),
            "Rule 7 must early-return before Rule 8 runs (no build_ship)",
        );
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "fortify_system"),
            "#532 F2: Rule 8 no longer emits fortify_system either",
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
        // Rule 7 sits *after* Rules 5a / 6, so a weak-fleet empire
        // with shipyard-needs and a build target should still emit
        // those before retreating.
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

    // ---- Rule 8 (fortify — #532 F2: now emits build_ship) ----

    #[test]
    fn rule_8_fires_when_shipyard_present_and_few_ships() {
        // #532 F2: Rule 8 now emits `build_ship{design_id}` directly,
        // sourced from `adapter.affordable_fortify_design()`. The stub
        // defaults `fortify_design` to `Some("patrol_corvette")` so
        // pre-F2 tests stay green.
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
        let build = proposals
            .iter()
            .find(|p| p.command.kind.as_str() == "build_ship")
            .expect("Rule 8 must emit build_ship (post-F2)");
        match build.command.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(
                s.as_ref(),
                "patrol_corvette",
                "Rule 8 must emit the adapter-picked design id",
            ),
            _ => panic!("Rule 8 build_ship missing design_id param"),
        }
        // Sanity: #532 F2 — fortify_system is no longer emitted by
        // Rule 8 itself (the command kind / handler remain for other
        // call paths).
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "fortify_system"),
            "#532 F2: Rule 8 no longer emits fortify_system",
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
                .all(|p| p.command.kind.as_str() != "build_ship"),
            "total_ships >= colony_count*2 → no Rule 8 build_ship",
        );
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "fortify_system"),
            "#532 F2: also no legacy fortify_system",
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
                .all(|p| p.command.kind.as_str() != "build_ship"),
            "can_build < 1 → no Rule 8 build_ship",
        );
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "fortify_system"),
            "#532 F2: also no legacy fortify_system",
        );
    }

    /// #532 F2: Rule 8 must stay silent when no affordable combat
    /// design exists (`adapter.affordable_fortify_design()` → `None`).
    /// Pre-F2 this rule emitted `fortify_system` with no
    /// affordability check, and the handler's auto-pick queued an
    /// unaffordable combat ship anyway. Post-fix the rule defers
    /// rather than booking an order the stockpile can't pay for.
    #[test]
    fn rule_8_silent_when_no_affordable_combat_design() {
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 3.0,
            total_ships: 1.0,
            fleet_composition: FleetComposition {
                survey_count: 1,
                colony_count: 1,
                combat_count: 3,
            },
            // Adapter says "no affordable combat design" — bankrupt
            // empire case.
            fortify_design: None,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals.iter().all(|p| {
                let k = p.command.kind.as_str();
                k != "build_ship" && k != "fortify_system"
            }),
            "Rule 8 must stay silent when affordable_fortify_design returns None",
        );
    }

    // ---- Hotfix-3: resource gate + Rule 6 fleet_composition semantics ----
    //
    // Pre-hotfix Rule 6 saw `survey_count == 0` for an empire whose
    // only explorer was currently surveying (because the surveying
    // ship's `ShipState::Surveying` left `info.system == None` and
    // `NpcContext.ships` filtered those out). The infinite-build loop
    // that followed (`build_ship explorer_mk1` every Reason tick)
    // bankrupted starter empires by stacking maintenance against a
    // depleted stockpile.

    /// Pin: Rule 6's explorer branch stays silent when the empire's
    /// fleet census already contains a surveyor — alive OR currently
    /// `Surveying` OR `InFTL` OR `SubLight`. The hotfix routes the
    /// census via `BevyMidGameAdapter.fleet_composition` (pre-computed
    /// in `npc_decision_tick` over the unfiltered `all_ships` query +
    /// build queue), so even a ship in `ShipState::Surveying` is
    /// counted.
    #[test]
    fn rule_6_explorer_not_re_built_while_alive_survey_ship_exists() {
        let mut affordable = std::collections::HashSet::new();
        affordable.insert("explorer_mk1".into());
        affordable.insert("colony_ship_mk1".into());
        affordable.insert("patrol_corvette".into());
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 1.0,
            total_ships: 10.0,
            // survey_count == 1 → explorer branch must be silent
            // even though `has_unsurveyed_targets` is true and the
            // resource gate permits the build. This is the
            // surveying-explorer case: the ship is alive but in
            // `ShipState::Surveying`, and pre-hotfix the
            // `info.system.is_some()` filter on `NpcContext.ships`
            // dropped it, collapsing the count to 0.
            fleet_composition: FleetComposition {
                survey_count: 1,
                colony_count: 1,
                combat_count: 3,
            },
            has_unsurveyed_targets: true,
            has_colonizable_targets: false,
            affordable_designs: Some(affordable),
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        for p in &proposals {
            if p.command.kind.as_str() == "build_ship" {
                match p.command.params.get("design_id") {
                    Some(CommandValue::Str(s)) => assert_ne!(
                        s.as_ref(),
                        "explorer_mk1",
                        "Rule 6 must not re-emit explorer while one is alive",
                    ),
                    _ => panic!("build_ship missing design_id"),
                }
            }
        }
    }

    /// Pin: when every surveyor has been destroyed (census shows
    /// `survey_count == 0`), Rule 6 still fires — confirms the
    /// hotfix did not over-correct into "never re-build".
    #[test]
    fn rule_6_explorer_re_built_when_all_destroyed() {
        let mut affordable = std::collections::HashSet::new();
        affordable.insert("explorer_mk1".into());
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 1.0,
            total_ships: 10.0,
            fleet_composition: FleetComposition {
                survey_count: 0,
                colony_count: 0,
                combat_count: 5,
            },
            has_unsurveyed_targets: true,
            affordable_designs: Some(affordable),
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        let build = proposals
            .iter()
            .find(|p| p.command.kind.as_str() == "build_ship")
            .expect("Rule 6 must rebuild explorer after every surveyor destroyed");
        match build.command.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "explorer_mk1"),
            _ => panic!("expected design_id"),
        }
    }

    /// Pin: Rule 6's explorer branch stays silent when the empire's
    /// stockpile cannot pay for the build, even if every other
    /// condition (no survey ship, has unsurveyed targets) matches.
    /// This stops the runaway "build, fail, retry" loop the brp QA
    /// report observed.
    #[test]
    fn rule_6_silent_when_stockpile_cannot_afford_explorer() {
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 1.0,
            total_ships: 10.0,
            fleet_composition: FleetComposition {
                survey_count: 0,
                colony_count: 0,
                combat_count: 5,
            },
            has_unsurveyed_targets: true,
            // affordable_designs = Some(empty) → can_afford_design
            // returns false for every id, including explorer_mk1.
            affordable_designs: Some(std::collections::HashSet::new()),
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "build_ship"),
            "stockpile zero → Rule 6 must not emit any build_ship",
        );
    }

    /// Pin: Rule 5a's shipyard branch honours the building-cost
    /// resource gate. Pre-hotfix the dedup absorbed re-emissions but
    /// the order was still booked the moment a slot opened — even if
    /// the minerals weren't there to invest.
    #[test]
    fn rule_5a_silent_when_stockpile_cannot_afford_shipyard() {
        let stub = StubAdapter {
            can_build: 0.0,
            systems_with_core: 1.0,
            colony_count: 2.0,
            affordable_buildings: Some(std::collections::HashSet::new()),
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "build_structure"),
            "stockpile zero → Rule 5a must not emit shipyard",
        );
    }

    /// Pin: priority semantics — Rule 6 picks the
    /// highest-priority unmet need, then applies the gate. A
    /// failing gate on the picked design must NOT fall through to
    /// the next branch. Pre-fold-in the gate was inlined per-branch
    /// with `else if`, which would have let an explorer-needing
    /// empire substitute a cheaper colony / corvette purchase the
    /// player did not authorise.
    #[test]
    fn rule_6_does_not_fall_through_to_cheaper_branch_when_top_priority_gate_fails() {
        // Survey is the top priority (survey_count == 0 &&
        // has_unsurveyed_targets), but the empire can only afford
        // colony_ship_mk1, not explorer_mk1.
        let mut affordable = std::collections::HashSet::new();
        affordable.insert("colony_ship_mk1".into());
        affordable.insert("patrol_corvette".into());
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 1.0,
            total_ships: 10.0,
            fleet_composition: FleetComposition {
                survey_count: 0,
                colony_count: 0,
                combat_count: 1,
            },
            has_unsurveyed_targets: true,
            has_colonizable_targets: true,
            affordable_designs: Some(affordable),
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        assert!(
            proposals
                .iter()
                .all(|p| p.command.kind.as_str() != "build_ship"),
            "Rule 6 must NOT substitute a cheaper design when its top-priority pick fails the gate",
        );
    }

    /// Soft gate sanity: deficit spending (revenue < expense, but
    /// stockpile non-zero) is PERMITTED. The hotfix is a stockpile
    /// gate, not a black-budget gate.
    #[test]
    fn rule_6_permits_deficit_spending_when_stockpile_positive() {
        // The stub's `affordable_designs = None` is the trait
        // default (always affordable), modelling a real adapter
        // whose stockpile passes the soft check even though the
        // empire's metrics would predict a future deficit. With
        // every other Rule 6 gate met, the build must emit.
        let stub = StubAdapter {
            can_build: 1.0,
            colony_count: 1.0,
            total_ships: 10.0,
            fleet_composition: FleetComposition {
                survey_count: 0,
                colony_count: 0,
                combat_count: 5,
            },
            has_unsurveyed_targets: true,
            affordable_designs: None,
            ..StubAdapter::empty()
        };
        let proposals = MidStanceAgent::decide(&stub, &Stance::default(), "vesk", 10);
        let build = proposals
            .iter()
            .find(|p| p.command.kind.as_str() == "build_ship")
            .expect("deficit-but-non-zero stockpile must still permit Rule 6");
        match build.command.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "explorer_mk1"),
            _ => panic!("expected design_id"),
        }
    }
}
