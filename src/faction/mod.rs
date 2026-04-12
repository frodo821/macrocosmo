//! Faction relations data model.
//!
//! Manages **asymmetric pair** relations between factions. Light-speed
//! communication delay means A→B and B→A perceptions can desynchronize, so
//! each direction is stored independently.
//!
//! # State Transition Rules
//!
//! ```text
//! Neutral   → Peace      (mutual agreement)
//! Neutral   → War        (unilateral declaration)
//! Peace     → War        (unilateral declaration, breaks non-aggression)
//! Peace     → Alliance   (mutual agreement)
//! War       → Peace      (mutual agreement / treaty)
//! Alliance  → Peace      (unilateral termination)
//! Alliance  → War        (unilateral declaration)
//! ```
//!
//! This module implements **only unilateral transitions** (e.g.
//! [`FactionRelations::declare_war`]). Mutual-agreement transitions are
//! deferred to the diplomatic command system (see #171/#172).
//!
//! # Asymmetry / Light-speed delay
//!
//! When A declares war on B, A's view immediately becomes [`RelationState::War`].
//! B's view remains stale until light-speed propagation completes (#171).
//! This module captures only A's immediate side-effect; propagation to B is
//! handled elsewhere.
//!
//! Scope (this issue, #167): data model + helpers only. Combat integration
//! (#168), ROE updates (#169), Lua API (#170), light-speed propagation (#171),
//! and diplomatic UI (#174) are tracked separately.

use std::collections::HashMap;

use bevy::prelude::*;

/// Plugin that registers the [`FactionRelations`] resource and seeds the
/// non-empire "hostile" factions used by `HostilePresence` (#168).
pub struct FactionRelationsPlugin;

impl Plugin for FactionRelationsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FactionRelations>()
            .init_resource::<HostileFactions>()
            .add_systems(
                Startup,
                spawn_hostile_factions
                    .after(crate::player::spawn_player_empire)
                    .after(crate::galaxy::generate_galaxy),
            )
            .add_systems(
                Startup,
                attach_hostile_faction_owners
                    .after(spawn_hostile_factions),
            );
    }
}

/// Component that links a non-empire entity (e.g. `HostilePresence`, ship,
/// structure) to the [`Faction`](crate::player::Faction) entity that owns it.
///
/// Combat resolution and ROE checks consult [`FactionRelations`] keyed by the
/// player's faction entity and this owner. Entities without `FactionOwner`
/// have no diplomatic identity and are skipped by combat (#168 — minimal
/// migration: legacy spawns without FactionOwner do not trigger combat).
#[derive(Component, Clone, Copy, Debug)]
pub struct FactionOwner(pub Entity);

/// Resource holding the entity ids of the auto-spawned passive factions
/// (`space_creature`, `ancient_defense`). Populated by [`spawn_hostile_factions`].
///
/// `Option` so that startup ordering issues degrade gracefully — code that
/// reads this should `if let Some(e) = res.space_creature` rather than panic.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct HostileFactions {
    pub space_creature: Option<Entity>,
    pub ancient_defense: Option<Entity>,
}

