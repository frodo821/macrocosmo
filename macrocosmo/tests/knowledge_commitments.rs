//! Slice 2 of the knowledge redesign
//! (`docs/knowledge-redesign-implementation-plan.md`).
//!
//! Coverage:
//!
//! 1. `ai_survey_dispatch_records_commitment` — AI dispatch of a
//!    `survey_system` command records an active commitment in the
//!    issuer empire's [`CommitmentLedger`] AND the legacy
//!    `PendingAssignment` / `ShipProjection` markers continue to be
//!    written. Dual-write is the entire point of Slice 2.
//! 2. `commitment_dual_write_does_not_double_issue` — calling the
//!    dispatch a second time for the same triple is idempotent at the
//!    `(subject, kind, target)` level.

mod common;

use bevy::prelude::*;
use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::assignments::PendingAssignment;
use macrocosmo::knowledge::{
    CommitmentKind, CommitmentStatus, CommitmentTarget, KnowledgeStore, KnowledgeSubject,
    ObservationSource, ShipSnapshot, ShipSnapshotState, SystemVisibilityMap, SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

fn setup_scenario(app: &mut App) -> (Entity, Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "commitment_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [5.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Catalogued);
    }

    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    spawn_test_ruler(app.world_mut(), empire, home);

    (empire, home, frontier, ship)
}

fn dispatch_survey(app: &mut App, empire: Entity, ship: Entity, target: Entity) {
    use macrocosmo::ai::convert::{to_ai_entity, to_ai_faction, to_ai_system};
    use macrocosmo_ai::{Command, CommandKindId, CommandValue};
    let mut c = Command::new(
        CommandKindId::from("survey_system"),
        to_ai_faction(empire),
        0,
    );
    c.params.insert(
        "target_system".into(),
        CommandValue::System(to_ai_system(target)),
    );
    c.params.insert("ship_count".into(), CommandValue::I64(1));
    c.params
        .insert("ship_0".into(), CommandValue::Entity(to_ai_entity(ship)));
    {
        let mut bus = app
            .world_mut()
            .resource_mut::<macrocosmo::ai::plugin::AiBusResource>();
        bus.emit_command(c);
    }
}

#[test]
fn ai_survey_dispatch_records_commitment() {
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app);

    // Warmup tick: `AiPlugin`'s `Startup` schedule must declare command
    // kinds before `bus.emit_command` will route survey_system.
    app.update();

    // Seed a snapshot so the dispatcher's belief is well-formed (this
    // also exercises the projection write path, which Slice 2 keeps
    // alongside the commitment write).
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship,
            name: "Scout-1".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InSystem,
            last_known_system: Some(home),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    dispatch_survey(&mut app, empire, ship, frontier);
    app.world_mut().resource_mut::<GameClock>().elapsed = 100;
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();

    // Slice 2 contract 1: the ledger records a fresh active commitment.
    let subject = KnowledgeSubject::Empire(empire);
    assert!(
        store.has_active_commitment(
            subject,
            CommitmentKind::Survey,
            CommitmentTarget::System(frontier),
        ),
        "AI dispatch must record an active Survey commitment for the target system",
    );
    let entries: Vec<_> = store.commitments().iter().collect();
    assert_eq!(
        entries.len(),
        1,
        "exactly one commitment should have been recorded"
    );
    let c = entries[0];
    assert_eq!(c.subject, subject);
    assert_eq!(c.actor, Some(ship));
    assert_eq!(c.kind, CommitmentKind::Survey);
    assert_eq!(c.target, CommitmentTarget::System(frontier));
    assert_eq!(c.status, CommitmentStatus::Active);
    // Slice 1.5 review F3: basis comes from the snapshot the
    // dispatcher had at dispatch time (`propagate_knowledge` may have
    // refreshed it after our seed, so just check that *some* basis
    // tick was captured, not the exact value).
    assert!(
        c.basis_observed_at.is_some(),
        "ledger should record some basis_observed_at from the snapshot"
    );
    assert!(
        c.expected_effect_at.is_some(),
        "spatial command should carry intended_takes_effect_at as expected_effect_at"
    );
    assert!(
        c.expected_resolution_at.is_some(),
        "survey has a return leg → expected_resolution_at must be Some"
    );

    // Slice 2 contract 2: existing markers still written.
    assert!(
        app.world()
            .entity(ship)
            .get::<PendingAssignment>()
            .is_some(),
        "PendingAssignment marker must still be stamped during Slice 2 dual-write",
    );
    assert!(
        store.get_projection(ship).is_some(),
        "ShipProjection must still be written during Slice 2 dual-write",
    );
}

