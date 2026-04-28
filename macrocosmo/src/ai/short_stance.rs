//! Short-layer rule logic — emits Proposals based on
//! [`super::short_adapter::ShortGameAdapter`] input. With #449 PR2d,
//! Rules 2 (survey) and 5b (slot fill) move out of `MidStanceAgent`
//! and onto per-`ShortAgent` reasoning: a `Fleet`-scope agent runs
//! Rule 2 over its own ships, a `ColonizedSystem`-scope agent runs
//! Rule 5b over its own colony.
//!
//! Stateless and engine-agnostic in spirit — `decide` is a pure
//! function of `(adapter, faction_id, now)`. Mirrors the
//! `MidStanceAgent::decide` shape so the call sites are uniform.

use macrocosmo_ai::{Command, CommandValue, Proposal};

use crate::ai::convert::{to_ai_entity, to_ai_system};
use crate::ai::schema::ids::command as cmd_ids;
use crate::ai::short_adapter::ShortGameAdapter;
use crate::ai::short_agent::ShortScope;

/// Stateless agent. State (active operations, decomposition queue)
/// lives on the `ShortAgent.state: PlanState` Component (PR2c).
pub struct ShortStanceAgent;

impl ShortStanceAgent {
    /// Run the short-layer ruleset for one `ShortAgent` on one tick.
    /// Branches on [`ShortScope`]:
    ///   - `Fleet`: Rule 2 (survey) — zips `idle_surveyors` against
    ///     `unsurveyed_targets`, one ship per system.
    ///   - `ColonizedSystem`: Rule 5b (slot fill) — picks a building
    ///     based on net production deficits.
    ///
    /// Both rules emit Proposals with the same Command shape the Mid
    /// layer used pre-PR2d, so the cutover is path-only — handler-side
    /// behavior stays identical.
    pub fn decide<A: ShortGameAdapter>(
        adapter: &A,
        faction_id: macrocosmo_ai::FactionId,
        now: i64,
    ) -> Vec<Proposal> {
        let mut proposals = Vec::new();

        match adapter.scope() {
            ShortScope::Fleet(_fleet_entity) => {
                // ----- Rule 2: Survey unsurveyed systems.
                // Mirror of the legacy Mid-side Rule 2: zip
                // `idle_surveyors × unsurveyed_targets` (one ship per
                // target, up to whichever runs out first), emit
                // `survey_system` with `target_system` + `ship_count = 1`
                // + `ship_0`. The adapter's `unsurveyed_targets` is the
                // final Rule 2 input — `npc_decision_tick` already
                // applied the Bug A dedup (`PendingAssignment` ∪
                // outbox-resident `survey_system` commands) before
                // building `pending_survey_targets`, so the agent does
                // **not** re-dedup.
                let surveyors = adapter.idle_surveyors();
                let targets = adapter.unsurveyed_targets();
                if !surveyors.is_empty() && !targets.is_empty() {
                    for (ship, &target) in surveyors.iter().zip(targets.iter()) {
                        let cmd = Command::new(cmd_ids::survey_system(), faction_id, now)
                            .with_param("target_system", CommandValue::System(to_ai_system(target)))
                            .with_param("ship_count", CommandValue::I64(1))
                            .with_param("ship_0", CommandValue::Entity(to_ai_entity(*ship)));
                        proposals.push(Proposal::at_system(cmd, to_ai_system(target)));
                    }
                }
            }
            ShortScope::ColonizedSystem(_system_entity) => {
                // ----- Rule 5b: Slot fill.
                // Mirror of the legacy Mid-side Rule 5b: gated on
                // `free_building_slots > 0.0`, then a three-branch
                // priority (`net_production_energy < 0` → power_plant,
                // else `net_production_food < 0` → farm, else mine).
                // Emission stays `Proposal::faction_wide` because the
                // build_structure handler picks which colony gets the
                // building — colony-locality lands once handler routing
                // is per-colony aware (future PR).
                if adapter.free_building_slots() > 0.0 {
                    let building_id = if adapter.net_production_energy() < 0.0 {
                        "power_plant"
                    } else if adapter.net_production_food() < 0.0 {
                        "farm"
                    } else {
                        "mine"
                    };
                    let cmd = Command::new(cmd_ids::build_structure(), faction_id, now)
                        .with_param("building_id", CommandValue::Str(building_id.into()));
                    proposals.push(Proposal::faction_wide(cmd));
                }
            }
        }

        proposals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Entity;
    use macrocosmo_ai::FactionId;

    use crate::ai::convert::from_ai_system;

    /// In-memory stub — mirrors the call sites
    /// `ShortStanceAgent::decide` needs without dragging in Bevy
    /// queries. Shape parallel to `mid_stance::tests::StubAdapter`.
    struct StubAdapter {
        empire: Entity,
        scope: ShortScope,
        idle_surveyors: Vec<Entity>,
        unsurveyed_targets: Vec<Entity>,
        free_building_slots: f64,
        net_production_energy: f64,
        net_production_food: f64,
    }

    impl StubAdapter {
        fn fleet(fleet: Entity) -> Self {
            Self {
                empire: Entity::from_raw_u32(1).unwrap(),
                scope: ShortScope::Fleet(fleet),
                idle_surveyors: vec![],
                unsurveyed_targets: vec![],
                free_building_slots: 0.0,
                net_production_energy: 0.0,
                net_production_food: 0.0,
            }
        }

        fn colonized_system(system: Entity) -> Self {
            Self {
                empire: Entity::from_raw_u32(1).unwrap(),
                scope: ShortScope::ColonizedSystem(system),
                idle_surveyors: vec![],
                unsurveyed_targets: vec![],
                free_building_slots: 0.0,
                net_production_energy: 0.0,
                net_production_food: 0.0,
            }
        }
    }

    impl ShortGameAdapter for StubAdapter {
        fn empire(&self) -> Entity {
            self.empire
        }
        fn scope(&self) -> &ShortScope {
            &self.scope
        }
        fn idle_surveyors(&self) -> &[Entity] {
            &self.idle_surveyors
        }
        fn unsurveyed_targets(&self) -> &[Entity] {
            &self.unsurveyed_targets
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

    // ---- Rule 2 (Fleet scope) ----

    #[test]
    fn fleet_scope_emits_survey_per_zip_pair() {
        let fleet = Entity::from_raw_u32(10).unwrap();
        let target_a = Entity::from_raw_u32(50).unwrap();
        let target_b = Entity::from_raw_u32(51).unwrap();
        let surveyor_a = Entity::from_raw_u32(200).unwrap();
        let surveyor_b = Entity::from_raw_u32(201).unwrap();
        let stub = StubAdapter {
            idle_surveyors: vec![surveyor_a, surveyor_b],
            unsurveyed_targets: vec![target_a, target_b],
            ..StubAdapter::fleet(fleet)
        };
        let proposals = ShortStanceAgent::decide(&stub, FactionId(7), 10);
        assert_eq!(proposals.len(), 2);
        for p in &proposals {
            assert_eq!(p.command.kind.as_str(), "survey_system");
        }
        match proposals[0].command.params.get("target_system") {
            Some(CommandValue::System(sys_ref)) => assert_eq!(from_ai_system(*sys_ref), target_a),
            _ => panic!("expected target_system"),
        }
        match proposals[0].command.params.get("ship_count") {
            Some(CommandValue::I64(n)) => assert_eq!(*n, 1),
            _ => panic!("expected ship_count=1"),
        }
        match proposals[0].command.params.get("ship_0") {
            Some(CommandValue::Entity(_)) => {}
            _ => panic!("expected ship_0"),
        }
    }

    #[test]
    fn fleet_scope_silent_without_surveyors() {
        let fleet = Entity::from_raw_u32(10).unwrap();
        let target = Entity::from_raw_u32(50).unwrap();
        let stub = StubAdapter {
            unsurveyed_targets: vec![target],
            ..StubAdapter::fleet(fleet)
        };
        let proposals = ShortStanceAgent::decide(&stub, FactionId(7), 10);
        assert!(proposals.is_empty());
    }

    #[test]
    fn fleet_scope_silent_without_targets() {
        let fleet = Entity::from_raw_u32(10).unwrap();
        let surveyor = Entity::from_raw_u32(200).unwrap();
        let stub = StubAdapter {
            idle_surveyors: vec![surveyor],
            ..StubAdapter::fleet(fleet)
        };
        let proposals = ShortStanceAgent::decide(&stub, FactionId(7), 10);
        assert!(proposals.is_empty());
    }

    // ---- Rule 5b (ColonizedSystem scope) ----

    #[test]
    fn colonized_system_picks_power_plant_when_energy_negative() {
        let sys = Entity::from_raw_u32(20).unwrap();
        let stub = StubAdapter {
            free_building_slots: 2.0,
            net_production_energy: -5.0,
            net_production_food: -3.0,
            ..StubAdapter::colonized_system(sys)
        };
        let proposals = ShortStanceAgent::decide(&stub, FactionId(7), 10);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].command.kind.as_str(), "build_structure");
        match proposals[0].command.params.get("building_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "power_plant"),
            _ => panic!("expected building_id"),
        }
    }

    #[test]
    fn colonized_system_picks_farm_when_only_food_negative() {
        let sys = Entity::from_raw_u32(20).unwrap();
        let stub = StubAdapter {
            free_building_slots: 1.0,
            net_production_energy: 5.0,
            net_production_food: -3.0,
            ..StubAdapter::colonized_system(sys)
        };
        let proposals = ShortStanceAgent::decide(&stub, FactionId(7), 10);
        assert_eq!(proposals.len(), 1);
        match proposals[0].command.params.get("building_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "farm"),
            _ => panic!("expected farm"),
        }
    }

    #[test]
    fn colonized_system_falls_back_to_mine() {
        let sys = Entity::from_raw_u32(20).unwrap();
        let stub = StubAdapter {
            free_building_slots: 1.0,
            net_production_energy: 5.0,
            net_production_food: 5.0,
            ..StubAdapter::colonized_system(sys)
        };
        let proposals = ShortStanceAgent::decide(&stub, FactionId(7), 10);
        assert_eq!(proposals.len(), 1);
        match proposals[0].command.params.get("building_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "mine"),
            _ => panic!("expected mine"),
        }
    }

    #[test]
    fn colonized_system_silent_without_free_slots() {
        let sys = Entity::from_raw_u32(20).unwrap();
        let stub = StubAdapter {
            free_building_slots: 0.0,
            net_production_energy: -5.0,
            net_production_food: -3.0,
            ..StubAdapter::colonized_system(sys)
        };
        let proposals = ShortStanceAgent::decide(&stub, FactionId(7), 10);
        assert!(proposals.is_empty());
    }
}