/// Startup system that spawns the passive `space_creature` and
/// `ancient_defense` faction entities (#168 Step 1) and seeds default
/// `Neutral` + `standing = -100` relations between every existing
/// [`PlayerEmpire`](crate::player::PlayerEmpire) and these factions.
///
/// Idempotent: if [`HostileFactions`] already has entities, does nothing.
/// Faction entities have a [`crate::player::Faction`] component so that
/// existing UI/lookup code continues to work; they have **no** AI or
/// empire-level systems attached (passive presence only).
pub fn spawn_hostile_factions(
    mut commands: Commands,
    mut hostile_factions: ResMut<HostileFactions>,
    mut relations: ResMut<FactionRelations>,
    empires: Query<Entity, With<crate::player::PlayerEmpire>>,
) {
    if hostile_factions.space_creature.is_some() && hostile_factions.ancient_defense.is_some() {
        return;
    }

    let space_creature = commands
        .spawn(crate::player::Faction {
            id: "space_creature_faction".into(),
            name: "Space Creatures".into(),
        })
        .id();

    let ancient_defense = commands
        .spawn(crate::player::Faction {
            id: "ancient_defense_faction".into(),
            name: "Ancient Defenses".into(),
        })
        .id();

    hostile_factions.space_creature = Some(space_creature);
    hostile_factions.ancient_defense = Some(ancient_defense);

    // Seed Neutral + -100 (hostile) relations in both directions for each
    // existing PlayerEmpire. Tests that don't add a PlayerEmpire just get
    // the entities with no relations — `get_or_default` will return Neutral
    // with standing 0 and combat will not trigger. The combat gate explicitly
    // requires `can_attack_aggressive()` so missing relations are safe.
    for empire in &empires {
        relations.set(empire, space_creature, FactionView::new(RelationState::Neutral, -100.0));
        relations.set(space_creature, empire, FactionView::new(RelationState::Neutral, -100.0));
        relations.set(empire, ancient_defense, FactionView::new(RelationState::Neutral, -100.0));
        relations.set(ancient_defense, empire, FactionView::new(RelationState::Neutral, -100.0));
    }
}

/// Startup system that attaches a [`FactionOwner`] component to every
/// [`HostilePresence`](crate::galaxy::HostilePresence) generated by the
/// galaxy generator (#168 Step 2). Pairs each presence with the appropriate
/// passive faction entity from [`HostileFactions`] based on its
/// [`HostileType`](crate::galaxy::HostileType).
///
/// Idempotent: skips entities that already have a `FactionOwner`. Runs once
/// at startup but is structured so it could become a per-tick system later
/// to handle late-spawned hostiles.
pub fn attach_hostile_faction_owners(
    mut commands: Commands,
    hostile_factions: Res<HostileFactions>,
    hostiles: Query<(Entity, &crate::galaxy::HostilePresence), Without<FactionOwner>>,
) {
    let Some(space_creature) = hostile_factions.space_creature else { return; };
    let Some(ancient_defense) = hostile_factions.ancient_defense else { return; };

    let mut count = 0;
    for (entity, hostile) in &hostiles {
        let owner = match hostile.hostile_type {
            crate::galaxy::HostileType::SpaceCreature => space_creature,
            crate::galaxy::HostileType::AncientDefense => ancient_defense,
        };
        commands.entity(entity).try_insert(FactionOwner(owner));
        count += 1;
    }
    if count > 0 {
        info!("Attached FactionOwner to {} hostile presence(s)", count);
    }
}

/// Diplomatic state between two factions, viewed from a single direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelationState {
    /// No formal diplomatic relationship. Hostile actions may still occur if
    /// `standing < 0` and the actor's ROE allows it.
    Neutral,
    /// Non-aggression in force. Attack is forbidden until war is declared.
    Peace,
    /// Open hostilities. Any ROE may engage.
    War,
    /// Allied. Military cooperation; attack forbidden.
    Alliance,
}

impl Default for RelationState {
    fn default() -> Self {
        RelationState::Neutral
    }
}

impl RelationState {
    /// Parse a state string (case-insensitive). Returns the matching
    /// variant or an `mlua::Error` describing the unknown value.
    /// Used by the Lua API (`define_faction_type`) to accept string inputs.
    pub fn from_str(s: &str) -> Result<Self, mlua::Error> {
        match s.to_ascii_lowercase().as_str() {
            "neutral" => Ok(RelationState::Neutral),
            "peace" => Ok(RelationState::Peace),
            "war" => Ok(RelationState::War),
            "alliance" => Ok(RelationState::Alliance),
            other => Err(mlua::Error::RuntimeError(format!(
                "Unknown relation state '{other}': expected one of neutral/peace/war/alliance"
            ))),
        }
    }
}