/// Slice 3: `SurveyComplete` arrival resolves a matching active
/// Survey commitment. Directly seeds the queue + ledger so the test
/// is independent of the dispatch path (already covered by
/// `ai_survey_dispatch_records_commitment`).
#[test]
fn survey_complete_resolves_commitment() {
    use macrocosmo::knowledge::{
        KnowledgeFact, PendingFactQueue, PerceivedFact, resolve_commitments_from_observations,
    };
    use macrocosmo::time_system::GameClock;

    let mut app = test_app();

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "resolve_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let target = spawn_test_system(
        app.world_mut(),
        "Target",
        [0.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    let ship = spawn_test_ship(app.world_mut(), "S", "explorer_mk1", home, [0.0, 0.0, 0.0]);
    spawn_test_ruler(app.world_mut(), empire, home);

    // Seed an active Survey commitment.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.commitments_mut().record_issued(
            KnowledgeSubject::Empire(empire),
            Some(ship),
            CommitmentKind::Survey,
            CommitmentTarget::System(target),
            10,
        );
    }

    // Push a SurveyComplete fact onto the queue with origin = (0,0,0)
    // so arrival is immediate from the Ruler's vantage at home.
    {
        let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
        queue.facts.push(PerceivedFact {
            fact: KnowledgeFact::SurveyComplete {
                event_id: None,
                system: target,
                system_name: "Target".into(),
                detail: String::new(),
                ship,
            },
            observed_at: 100,
            origin_pos: [0.0, 0.0, 0.0],
            arrives_at: 100,
            source: macrocosmo::knowledge::ObservationSource::Direct,
            related_system: Some(target),
        });
    }
    app.world_mut().resource_mut::<GameClock>().elapsed = 200;

    // Run the resolve system as a one-shot.
    let sys_id = app
        .world_mut()
        .register_system(resolve_commitments_from_observations);
    app.world_mut().run_system(sys_id).expect("system ok");

    // Commitment should now be Resolved.
    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    assert!(
        !store.has_active_commitment(
            KnowledgeSubject::Empire(empire),
            CommitmentKind::Survey,
            CommitmentTarget::System(target),
        ),
        "SurveyComplete arrival must resolve a matching Survey commitment"
    );
    let resolved = store
        .commitments()
        .iter()
        .filter(|c| c.status == CommitmentStatus::Resolved)
        .count();
    assert_eq!(
        resolved, 1,
        "exactly one commitment should have been marked Resolved"
    );
}

