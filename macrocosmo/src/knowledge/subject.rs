//! Subject identity for knowledge state.
//!
//! Slice 1 of the knowledge redesign (`docs/knowledge-redesign-implementation-plan.md`).
//!
//! This module introduces three additive types:
//!
//! * [`KnowledgeSubject`] — who is reasoning. Empire today; ship/fleet/region/colony
//!   tomorrow without API churn.
//! * [`KnowledgeScope`] — the responsibility boundary of a subject.
//! * [`KnowledgeNode`] — a Bevy [`Component`] tagging an entity as a knowledge holder.
//!
//! Slice 1 does NOT change the storage layout of [`super::KnowledgeStore`]. It only
//! makes "who owns knowledge" first-class so later slices (commitments, perception
//! facade) can be written against subject identity rather than `With<Empire>`
//! queries.

use bevy::prelude::*;

/// Who the knowledge belongs to.
///
/// Today only [`KnowledgeSubject::Empire`] is constructed at runtime; the other
/// variants exist so call sites can be written in terms of subject identity from
/// the start and ship/fleet/region/colony knowledge can land later without
/// breaking API consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub enum KnowledgeSubject {
    Empire(Entity),
    Region(Entity),
    Fleet(Entity),
    Ship(Entity),
    Colony(Entity),
}

impl KnowledgeSubject {
    /// The Bevy entity this subject is anchored to, regardless of variant.
    pub fn entity(self) -> Entity {
        match self {
            KnowledgeSubject::Empire(e)
            | KnowledgeSubject::Region(e)
            | KnowledgeSubject::Fleet(e)
            | KnowledgeSubject::Ship(e)
            | KnowledgeSubject::Colony(e) => e,
        }
    }
}

/// The responsibility boundary of a knowledge subject.
///
/// Scope is NOT a visibility filter on its own — it is the boundary that
/// belief materialization and decision making operate within. An empire's
/// global empire scope, a region governor's region scope, and a ship's
/// self scope all hold different beliefs derived from the same world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub enum KnowledgeScope {
    GlobalEmpire,
    Region(Entity),
    Fleet(Entity),
    LocalSystem(Entity),
    ShipSelf(Entity),
}

/// Marker component placed on a knowledge holder entity.
///
/// Slice 1 attaches this only to empires alongside the existing
/// [`super::KnowledgeStore`]. Later slices will attach it to ships, fleets,
/// regions, and colonies as those subjects gain their own state.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct KnowledgeNode {
    pub subject: KnowledgeSubject,
    pub parent: Option<Entity>,
    pub scope: KnowledgeScope,
}

impl KnowledgeNode {
    /// Build the canonical empire-level node: subject is the empire entity,
    /// scope is [`KnowledgeScope::GlobalEmpire`], no parent.
    pub fn empire(entity: Entity) -> Self {
        Self {
            subject: KnowledgeSubject::Empire(entity),
            parent: None,
            scope: KnowledgeScope::GlobalEmpire,
        }
    }
}