/// One-directional view of the relation between two factions.
///
/// `(A, B)` and `(B, A)` are stored as independent [`FactionView`] entries
/// in [`FactionRelations`] so light-speed delayed updates can leave the two
/// directions temporarily inconsistent.
#[derive(Clone, Debug)]
pub struct FactionView {
    pub state: RelationState,
    /// Standing in `[-100.0, +100.0]`. Negative values indicate hostility,
    /// positive values indicate friendliness. Used to determine whether
    /// `Neutral` factions will attack each other under aggressive ROE.
    pub standing: f64,
}

impl Default for FactionView {
    fn default() -> Self {
        Self {
            state: RelationState::Neutral,
            standing: 0.0,
        }
    }
}

impl FactionView {
    /// Construct a view from a state and standing. Standing is clamped to
    /// `[-100.0, +100.0]`.
    pub fn new(state: RelationState, standing: f64) -> Self {
        Self {
            state,
            standing: standing.clamp(-100.0, 100.0),
        }
    }

    /// Set standing, clamping to `[-100.0, +100.0]`.
    pub fn set_standing(&mut self, value: f64) {
        self.standing = value.clamp(-100.0, 100.0);
    }

    /// Adjust standing by `delta`, clamping the result.
    pub fn adjust_standing(&mut self, delta: f64) {
        self.set_standing(self.standing + delta);
    }

    /// Whether the holder of this view may attack the target under
    /// `Aggressive` rules of engagement.
    ///
    /// - `War`: always allowed.
    /// - `Neutral`: allowed iff `standing < 0`.
    /// - `Peace` / `Alliance`: never allowed.
    pub fn can_attack_aggressive(&self) -> bool {
        match self.state {
            RelationState::War => true,
            RelationState::Neutral => self.standing < 0.0,
            RelationState::Peace | RelationState::Alliance => false,
        }
    }

    /// Whether attack is allowed under any ROE (i.e. open war).
    pub fn is_at_war(&self) -> bool {
        matches!(self.state, RelationState::War)
    }
}

/// Asymmetric registry of faction-to-faction relations.
///
/// Keyed by `(from, to)` faction entities. Each direction is independent;
/// `(A, B)` may be `War` while `(B, A)` is still `Peace` if the war
/// declaration has not yet propagated at light-speed.
#[derive(Resource, Default, Debug)]
pub struct FactionRelations {
    pub relations: HashMap<(Entity, Entity), FactionView>,
}

impl FactionRelations {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the view that `from` holds of `to`.
    pub fn get(&self, from: Entity, to: Entity) -> Option<&FactionView> {
        self.relations.get(&(from, to))
    }

    /// Get a mutable view that `from` holds of `to`.
    pub fn get_mut(&mut self, from: Entity, to: Entity) -> Option<&mut FactionView> {
        self.relations.get_mut(&(from, to))
    }

    /// Get the view, or a default `Neutral` view if not present.
    pub fn get_or_default(&self, from: Entity, to: Entity) -> FactionView {
        self.relations
            .get(&(from, to))
            .cloned()
            .unwrap_or_default()
    }

    /// Set the view that `from` holds of `to`.
    pub fn set(&mut self, from: Entity, to: Entity, view: FactionView) {
        self.relations.insert((from, to), view);
    }

    /// Remove the view that `from` holds of `to`, returning it if present.
    pub fn remove(&mut self, from: Entity, to: Entity) -> Option<FactionView> {
        self.relations.remove(&(from, to))
    }

    /// Number of stored directional views.
    pub fn len(&self) -> usize {
        self.relations.len()
    }

    /// Whether the registry has no stored views.
    pub fn is_empty(&self) -> bool {
        self.relations.is_empty()
    }

