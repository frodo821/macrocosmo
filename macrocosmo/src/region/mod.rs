//! Sub-empire geographic instances. #449 PR2a.
//!
//! A `Region` is a slice of an empire's territory that one Mid-term agent
//! ("MidAgent") oversees. Initial state (PR2a): each empire spawns with
//! exactly one Region whose capital and only member is the empire's
//! `HomeSystem`. Multi-Region splits and the MidAgent population step land
//! in PR2b+.
//!
//! ## Bookkeeping invariant — Region ↔ RegionMembership
//!
//! `Region.member_systems: Vec<Entity>` and `RegionMembership.region:
//! Entity` are deliberately a **double index**:
//!
//! - `Region.member_systems` answers "which systems does this region
//!   own?" in one read of the Region entity.
//! - `RegionMembership` (attached to each StarSystem) answers "which
//!   region owns this system?" in O(1) without scanning every Region —
//!   used in the hot "ship is in system X → its owner Mid lives where?"
//!   lookup.
//!
//! Any mutation must keep them in sync: adding a system to a Region
//! requires both `Region.member_systems.push(sys)` AND
//! `world.entity_mut(sys).insert(RegionMembership { region })`. PR2a only
//! has the spawn path (one system per region), so the invariant is
//! trivial; PR2c (Region growth / merge / split) will add helpers that
//! enforce it.
//!
//! `RegionRegistry` is the empire→regions reverse index. Like the Region
//! ↔ RegionMembership pair, it is bookkeeping that must be kept in sync
//! with the actual `Region` entities by every spawn / despawn site.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use macrocosmo_ai::LongTermState;

/// Sub-empire geographic instance. One `MidAgent` will be attached per
/// `Region` in PR2b. PR2a spawns exactly one Region per Empire whose
/// `member_systems == [capital_system]`.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct Region {
    /// Owning empire.
    pub empire: Entity,
    /// StarSystem entities that belong to this region. Must stay in
    /// sync with each system's `RegionMembership` — see module docs.
    pub member_systems: Vec<Entity>,
    /// The Region's "headquarters" system. PR2b will place the MidAgent
    /// here; today it is just the capital seed and is always present in
    /// `member_systems`.
    pub capital_system: Entity,
    /// MidAgent entity managing this region. `None` until PR2b lands.
    pub mid_agent: Option<Entity>,
}

/// Reverse index attached to each StarSystem: which Region owns this
/// system? See module docs for the bookkeeping invariant with
/// `Region.member_systems`.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct RegionMembership {
    pub region: Entity,
}

/// Reverse index Resource: empire → list of its Region entities. Kept in
/// sync by every `spawn_initial_region` / Region split / Region merge
/// site.
#[derive(Resource, Reflect, Default, Debug)]
#[reflect(Resource)]
pub struct RegionRegistry {
    pub by_empire: HashMap<Entity, Vec<Entity>>,
}

/// Empire-level Component wrapping the engine-agnostic
/// `macrocosmo_ai::LongTermState`. Migrated out of
/// `OrchestratorState.long_state` in #449 PR2a (state-on-Component).
///
/// `LongTermState` is defined in the engine-agnostic `macrocosmo-ai`
/// crate, which cannot depend on `bevy_reflect` (CI:
/// `ai-core-isolation.yml`). The wrapper is registered with `Reflect`
/// here, but the inner field is `#[reflect(ignore)]` — exactly the same
/// pattern as `AiBusResource` (`crate::ai::plugin::AiBusResource`). The
/// wrapper still appears in BRP type queries; the inner state is opaque
/// to reflection but persistence is handled by postcard, not Reflect, so
/// save/load is unaffected.
#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component)]
pub struct EmpireLongTermState {
    /// Engine-agnostic strategic memory (pursued metrics, victory
    /// progress, current campaign phase). Schema continues to evolve
    /// in `macrocosmo-ai` — wrapper is intentionally a thin newtype.
    #[reflect(ignore)]
    pub inner: LongTermState,
}

/// Spawn a Region entity for `empire`, anchored at `home_system`. Returns
/// the new Region entity. Idempotent at the registry level: re-calling
/// for the same empire **appends** another region (callers responsible
/// for not re-spawning during ordinary game start).
///
/// Side effects:
/// 1. Spawn a `Region` entity with `member_systems = [home_system]`,
///    `capital_system = home_system`, `mid_agent = None`.
/// 2. Insert `RegionMembership { region }` on `home_system`.
/// 3. Push the new region entity into
///    `RegionRegistry.by_empire[empire]` (creating the Vec if absent).
pub fn spawn_initial_region(world: &mut World, empire: Entity, home_system: Entity) -> Entity {
    let region = world
        .spawn(Region {
            empire,
            member_systems: vec![home_system],
            capital_system: home_system,
            mid_agent: None,
        })
        .id();
    world
        .entity_mut(home_system)
        .insert(RegionMembership { region });
    let mut registry = world.resource_mut::<RegionRegistry>();
    registry.by_empire.entry(empire).or_default().push(region);
    region
}