/// Slice 3: `ShipDestroyed` arrival fails all commitments for that
/// actor. Mirrors the projection terminal state.
#[test]
fn ship_destroyed_fails_actor_commitments() {
    use macrocosmo::knowledge::{
        KnowledgeFact, PendingFactQueue, PerceivedFact, resolve_commitments_from_observations,
    };
    use macrocosmo::time_system::GameClock;

    let mut app = test_app();
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "destroyed_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let target = spawn_test_system(
        app.world_mut(),
        "Target",
        [0.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    let ship = spawn_test_ship(app.world_mut(), "S", "explorer_mk1", home, [0.0, 0.0, 0.0]);
    spawn_test_ruler(app.world_mut(), empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.commitments_mut().record_issued(
            KnowledgeSubject::Empire(empire),
            Some(ship),
            CommitmentKind::Survey,
            CommitmentTarget::System(target),
            10,
        );
    }

    {
        let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
        queue.facts.push(PerceivedFact {
            fact: KnowledgeFact::ShipDestroyed {
                event_id: None,
                system: Some(target),
                ship_name: "S".into(),
                destroyed_at: 100,
                detail: String::new(),
                ship,
            },
            observed_at: 100,
            origin_pos: [0.0, 0.0, 0.0],
            arrives_at: 100,
            source: macrocosmo::knowledge::ObservationSource::Direct,
            related_system: Some(target),
        });
    }
    app.world_mut().resource_mut::<GameClock>().elapsed = 200;

    let sys_id = app
        .world_mut()
        .register_system(resolve_commitments_from_observations);
    app.world_mut().run_system(sys_id).expect("system ok");

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let failed = store
        .commitments()
        .iter()
        .filter(|c| c.status == CommitmentStatus::Failed)
        .count();
    assert_eq!(
        failed, 1,
        "ShipDestroyed arrival must fail this empire's commitment for that actor"
    );
}

/// Followup 1 (review 2026-05-29): `fail_commitments_for_actor` must
/// only touch commitments belonging to the empire whose
/// `KnowledgeStore` is currently being scanned. The current
/// empire-only producer cannot trigger this in practice, but the
/// invariant is load-bearing for the merge skeleton (Slice 7) and the
/// commitment ledger refactor (Slice 9).
///
/// Setup: seed the empire's ledger with TWO commitments for the same
/// ship actor — one whose `subject` matches the empire, one whose
/// `subject` belongs to a fictional other subject. After
/// `ShipDestroyed` for the ship arrives, only the matching-subject
/// entry must transition to Failed; the cross-subject entry must
/// stay Active.
#[test]
fn ship_destroyed_does_not_fail_cross_subject_actor_commitments() {
    use macrocosmo::knowledge::{
        KnowledgeFact, PendingFactQueue, PerceivedFact, resolve_commitments_from_observations,
    };
    use macrocosmo::time_system::GameClock;

    let mut app = test_app();
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "cross_subject_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let target = spawn_test_system(
        app.world_mut(),
        "Target",
        [0.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    let ship = spawn_test_ship(app.world_mut(), "S", "explorer_mk1", home, [0.0, 0.0, 0.0]);
    spawn_test_ruler(app.world_mut(), empire, home);

    // Pretend a different subject (a future fleet / region holder)
    // also keeps a commitment for the same ship actor in this store.
    let other_subject =
        KnowledgeSubject::Fleet(bevy::prelude::Entity::from_raw_u32(987654).expect("nonzero"));
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        // Empire-subject commitment (this is the one Slice 3 should fail).
        store.commitments_mut().record_issued(
            KnowledgeSubject::Empire(empire),
            Some(ship),
            CommitmentKind::Survey,
            CommitmentTarget::System(target),
            10,
        );
        // Cross-subject commitment (must stay Active).
        store.commitments_mut().record_issued(
            other_subject,
            Some(ship),
            CommitmentKind::Survey,
            CommitmentTarget::System(target),
            10,
        );
    }

    {
        let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
        queue.facts.push(PerceivedFact {
            fact: KnowledgeFact::ShipDestroyed {
                event_id: None,
                system: Some(target),
                ship_name: "S".into(),
                destroyed_at: 100,
                detail: String::new(),
                ship,
            },
            observed_at: 100,
            origin_pos: [0.0, 0.0, 0.0],
            arrives_at: 100,
            source: macrocosmo::knowledge::ObservationSource::Direct,
            related_system: Some(target),
        });
    }
    app.world_mut().resource_mut::<GameClock>().elapsed = 200;

    let sys_id = app
        .world_mut()
        .register_system(resolve_commitments_from_observations);
    app.world_mut().run_system(sys_id).expect("system ok");

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let mut empire_status = None;
    let mut other_status = None;
    for c in store.commitments().iter() {
        if c.subject == KnowledgeSubject::Empire(empire) {
            empire_status = Some(c.status);
        }
        if c.subject == other_subject {
            other_status = Some(c.status);
        }
    }
    assert_eq!(
        empire_status,
        Some(CommitmentStatus::Failed),
        "empire-subject commitment must be failed"
    );
    assert_eq!(
        other_status,
        Some(CommitmentStatus::Active),
        "cross-subject commitment must NOT be failed by an empire-scoped ShipDestroyed scan"
    );
}

/// Slice 4b1: dispatching `colonize_planet` must record a sibling
/// `Colonize(System)` entry alongside the planet-keyed entry so the
/// AI dedup query can answer "is this system being colonized?"
/// without going through the planet → system resolver.
#[test]
fn ai_colonize_planet_dispatch_records_planet_and_system_commitments() {
    use macrocosmo::ai::convert::{to_ai_entity, to_ai_faction, to_ai_system};
    use macrocosmo::galaxy::Planet;
    use macrocosmo_ai::{Command, CommandKindId, CommandValue};

    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app);
    app.update();

    // Spawn a planet in the frontier system so the dispatch can
    // resolve a real planet → system pair.
    let planet = app
        .world_mut()
        .spawn(Planet {
            name: "Frontier-1".into(),
            system: frontier,
            planet_type: "terrestrial".into(),
        })
        .id();

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship,
            name: "Scout-1".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InSystem,
            last_known_system: Some(home),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    let mut c = Command::new(
        CommandKindId::from("colonize_planet"),
        to_ai_faction(empire),
        0,
    );
    c.params.insert(
        "target_system".into(),
        CommandValue::System(to_ai_system(frontier)),
    );
    c.params.insert(
        "target_planet".into(),
        CommandValue::Entity(to_ai_entity(planet)),
    );
    c.params.insert("ship_count".into(), CommandValue::I64(1));
    c.params
        .insert("ship_0".into(), CommandValue::Entity(to_ai_entity(ship)));
    {
        let mut bus = app
            .world_mut()
            .resource_mut::<macrocosmo::ai::plugin::AiBusResource>();
        bus.emit_command(c);
    }
    app.world_mut().resource_mut::<GameClock>().elapsed = 100;
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let subject = KnowledgeSubject::Empire(empire);
    assert!(
        store.has_active_commitment(
            subject,
            CommitmentKind::Colonize,
            CommitmentTarget::Planet(planet),
        ),
        "planet-keyed commitment must be present"
    );
    assert!(
        store.has_active_commitment(
            subject,
            CommitmentKind::Colonize,
            CommitmentTarget::System(frontier),
        ),
        "Slice 4b1 sibling: system-keyed commitment must also be present so AI dedup answers \"is this system being colonized?\" without planet→system resolution"
    );
    // Two Colonize commitments for this specific planet/system pair —
    // the planet-keyed primary and the system-keyed sibling. Total
    // entry count is not asserted: the AI tick may emit unrelated
    // commitments (e.g. a Survey for the same frontier system) during
    // the same Update, which is orthogonal to the 4b1 contract.
    let colonize_for_pair: Vec<_> = store
        .commitments()
        .iter()
        .filter(|c| {
            c.subject == subject
                && c.status == CommitmentStatus::Active
                && c.kind == CommitmentKind::Colonize
                && (c.target == CommitmentTarget::Planet(planet)
                    || c.target == CommitmentTarget::System(frontier))
        })
        .collect();
    assert_eq!(
        colonize_for_pair.len(),
        2,
        "expected exactly the planet-keyed entry and the system-keyed sibling for the dispatched colonize_planet"
    );
}