    /// Unilateral war declaration. Sets `from`'s view of `to` to
    /// [`RelationState::War`] and floors standing at `-50.0`.
    ///
    /// `to`'s view of `from` is **not** modified — propagation is
    /// performed asynchronously by the light-speed delivery system (#171).
    /// Inserts a default view first if none exists.
    pub fn declare_war(&mut self, from: Entity, to: Entity) {
        let view = self
            .relations
            .entry((from, to))
            .or_insert_with(FactionView::default);
        view.state = RelationState::War;
        if view.standing > -50.0 {
            view.standing = -50.0;
        }
    }

    /// Unilaterally break an alliance, returning to `Peace`. No-op if the
    /// view is not currently `Alliance`.
    pub fn break_alliance(&mut self, from: Entity, to: Entity) {
        if let Some(view) = self.relations.get_mut(&(from, to))
            && view.state == RelationState::Alliance
        {
            view.state = RelationState::Peace;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;

    /// Spawn `n` empty entities in a fresh World, return them as a vector.
    /// Provides distinct, valid `Entity` ids for tests.
    fn spawn_n(n: usize) -> Vec<Entity> {
        let mut world = World::new();
        (0..n).map(|_| world.spawn_empty().id()).collect()
    }

    fn pair() -> (Entity, Entity) {
        let v = spawn_n(2);
        (v[0], v[1])
    }

    // ---- FactionView basics ----

    #[test]
    fn view_default_is_neutral_zero() {
        let v = FactionView::default();
        assert_eq!(v.state, RelationState::Neutral);
        assert_eq!(v.standing, 0.0);
    }

    #[test]
    fn view_new_clamps_standing() {
        let v = FactionView::new(RelationState::Neutral, 250.0);
        assert_eq!(v.standing, 100.0);
        let v = FactionView::new(RelationState::Neutral, -250.0);
        assert_eq!(v.standing, -100.0);
        let v = FactionView::new(RelationState::Neutral, 42.0);
        assert_eq!(v.standing, 42.0);
    }

    #[test]
    fn view_set_and_adjust_standing_clamps() {
        let mut v = FactionView::default();
        v.set_standing(150.0);
        assert_eq!(v.standing, 100.0);
        v.set_standing(-200.0);
        assert_eq!(v.standing, -100.0);

        v.set_standing(0.0);
        v.adjust_standing(120.0);
        assert_eq!(v.standing, 100.0);
        v.adjust_standing(-300.0);
        assert_eq!(v.standing, -100.0);
    }

    // ---- can_attack_aggressive — full state × standing matrix ----

    #[test]
    fn can_attack_aggressive_war_always_true() {
        for &standing in &[-100.0, -1.0, 0.0, 1.0, 100.0] {
            let v = FactionView::new(RelationState::War, standing);
            assert!(
                v.can_attack_aggressive(),
                "War must allow attack regardless of standing ({standing})"
            );
        }
    }

    #[test]
    fn can_attack_aggressive_neutral_depends_on_standing() {
        // Negative standing → attack allowed
        for &standing in &[-100.0, -50.0, -0.0001] {
            let v = FactionView::new(RelationState::Neutral, standing);
            assert!(
                v.can_attack_aggressive(),
                "Neutral with standing={standing} should allow attack"
            );
        }
        // Zero or positive standing → attack forbidden
        for &standing in &[0.0, 0.0001, 50.0, 100.0] {
            let v = FactionView::new(RelationState::Neutral, standing);
            assert!(
                !v.can_attack_aggressive(),
                "Neutral with standing={standing} should forbid attack"
            );
        }
    }

    #[test]
    fn can_attack_aggressive_peace_always_false() {
        for &standing in &[-100.0, -1.0, 0.0, 1.0, 100.0] {
            let v = FactionView::new(RelationState::Peace, standing);
            assert!(
                !v.can_attack_aggressive(),
                "Peace must forbid attack regardless of standing ({standing})"
            );
        }
    }

    #[test]
    fn can_attack_aggressive_alliance_always_false() {
        for &standing in &[-100.0, -1.0, 0.0, 1.0, 100.0] {
            let v = FactionView::new(RelationState::Alliance, standing);
            assert!(
                !v.can_attack_aggressive(),
                "Alliance must forbid attack regardless of standing ({standing})"
            );
        }
    }

    #[test]
    fn is_at_war_only_true_for_war() {
        assert!(FactionView::new(RelationState::War, 0.0).is_at_war());
        assert!(!FactionView::new(RelationState::Neutral, -100.0).is_at_war());
        assert!(!FactionView::new(RelationState::Peace, 0.0).is_at_war());
        assert!(!FactionView::new(RelationState::Alliance, 100.0).is_at_war());
    }

    // ---- FactionRelations get/set ----

    #[test]
    fn relations_default_empty() {
        let r = FactionRelations::default();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn relations_set_and_get_roundtrip() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();

        assert!(r.get(a, b).is_none());
        r.set(a, b, FactionView::new(RelationState::Peace, 25.0));
        let v = r.get(a, b).unwrap();
        assert_eq!(v.state, RelationState::Peace);
        assert_eq!(v.standing, 25.0);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn relations_get_or_default_returns_neutral_for_missing() {
        let r = FactionRelations::new();
        let (a, b) = pair();
        let v = r.get_or_default(a, b);
        assert_eq!(v.state, RelationState::Neutral);
        assert_eq!(v.standing, 0.0);
    }

    #[test]
    fn relations_get_mut_allows_modification() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(a, b, FactionView::default());
        r.get_mut(a, b).unwrap().standing = 42.0;
        assert_eq!(r.get(a, b).unwrap().standing, 42.0);
    }

    #[test]
    fn relations_set_overwrites_existing() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(a, b, FactionView::new(RelationState::Peace, 10.0));
        r.set(a, b, FactionView::new(RelationState::War, -75.0));
        let v = r.get(a, b).unwrap();
        assert_eq!(v.state, RelationState::War);
        assert_eq!(v.standing, -75.0);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn relations_remove_returns_value() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(a, b, FactionView::new(RelationState::Peace, 5.0));
        let removed = r.remove(a, b).unwrap();
        assert_eq!(removed.state, RelationState::Peace);
        assert!(r.get(a, b).is_none());
        assert!(r.remove(a, b).is_none());
    }

    // ---- Asymmetry: (A,B) and (B,A) independent ----

    #[test]
    fn relations_are_asymmetric() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(a, b, FactionView::new(RelationState::War, -80.0));
        r.set(b, a, FactionView::new(RelationState::Peace, 10.0));

        let ab = r.get(a, b).unwrap();
        let ba = r.get(b, a).unwrap();
        assert_eq!(ab.state, RelationState::War);
        assert_eq!(ba.state, RelationState::Peace);
        assert_eq!(ab.standing, -80.0);
        assert_eq!(ba.standing, 10.0);
        assert_eq!(r.len(), 2);
    }

