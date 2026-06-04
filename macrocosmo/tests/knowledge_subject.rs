//! Slice 1 of the knowledge redesign
//! (see `docs/knowledge-redesign-implementation-plan.md`).
//!
//! Verifies the new subject identity types are wired up:
//!
//! * `KnowledgeSubject`, `KnowledgeScope`, `KnowledgeNode`,
//!   `Perception`, `PerceptionMode` are re-exported from
//!   `macrocosmo::knowledge`.
//! * `spawn_player_empire` attaches both `KnowledgeStore` and
//!   `KnowledgeNode` to the spawned empire entity, and the node's
//!   subject points back at the empire itself.
//!
//! Slice 1 is intentionally additive — `KnowledgeStore` storage and
//! every existing `Query<&KnowledgeStore, With<Empire>>` callsite are
//! unchanged. This test guards against regressions where a future
//! refactor of the spawn path forgets to attach the node.

use bevy::prelude::*;
use macrocosmo::knowledge::{
    KnowledgeNode, KnowledgeScope, KnowledgeStore, KnowledgeSubject, Perception, PerceptionMode,
};
use macrocosmo::player::{PlayerEmpire, spawn_player_empire};

fn run_spawn_player_empire() -> (World, Entity) {
    let mut world = World::new();
    // `spawn_player_empire` is a normal Bevy system; running it as a
    // one-shot exercises the production spawn path without needing the
    // full app harness.
    let id = world.register_system(spawn_player_empire);
    world.run_system(id).expect("spawn_player_empire ok");
    let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
    let empire = q.single(&world).expect("one player empire");
    (world, empire)
}

#[test]
fn player_empire_carries_knowledge_store_and_node() {
    let (world, empire) = run_spawn_player_empire();

    // Existing component preserved.
    assert!(
        world.get::<KnowledgeStore>(empire).is_some(),
        "spawn_player_empire must still attach KnowledgeStore"
    );
    // New subject identity attached.
    let node = world
        .get::<KnowledgeNode>(empire)
        .expect("spawn_player_empire must attach KnowledgeNode");

    assert_eq!(node.subject, KnowledgeSubject::Empire(empire));
    assert_eq!(node.scope, KnowledgeScope::GlobalEmpire);
    assert!(node.parent.is_none());
}

#[test]
fn knowledge_subject_entity_extraction() {
    let e = Entity::from_raw_u32(42).expect("nonzero index");
    assert_eq!(KnowledgeSubject::Empire(e).entity(), e);
    assert_eq!(KnowledgeSubject::Region(e).entity(), e);
    assert_eq!(KnowledgeSubject::Fleet(e).entity(), e);
    assert_eq!(KnowledgeSubject::Ship(e).entity(), e);
    assert_eq!(KnowledgeSubject::Colony(e).entity(), e);
}

#[test]
fn perception_mode_helpers() {
    let e = Entity::from_raw_u32(7).expect("nonzero index");
    let subj = KnowledgeSubject::Empire(e);

    let omni = PerceptionMode::Omniscient;
    assert!(omni.is_omniscient());
    assert!(omni.subject().is_none());

    let sub = PerceptionMode::Subject(subj);
    assert!(!sub.is_omniscient());
    assert_eq!(sub.subject(), Some(subj));

    let store = KnowledgeStore::default();
    let p = Perception::subject(subj, Some(&store));
    assert!(!p.is_omniscient());
    assert!(p.knowledge.is_some());

    let o = Perception::omniscient();
    assert!(o.is_omniscient());
    assert!(o.knowledge.is_none());
}

/// `KnowledgeNode::empire` is the canonical helper used by both player
/// and NPC spawn paths. Regression guard: scope is `GlobalEmpire`,
/// subject is `Empire(self)`, parent is `None`.
#[test]
fn knowledge_node_empire_helper() {
    let e = Entity::from_raw_u32(99).expect("nonzero index");
    let node = KnowledgeNode::empire(e);
    assert_eq!(node.subject, KnowledgeSubject::Empire(e));
    assert_eq!(node.scope, KnowledgeScope::GlobalEmpire);
    assert!(node.parent.is_none());
}

/// Slice 1.5 backfill: an empire spawned with `KnowledgeStore` but
/// *without* `KnowledgeNode` (= simulated load path / test setup)
/// must gain a node after `backfill_knowledge_node` runs.
#[test]
fn backfill_inserts_knowledge_node_on_loaded_empire() {
    use macrocosmo::knowledge::backfill_knowledge_node;
    use macrocosmo::player::Empire;

    let mut world = World::new();
    let empire = world
        .spawn((
            Empire {
                name: "Loaded".into(),
            },
            KnowledgeStore::default(),
        ))
        .id();
    assert!(
        world.get::<KnowledgeNode>(empire).is_none(),
        "precondition: empire intentionally spawned without KnowledgeNode"
    );
    let sys = world.register_system(backfill_knowledge_node);
    world.run_system(sys).expect("system ok");
    let node = world
        .get::<KnowledgeNode>(empire)
        .expect("backfill must attach KnowledgeNode");
    assert_eq!(node.subject, KnowledgeSubject::Empire(empire));
    assert_eq!(node.scope, KnowledgeScope::GlobalEmpire);
    assert!(node.parent.is_none());
}

/// Slice 1.5 backfill: idempotent — running the system again on a
/// world where the invariant already holds inserts nothing.
#[test]
fn backfill_is_idempotent_when_node_present() {
    use macrocosmo::knowledge::backfill_knowledge_node;
    use macrocosmo::player::Empire;

    let mut world = World::new();
    let empire = world
        .spawn((
            Empire {
                name: "Already-tagged".into(),
            },
            KnowledgeStore::default(),
            KnowledgeNode::empire(Entity::from_raw_u32(1).unwrap()),
        ))
        .id();
    let sys = world.register_system(backfill_knowledge_node);
    world.run_system(sys).expect("system ok");
    world.run_system(sys).expect("system ok again");
    // Original node's subject (placeholder Entity(1)) must be unchanged
    // — backfill only inserts on empires lacking the node.
    let node = world.get::<KnowledgeNode>(empire).expect("still present");
    assert_eq!(
        node.subject,
        KnowledgeSubject::Empire(Entity::from_raw_u32(1).unwrap()),
        "backfill must NOT overwrite an existing KnowledgeNode"
    );
}