/// Slice 4b2: a `deploy_deliverable` macro emission must record a
/// `DeployDeliverable(System)` commitment on the issuer empire's
/// ledger BEFORE eager macro decomposition turns it into the
/// `build → load → reposition → unload` primitive chain. Without
/// this pre-scan, the macro identity is lost by the time the
/// primitives reach `dispatch_ship_command_per_ship` and the dedup
/// can't answer "is this empire already deploying a Core here?".
#[test]
fn ai_deploy_deliverable_macro_records_commitment() {
    use macrocosmo::ai::convert::{to_ai_faction, to_ai_system};
    use macrocosmo_ai::{Command, CommandKindId, CommandValue};

    let mut app = test_app();
    let (empire, _home, frontier, _ship) = setup_scenario(&mut app);
    app.update();

    let mut c = Command::new(
        CommandKindId::from("deploy_deliverable"),
        to_ai_faction(empire),
        0,
    );
    c.params.insert(
        "target_system".into(),
        CommandValue::System(to_ai_system(frontier)),
    );
    c.params.insert(
        "deliverable".into(),
        CommandValue::Str("infrastructure_core".into()),
    );
    {
        let mut bus = app
            .world_mut()
            .resource_mut::<macrocosmo::ai::plugin::AiBusResource>();
        bus.emit_command(c);
    }
    app.world_mut().resource_mut::<GameClock>().elapsed = 100;
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let subject = KnowledgeSubject::Empire(empire);
    assert!(
        store.has_active_commitment(
            subject,
            CommitmentKind::DeployDeliverable,
            CommitmentTarget::System(frontier),
        ),
        "deploy_deliverable macro emission must record a DeployDeliverable commitment for the target system",
    );
}