    // ---- declare_war: unilateral, asymmetric ----

    #[test]
    fn declare_war_only_changes_from_side() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(b, a, FactionView::new(RelationState::Peace, 30.0));

        r.declare_war(a, b);

        // A's view: now War
        let ab = r.get(a, b).unwrap();
        assert_eq!(ab.state, RelationState::War);
        assert!(ab.standing <= -50.0);

        // B's view: untouched (light-speed propagation handled elsewhere)
        let ba = r.get(b, a).unwrap();
        assert_eq!(ba.state, RelationState::Peace);
        assert_eq!(ba.standing, 30.0);
    }

    #[test]
    fn declare_war_creates_view_when_missing() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        assert!(r.get(a, b).is_none());

        r.declare_war(a, b);

        let ab = r.get(a, b).unwrap();
        assert_eq!(ab.state, RelationState::War);
        assert_eq!(ab.standing, -50.0);
        // B side never created
        assert!(r.get(b, a).is_none());
    }

    #[test]
    fn declare_war_floors_standing_but_keeps_lower_value() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(a, b, FactionView::new(RelationState::Peace, -90.0));

        r.declare_war(a, b);
        let ab = r.get(a, b).unwrap();
        assert_eq!(ab.state, RelationState::War);
        // Already lower than -50, must not be raised back to -50
        assert_eq!(ab.standing, -90.0);
    }

    #[test]
    fn declare_war_from_alliance_transitions_state() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(a, b, FactionView::new(RelationState::Alliance, 80.0));
        r.declare_war(a, b);
        let ab = r.get(a, b).unwrap();
        assert_eq!(ab.state, RelationState::War);
        assert_eq!(ab.standing, -50.0);
    }

    // ---- break_alliance ----

    #[test]
    fn break_alliance_demotes_to_peace() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(a, b, FactionView::new(RelationState::Alliance, 60.0));
        r.break_alliance(a, b);
        let ab = r.get(a, b).unwrap();
        assert_eq!(ab.state, RelationState::Peace);
        assert_eq!(ab.standing, 60.0); // standing unchanged
    }

    #[test]
    fn break_alliance_noop_when_not_alliance() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.set(a, b, FactionView::new(RelationState::Peace, 10.0));
        r.break_alliance(a, b);
        assert_eq!(r.get(a, b).unwrap().state, RelationState::Peace);

        r.set(a, b, FactionView::new(RelationState::War, -80.0));
        r.break_alliance(a, b);
        assert_eq!(r.get(a, b).unwrap().state, RelationState::War);
    }

    #[test]
    fn break_alliance_noop_when_missing() {
        let mut r = FactionRelations::new();
        let (a, b) = pair();
        r.break_alliance(a, b);
        assert!(r.get(a, b).is_none());
    }

    // ---- Plugin registers resource ----

    #[test]
    fn plugin_inits_resource() {
        let mut app = App::new();
        app.add_plugins(FactionRelationsPlugin);
        assert!(app.world().get_resource::<FactionRelations>().is_some());
        assert!(app.world().get_resource::<HostileFactions>().is_some());
    }

    // ---- #168: HostileFactions startup + FactionOwner attachment ----

    /// `spawn_hostile_factions` must populate the resource with two distinct
    /// faction entities (one for SpaceCreature, one for AncientDefense).
    #[test]
    fn spawn_hostile_factions_creates_two_distinct_entities() {
        use crate::player::{Empire, PlayerEmpire, Faction as PlayerFaction};
        let mut app = App::new();
        app.init_resource::<FactionRelations>();
        app.init_resource::<HostileFactions>();
        // Spawn a player empire so relations get seeded.
        app.world_mut().spawn((
            Empire { name: "Test".into() },
            PlayerEmpire,
            PlayerFaction { id: "test".into(), name: "Test".into() },
        ));
        app.add_systems(Update, spawn_hostile_factions);
        app.update();

        let hf = app.world().resource::<HostileFactions>();
        let space = hf.space_creature.expect("space_creature spawned");
        let ancient = hf.ancient_defense.expect("ancient_defense spawned");
        assert_ne!(space, ancient);

        // Both entities should carry the Faction component.
        assert!(app.world().get::<crate::player::Faction>(space).is_some());
        assert!(app.world().get::<crate::player::Faction>(ancient).is_some());
    }

    /// Default relations: empire→space_creature is Neutral with -100 standing.
    /// `can_attack_aggressive()` therefore returns true.
    #[test]
    fn spawn_hostile_factions_seeds_neutral_negative_relations() {
        use crate::player::{Empire, PlayerEmpire, Faction as PlayerFaction};
        let mut app = App::new();
        app.init_resource::<FactionRelations>();
        app.init_resource::<HostileFactions>();
        let empire = app.world_mut().spawn((
            Empire { name: "Test".into() },
            PlayerEmpire,
            PlayerFaction { id: "test".into(), name: "Test".into() },
        )).id();
        app.add_systems(Update, spawn_hostile_factions);
        app.update();

        let hf = *app.world().resource::<HostileFactions>();
        let rel = app.world().resource::<FactionRelations>();
        let view = rel.get(empire, hf.space_creature.unwrap()).expect("relation seeded");
        assert_eq!(view.state, RelationState::Neutral);
        assert!(view.standing < 0.0);
        assert!(view.can_attack_aggressive());
    }

    /// Idempotent: running the system twice doesn't spawn duplicate factions.
    #[test]
    fn spawn_hostile_factions_is_idempotent() {
        let mut app = App::new();
        app.init_resource::<FactionRelations>();
        app.init_resource::<HostileFactions>();
        app.add_systems(Update, spawn_hostile_factions);
        app.update();
        let first = *app.world().resource::<HostileFactions>();
        app.update();
        let second = *app.world().resource::<HostileFactions>();
        assert_eq!(first.space_creature, second.space_creature);
        assert_eq!(first.ancient_defense, second.ancient_defense);
    }

    /// `attach_hostile_faction_owners` should pair existing HostilePresence
    /// components with the matching faction entity and skip ones that already
    /// have a `FactionOwner`.
    #[test]
    fn attach_hostile_faction_owners_assigns_by_type() {
        use crate::galaxy::{HostilePresence, HostileType};

        let mut app = App::new();
        app.init_resource::<FactionRelations>();
        app.init_resource::<HostileFactions>();

        // Pre-populate HostileFactions so the attach system has something to use.
        let space = app.world_mut().spawn_empty().id();
        let ancient = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<HostileFactions>().space_creature = Some(space);
        app.world_mut().resource_mut::<HostileFactions>().ancient_defense = Some(ancient);

        let sys = app.world_mut().spawn_empty().id();
        let creature = app.world_mut().spawn(HostilePresence {
            system: sys, strength: 1.0, hp: 10.0, max_hp: 10.0,
            hostile_type: HostileType::SpaceCreature, evasion: 0.0,
        }).id();
        let ancient_e = app.world_mut().spawn(HostilePresence {
            system: sys, strength: 1.0, hp: 10.0, max_hp: 10.0,
            hostile_type: HostileType::AncientDefense, evasion: 0.0,
        }).id();
        // One pre-tagged entity should not be retagged.
        let preset = app.world_mut().spawn((HostilePresence {
            system: sys, strength: 1.0, hp: 10.0, max_hp: 10.0,
            hostile_type: HostileType::SpaceCreature, evasion: 0.0,
        }, FactionOwner(ancient))).id();

        app.add_systems(Update, attach_hostile_faction_owners);
        app.update();

        assert_eq!(app.world().get::<FactionOwner>(creature).unwrap().0, space);
        assert_eq!(app.world().get::<FactionOwner>(ancient_e).unwrap().0, ancient);
        // Pre-existing FactionOwner is untouched.
        assert_eq!(app.world().get::<FactionOwner>(preset).unwrap().0, ancient);
    }

    /// `attach_hostile_faction_owners` is a no-op if HostileFactions hasn't
    /// been populated yet (defensive — preserves the system order contract).
    #[test]
    fn attach_hostile_faction_owners_noop_when_factions_missing() {
        use crate::galaxy::{HostilePresence, HostileType};

        let mut app = App::new();
        app.init_resource::<FactionRelations>();
        app.init_resource::<HostileFactions>();

        let sys = app.world_mut().spawn_empty().id();
        let h = app.world_mut().spawn(HostilePresence {
            system: sys, strength: 1.0, hp: 10.0, max_hp: 10.0,
            hostile_type: HostileType::SpaceCreature, evasion: 0.0,
        }).id();

        app.add_systems(Update, attach_hostile_faction_owners);
        app.update();

        assert!(app.world().get::<FactionOwner>(h).is_none());
    }
}