/// Slice 4b2 resolution: a `ColonyEstablished` arrival at the
/// deployment target resolves the matching DeployDeliverable
/// commitment alongside the Colonize commitments.
#[test]
fn colony_established_resolves_deploy_commitment() {
    use macrocosmo::knowledge::{
        KnowledgeFact, PendingFactQueue, PerceivedFact, resolve_commitments_from_observations,
    };
    use macrocosmo::time_system::GameClock;

    let mut app = test_app();
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "deploy_resolve_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let target = spawn_test_system(
        app.world_mut(),
        "Target",
        [0.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    let planet = app
        .world_mut()
        .spawn(macrocosmo::galaxy::Planet {
            name: "P".into(),
            system: target,
            planet_type: "terrestrial".into(),
        })
        .id();
    spawn_test_ruler(app.world_mut(), empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.commitments_mut().record_issued(
            KnowledgeSubject::Empire(empire),
            None,
            CommitmentKind::DeployDeliverable,
            CommitmentTarget::System(target),
            10,
        );
    }
    {
        let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
        queue.facts.push(PerceivedFact {
            fact: KnowledgeFact::ColonyEstablished {
                event_id: None,
                system: target,
                planet,
                name: "P".into(),
                detail: String::new(),
            },
            observed_at: 100,
            origin_pos: [0.0, 0.0, 0.0],
            arrives_at: 100,
            source: macrocosmo::knowledge::ObservationSource::Direct,
            related_system: Some(target),
        });
    }
    app.world_mut().resource_mut::<GameClock>().elapsed = 200;

    let sys_id = app
        .world_mut()
        .register_system(resolve_commitments_from_observations);
    app.world_mut().run_system(sys_id).expect("system ok");

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    assert!(
        !store.has_active_commitment(
            KnowledgeSubject::Empire(empire),
            CommitmentKind::DeployDeliverable,
            CommitmentTarget::System(target),
        ),
        "ColonyEstablished arrival must resolve the DeployDeliverable commitment"
    );
}

#[test]
fn commitment_dual_write_does_not_double_issue() {
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app);
    app.update();

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship,
            name: "Scout-1".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InSystem,
            last_known_system: Some(home),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    dispatch_survey(&mut app, empire, ship, frontier);
    app.world_mut().resource_mut::<GameClock>().elapsed = 100;
    app.update();
    // Second emission for the same (subject, kind, target).
    dispatch_survey(&mut app, empire, ship, frontier);
    app.world_mut().resource_mut::<GameClock>().elapsed = 110;
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    // Ledger guard: still exactly one active entry. PendingAssignment
    // dedup is handled elsewhere; we're proving the ledger doesn't
    // grow unbounded under repeated dispatch of the same triple.
    let active: Vec<_> = store
        .commitments()
        .iter()
        .filter(|c| c.status == CommitmentStatus::Active)
        .collect();
    assert_eq!(
        active.len(),
        1,
        "repeated dispatch for the same (subject, kind, target) must not double-issue commitments — got {} active entries",
        active.len(),
    );
}
