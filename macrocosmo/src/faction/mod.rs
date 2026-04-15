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
//!
//! # Light-speed delayed diplomacy (#171)
//!
//! [`DiplomaticAction`] + [`PendingDiplomaticAction`] model formal diplomatic
//! actions that propagate at light-speed. Helpers such as
//! [`declare_war_with_delay`] apply the **sender side** immediately and spawn a
//! `PendingDiplomaticAction` that the [`tick_diplomatic_actions`] system applies
//! to the **receiver** when `arrives_at <= clock.elapsed`. Mutual-agreement
//! actions (peace, alliance) use a two-leg pattern: the proposal arrives at the
//! receiver (one-way delay), which auto-accepts and queues a reply that arrives
//! at the sender after another one-way delay. This produces the round-trip
//! delay that opens a window for surprise attacks (declare-war that is still
//! in flight).

use std::collections::HashMap;

use bevy::prelude::*;

/// Plugin that registers the [`FactionRelations`] resource and seeds the
/// non-empire "hostile" factions used by hostile entities (#168/#293).
pub struct FactionRelationsPlugin;

impl Plugin for FactionRelationsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FactionRelations>()
            .init_resource::<HostileFactions>()
            .add_systems(
                Startup,
                // #293 follow-up: `spawn_hostile_factions` runs before
                // `generate_galaxy` so hostiles can be spawned with
                // `FactionOwner` directly. `generate_galaxy` carries the
                // reverse `.after(spawn_hostile_factions)` in galaxy/mod.rs.
                spawn_hostile_factions.after(crate::player::spawn_player_empire),
            )
            // #173: After NPC empires have been promoted to `Empire`
            // entities by `run_all_factions_on_game_start`, seed their
            // relations against the passive hostile factions and against
            // each other. `spawn_hostile_factions` only seeds
            // PlayerEmpire ↔ hostile pairs, so NPCs would otherwise see
            // hostiles as Neutral / standing=0 and never engage under the
            // aggressive ROE.
            .add_systems(
                Startup,
                seed_npc_relations
                    .after(spawn_hostile_factions)
                    .after(crate::setup::run_all_factions_on_game_start),
            )
            .add_systems(
                Update,
                tick_diplomatic_actions.after(crate::time_system::advance_game_time),
            )
            .add_systems(
                Update,
                tick_custom_diplomatic_actions
                    .after(crate::time_system::advance_game_time)
                    .after(tick_diplomatic_actions),
            );
    }
}

/// Startup system (#173) that seeds NPC empire relations after
/// `run_all_factions_on_game_start` has spawned NPC `Empire` entities.
///
/// Seeds two kinds of relations:
/// 1. `NPC ↔ passive hostile` (space_creature, ancient_defense) with
///    `Neutral` + `standing = -100`, mirroring
///    [`spawn_hostile_factions`] for PlayerEmpires.
/// 2. `NPC ↔ NPC` with `Neutral` + `standing = 0` (no pre-existing
///    hostility — diplomacy can evolve through #172 actions).
///
/// The player empire is deliberately left alone here; its relations are
/// seeded by [`spawn_hostile_factions`].
pub fn seed_npc_relations(
    mut relations: ResMut<FactionRelations>,
    hostiles: Res<HostileFactions>,
    npcs: Query<Entity, (With<crate::player::Empire>, Without<crate::player::PlayerEmpire>)>,
) {
    let npc_entities: Vec<Entity> = npcs.iter().collect();
    if npc_entities.is_empty() {
        return;
    }

    // 1. NPC ↔ passive hostiles.
    for &npc in &npc_entities {
        if let Some(sc) = hostiles.space_creature {
            relations.set(npc, sc, FactionView::new(RelationState::Neutral, -100.0));
            relations.set(sc, npc, FactionView::new(RelationState::Neutral, -100.0));
        }
        if let Some(ad) = hostiles.ancient_defense {
            relations.set(npc, ad, FactionView::new(RelationState::Neutral, -100.0));
            relations.set(ad, npc, FactionView::new(RelationState::Neutral, -100.0));
        }
    }

    // 2. NPC ↔ NPC (other direction picked up on the symmetric iteration).
    for (i, &a) in npc_entities.iter().enumerate() {
        for &b in &npc_entities[i + 1..] {
            relations.set(a, b, FactionView::new(RelationState::Neutral, 0.0));
            relations.set(b, a, FactionView::new(RelationState::Neutral, 0.0));
        }
    }
}

/// Component that links a non-empire entity (e.g. hostile, ship,
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

    /// Whether the holder of this view should engage the target under
    /// `Defensive` rules of engagement.
    ///
    /// Defensive ROE never starts a fight on its own from low standing alone:
    /// it only engages when the relation is open `War`, or when a hostile
    /// action is in progress (`being_attacked`). The latter allows a unit to
    /// retaliate even against a faction whose view is still `Peace` /
    /// `Alliance` from the holder's side — useful when the standing/state
    /// information is stale due to light-speed propagation.
    ///
    /// Used by [`crate::ship::combat::resolve_combat`] (#169). The
    /// `being_attacked` flag is currently inferred from the presence of a
    /// hostile entity in the same star system; a more granular,
    /// damage-event-driven variant is tracked separately.
    pub fn should_engage_defensive(&self, being_attacked: bool) -> bool {
        self.is_at_war() || being_attacked
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

    /// Set `from`'s view of `to` to [`RelationState::Peace`], preserving
    /// existing standing. Inserts a default view first if none exists.
    /// Used by [`tick_diplomatic_actions`] to apply mutual-agreement results.
    pub fn make_peace(&mut self, from: Entity, to: Entity) {
        let view = self
            .relations
            .entry((from, to))
            .or_insert_with(FactionView::default);
        view.state = RelationState::Peace;
    }

    /// Set `from`'s view of `to` to [`RelationState::Alliance`], preserving
    /// existing standing. Inserts a default view first if none exists.
    pub fn make_alliance(&mut self, from: Entity, to: Entity) {
        let view = self
            .relations
            .entry((from, to))
            .or_insert_with(FactionView::default);
        view.state = RelationState::Alliance;
    }
}

// ---------------------------------------------------------------------------
// #171: Light-speed delayed diplomacy
// ---------------------------------------------------------------------------

/// Type of diplomatic action carried by a [`PendingDiplomaticAction`].
///
/// - **Unilateral** ([`DeclareWar`](DiplomaticAction::DeclareWar),
///   [`BreakAlliance`](DiplomaticAction::BreakAlliance)): the sender's view
///   changes immediately when the helper is called; this variant is delivered
///   to the receiver after a one-way light-speed delay.
/// - **Mutual** ([`ProposePeace`](DiplomaticAction::ProposePeace),
///   [`ProposeAlliance`](DiplomaticAction::ProposeAlliance)): the proposal
///   travels to the receiver (one-way), which auto-accepts and queues an
///   [`AcceptPeace`](DiplomaticAction::AcceptPeace) /
///   [`AcceptAlliance`](DiplomaticAction::AcceptAlliance) for the return trip.
/// - **Acceptance** ([`AcceptPeace`](DiplomaticAction::AcceptPeace),
///   [`AcceptAlliance`](DiplomaticAction::AcceptAlliance)): emitted internally
///   by [`tick_diplomatic_actions`] for the return leg of a mutual proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiplomaticAction {
    DeclareWar,
    ProposePeace,
    ProposeAlliance,
    BreakAlliance,
    AcceptPeace,
    AcceptAlliance,
    /// Lua-defined custom action (#172). The `String` is the action id
    /// registered in
    /// [`crate::scripting::faction_api::DiplomaticActionRegistry`]. When the
    /// message arrives, [`tick_custom_diplomatic_actions`] looks up the
    /// definition and invokes its optional `on_accepted` callback.
    CustomAction(String),
}

/// In-flight diplomatic message awaiting delivery.
///
/// Spawned by helpers such as [`declare_war_with_delay`] and drained by
/// [`tick_diplomatic_actions`] when `arrives_at <= clock.elapsed`. Each
/// message carries the `from`/`to` faction entities, the [`DiplomaticAction`]
/// to apply, the absolute hexadies timestamp at which it should land, and the
/// one-way delay used to schedule it (so a mutual-agreement proposal can
/// queue an acceptance for the return leg with the same delay).
#[derive(Component, Clone, Debug)]
pub struct PendingDiplomaticAction {
    /// Faction that originated the action.
    pub from: Entity,
    /// Faction the action is delivered to.
    pub to: Entity,
    /// What to apply on arrival.
    pub action: DiplomaticAction,
    /// Absolute hexadies timestamp at which the action is delivered.
    pub arrives_at: i64,
    /// One-way light-speed delay (hexadies) used when this message was
    /// scheduled. Reused by [`tick_diplomatic_actions`] to schedule the
    /// return leg of mutual-agreement proposals at the same cadence.
    pub one_way_delay_hexadies: i64,
}

/// Sender-side immediate war declaration plus a delayed receiver-side war
/// transition.
///
/// `from`'s view of `to` is set to [`RelationState::War`] right away (sender
/// "knows" they declared war). A [`PendingDiplomaticAction`] entity is spawned
/// so that `to`'s view of `from` flips to `War` only after `delay_hexadies`
/// have elapsed. The window between the two updates is the surprise-attack
/// window — the receiver still sees `Peace`/`Neutral` and a `Defensive` ROE
/// will not retaliate.
pub fn declare_war_with_delay(
    commands: &mut Commands,
    relations: &mut FactionRelations,
    clock: &crate::time_system::GameClock,
    from: Entity,
    to: Entity,
    delay_hexadies: i64,
) {
    let delay = delay_hexadies.max(0);
    relations.declare_war(from, to);
    commands.spawn(PendingDiplomaticAction {
        from,
        to,
        action: DiplomaticAction::DeclareWar,
        arrives_at: clock.elapsed + delay,
        one_way_delay_hexadies: delay,
    });
}

/// Sender-side immediate alliance termination plus a delayed receiver-side
/// transition. Mirrors [`declare_war_with_delay`] but transitions to
/// [`RelationState::Peace`] rather than `War`.
///
/// If `from`'s view of `to` is not currently `Alliance` the call is a no-op
/// on the sender side (matching [`FactionRelations::break_alliance`]); the
/// pending message is still spawned so the receiver finds out (and the
/// receiver-side handler is itself a no-op if their view is no longer
/// `Alliance`).
pub fn break_alliance_with_delay(
    commands: &mut Commands,
    relations: &mut FactionRelations,
    clock: &crate::time_system::GameClock,
    from: Entity,
    to: Entity,
    delay_hexadies: i64,
) {
    let delay = delay_hexadies.max(0);
    relations.break_alliance(from, to);
    commands.spawn(PendingDiplomaticAction {
        from,
        to,
        action: DiplomaticAction::BreakAlliance,
        arrives_at: clock.elapsed + delay,
        one_way_delay_hexadies: delay,
    });
}

/// Spawn a peace proposal in flight to `to`.
///
/// Implemented as auto-accept (AI-driven acceptance is #189). When the
/// proposal arrives, `to`'s view of `from` is set to [`RelationState::Peace`]
/// and an [`DiplomaticAction::AcceptPeace`] is queued for the return trip;
/// when that lands, `from`'s view of `to` becomes `Peace`. Total round-trip
/// time is `2 * delay_hexadies`.
pub fn propose_peace_with_delay(
    commands: &mut Commands,
    clock: &crate::time_system::GameClock,
    from: Entity,
    to: Entity,
    delay_hexadies: i64,
) {
    let delay = delay_hexadies.max(0);
    commands.spawn(PendingDiplomaticAction {
        from,
        to,
        action: DiplomaticAction::ProposePeace,
        arrives_at: clock.elapsed + delay,
        one_way_delay_hexadies: delay,
    });
}

/// Spawn an alliance proposal in flight to `to`. See
/// [`propose_peace_with_delay`] for the round-trip semantics.
pub fn propose_alliance_with_delay(
    commands: &mut Commands,
    clock: &crate::time_system::GameClock,
    from: Entity,
    to: Entity,
    delay_hexadies: i64,
) {
    let delay = delay_hexadies.max(0);
    commands.spawn(PendingDiplomaticAction {
        from,
        to,
        action: DiplomaticAction::ProposeAlliance,
        arrives_at: clock.elapsed + delay,
        one_way_delay_hexadies: delay,
    });
}

/// System: drain every [`PendingDiplomaticAction`] whose `arrives_at` has
/// passed and apply its effect to [`FactionRelations`].
///
/// **Ordering.** Must run `.after(advance_game_time)` so that newly elapsed
/// hexadies are visible. Registered by [`FactionRelationsPlugin`].
///
/// **Mutual-agreement return leg.** When a `ProposePeace` /
/// `ProposeAlliance` arrives at the receiver, the receiver's view is
/// updated immediately and a return-leg `AcceptPeace` / `AcceptAlliance`
/// pending message is spawned with the same delay (the round-trip equals
/// 2× the one-way delay). The auto-acceptance models a friendly NPC; a
/// future AI (#189) will replace this with a decision step.
pub fn tick_diplomatic_actions(
    mut commands: Commands,
    clock: Res<crate::time_system::GameClock>,
    mut relations: ResMut<FactionRelations>,
    pending: Query<(Entity, &PendingDiplomaticAction)>,
) {
    let now = clock.elapsed;
    // Snapshot the arrived messages so we can despawn / spawn freely.
    // `DiplomaticAction::CustomAction` carries a String so we clone rather
    // than copy; the match below skips `CustomAction` and defers it to
    // [`tick_custom_diplomatic_actions`] which has access to the Lua engine.
    let arrived: Vec<(Entity, PendingDiplomaticAction)> = pending
        .iter()
        .filter(|(_, p)| p.arrives_at <= now)
        .map(|(e, p)| (e, p.clone()))
        .collect();

    for (entity, msg) in arrived {
        match &msg.action {
            DiplomaticAction::DeclareWar => {
                // Receiver's view (to → from) flips to War.
                relations.declare_war(msg.to, msg.from);
            }
            DiplomaticAction::BreakAlliance => {
                // Receiver's view (to → from) drops Alliance → Peace.
                relations.break_alliance(msg.to, msg.from);
            }
            DiplomaticAction::ProposePeace => {
                // Receiver auto-accepts: their view of the proposer becomes
                // Peace immediately, and an acceptance is queued for the
                // return leg with the same one-way delay.
                relations.make_peace(msg.to, msg.from);
                commands.spawn(PendingDiplomaticAction {
                    from: msg.to,
                    to: msg.from,
                    action: DiplomaticAction::AcceptPeace,
                    arrives_at: now + msg.one_way_delay_hexadies,
                    one_way_delay_hexadies: msg.one_way_delay_hexadies,
                });
            }
            DiplomaticAction::ProposeAlliance => {
                relations.make_alliance(msg.to, msg.from);
                commands.spawn(PendingDiplomaticAction {
                    from: msg.to,
                    to: msg.from,
                    action: DiplomaticAction::AcceptAlliance,
                    arrives_at: now + msg.one_way_delay_hexadies,
                    one_way_delay_hexadies: msg.one_way_delay_hexadies,
                });
            }
            DiplomaticAction::AcceptPeace => {
                // Acceptance lands at original sender; their view of the
                // (now-accepted) target becomes Peace.
                relations.make_peace(msg.to, msg.from);
            }
            DiplomaticAction::AcceptAlliance => {
                relations.make_alliance(msg.to, msg.from);
            }
            DiplomaticAction::CustomAction(_) => {
                // Custom actions need the Lua engine to run `on_accepted`.
                // Handled by `tick_custom_diplomatic_actions` which sees the
                // same entity but has the extra resources it needs. Skip
                // here so the message isn't despawned.
                continue;
            }
        }

        commands.entity(entity).despawn();
    }
}

/// Spawn a [`PendingDiplomaticAction`] carrying a Lua-defined custom action
/// (#172). No sender-side state change is performed — the effect (if any)
/// is entirely described by the action's `on_accepted` Lua callback, applied
/// when the message arrives at the receiver.
///
/// Callers are expected to have validated availability via
/// [`crate::scripting::faction_api::DiplomaticActionDefinition::is_available`]
/// first; this helper deliberately does not re-check, matching the pattern
/// of [`declare_war_with_delay`] et al.
pub fn send_custom_diplomatic_action(
    commands: &mut Commands,
    clock: &crate::time_system::GameClock,
    from: Entity,
    to: Entity,
    action_id: impl Into<String>,
    delay_hexadies: i64,
) {
    let delay = delay_hexadies.max(0);
    commands.spawn(PendingDiplomaticAction {
        from,
        to,
        action: DiplomaticAction::CustomAction(action_id.into()),
        arrives_at: clock.elapsed + delay,
        one_way_delay_hexadies: delay,
    });
}

/// System: drain every arrived [`PendingDiplomaticAction`] whose
/// `action` is a [`DiplomaticAction::CustomAction`] and invoke the action's
/// Lua `on_accepted` callback (#172).
///
/// Applies returned [`crate::effect::DescriptiveEffect`] records to the
/// sender-side `PlayerEmpire`'s flag/param state in the same way tech
/// research effects do. Receiver-side application is left to a future pass
/// (factions other than the player have no flag store to write into today).
///
/// **Ordering.** Must run after [`tick_diplomatic_actions`] so the two
/// systems see the same set of arrived messages in a consistent order.
pub fn tick_custom_diplomatic_actions(
    mut commands: Commands,
    clock: Res<crate::time_system::GameClock>,
    engine: Option<Res<crate::scripting::ScriptEngine>>,
    registry: Option<Res<crate::scripting::faction_api::DiplomaticActionRegistry>>,
    pending: Query<(Entity, &PendingDiplomaticAction)>,
    mut empire_q: Query<
        (
            Entity,
            &mut crate::technology::GameFlags,
            &mut crate::condition::ScopedFlags,
            &mut crate::technology::GlobalParams,
        ),
        With<crate::player::PlayerEmpire>,
    >,
) {
    let now = clock.elapsed;
    let arrived: Vec<(Entity, PendingDiplomaticAction)> = pending
        .iter()
        .filter(|(_, p)| {
            p.arrives_at <= now && matches!(p.action, DiplomaticAction::CustomAction(_))
        })
        .map(|(e, p)| (e, p.clone()))
        .collect();

    if arrived.is_empty() {
        return;
    }

    let Some(engine) = engine else {
        // No Lua available (headless tests) — despawn messages to avoid
        // piling up but don't attempt callbacks.
        for (entity, _) in arrived {
            commands.entity(entity).despawn();
        }
        return;
    };
    let lua = engine.lua();

    for (entity, msg) in arrived {
        let action_id = match &msg.action {
            DiplomaticAction::CustomAction(id) => id.clone(),
            _ => continue,
        };

        // Look up the definition; unknown ids are logged and dropped.
        let def_present = registry
            .as_ref()
            .map(|r| r.get(&action_id).is_some())
            .unwrap_or(false);
        if !def_present {
            warn!(
                "Custom diplomatic action '{}' not found in registry; dropping",
                action_id
            );
            commands.entity(entity).despawn();
            continue;
        }

        // Resolve the on_accepted function (may legitimately be absent).
        let func_opt =
            match crate::scripting::faction_api::lookup_on_accepted(lua, &action_id) {
                Ok(f) => f,
                Err(e) => {
                    warn!(
                        "Failed to look up on_accepted for '{}': {e}",
                        action_id
                    );
                    commands.entity(entity).despawn();
                    continue;
                }
            };

        if let Some(func) = func_opt {
            let scope = crate::scripting::effect_scope::EffectScope::new();
            let call_result = func.call::<mlua::Value>(scope.clone());
            let effects = match call_result {
                Ok(v) => {
                    match crate::scripting::effect_scope::collect_effects(&scope, v) {
                        Ok(effects) => effects,
                        Err(e) => {
                            warn!(
                                "collect_effects failed for custom diplomatic action '{}': {e}",
                                action_id
                            );
                            Vec::new()
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "on_accepted callback failed for '{}': {e}",
                        action_id
                    );
                    Vec::new()
                }
            };

            if !effects.is_empty()
                && let Ok((_, mut game_flags, mut scoped_flags, mut global_params)) =
                    empire_q.single_mut()
            {
                for effect in &effects {
                    apply_custom_action_effect(
                        effect,
                        &mut game_flags,
                        &mut scoped_flags,
                        &mut global_params,
                    );
                }
                info!(
                    "Applied {} effect(s) for custom diplomatic action '{}'",
                    effects.len(),
                    action_id
                );
            }

            // #332-B3: removed `_pending_flags` / `_pending_global_mods`
            // drain — faction action callbacks emit `EffectScope`
            // descriptors only, which are collected above. The legacy
            // global `set_flag(name)` / `modify_global(...)` helpers are
            // retired in Phase B4.
        }

        commands.entity(entity).despawn();
    }
}

/// Apply a single `DescriptiveEffect` produced by a custom diplomatic
/// action's `on_accepted` callback. Mirrors the `apply_effect` helper used
/// by tech research — kept local to avoid a cross-module dependency on a
/// `pub fn` that's today private to `technology::effects`.
fn apply_custom_action_effect(
    effect: &crate::effect::DescriptiveEffect,
    game_flags: &mut crate::technology::GameFlags,
    scoped_flags: &mut crate::condition::ScopedFlags,
    global_params: &mut crate::technology::GlobalParams,
) {
    use crate::effect::DescriptiveEffect;
    match effect {
        DescriptiveEffect::PushModifier {
            target,
            base_add,
            multiplier,
            add,
            ..
        } => {
            apply_custom_action_modifier(
                global_params,
                target,
                *base_add,
                *multiplier,
                *add,
            );
        }
        DescriptiveEffect::PopModifier { .. } => {
            // Pop is meaningless for one-shot acceptance.
        }
        DescriptiveEffect::SetFlag { name, value, .. } => {
            if *value {
                game_flags.set(name);
                scoped_flags.set(name);
            }
        }
        DescriptiveEffect::FireEvent { event_id, .. } => {
            info!(
                "Custom diplomatic action requests event fire: {} (not yet wired)",
                event_id
            );
        }
        DescriptiveEffect::Hidden { inner, .. } => {
            apply_custom_action_effect(inner, game_flags, scoped_flags, global_params);
        }
    }
}

/// Map a subset of well-known modifier targets onto `GlobalParams`. This
/// duplicates the shape of `apply_modifier_to_params` in
/// `technology::effects` and is intentionally small — diplomacy-specific
/// targets (`empire.trade_income`, etc.) don't yet have a dedicated home in
/// Rust state and are silently accepted for later wiring.
fn apply_custom_action_modifier(
    params: &mut crate::technology::GlobalParams,
    target: &str,
    base_add: f64,
    multiplier: f64,
    add: f64,
) {
    match target {
        "ship.sublight_speed" => {
            params.sublight_speed_bonus += base_add + add;
        }
        "ship.ftl_speed" => {
            if multiplier != 0.0 {
                params.ftl_speed_multiplier += multiplier;
            }
            params.sublight_speed_bonus += base_add + add;
        }
        "ship.ftl_range" => {
            params.ftl_range_bonus += base_add + add;
        }
        "sensor.range" => {
            params.survey_range_bonus += base_add + add;
        }
        _ => {
            debug!(
                "Custom diplomatic action modifier target '{target}' not mapped to GlobalParams"
            );
        }
    }
}

/// Convenience: look up a [`FactionTypeRegistry`] entry for `faction` (via
/// its [`crate::scripting::faction_api::FactionDefinition`] id) and return
/// `true` iff the type is marked `can_diplomacy`. Used as a guard on the
/// public diplomatic helpers so callers don't accidentally try to negotiate
/// with passive factions (e.g. `space_creature`).
///
/// Returns `false` when the faction has no `Faction` component or when its
/// declared `faction_type` is not registered. Callers that want to skip the
/// guard (e.g. tests, internal lifecycle hooks) can call the per-action
/// helpers directly — this function is purely advisory.
pub fn faction_can_diplomacy(
    faction_entity: Entity,
    factions: &Query<&crate::player::Faction>,
    faction_registry: &crate::scripting::faction_api::FactionRegistry,
    type_registry: &crate::scripting::faction_api::FactionTypeRegistry,
) -> bool {
    let Ok(faction) = factions.get(faction_entity) else { return false; };
    let Some(def) = faction_registry.factions.get(&faction.id) else { return false; };
    let Some(type_id) = def.faction_type.as_ref() else { return false; };
    type_registry
        .get(type_id)
        .map(|t| t.can_diplomacy)
        .unwrap_or(false)
}

/// #295 (S-1) / #296 (S-3): Derive the sovereign owner of a star system from
/// the Core ship present in that system. Returns `Some(faction_entity)` when
/// a Core ship with a [`FactionOwner`] sits in `system`, `None` otherwise.
///
/// The sovereign owner of a system is defined by the Core ship stationed
/// there — removing the Core ship removes sovereignty. This replaces the
/// previous colony-presence-based hardcoded `player_empire` heuristic.
///
/// The query filters by `With<crate::ship::CoreShip>` so transient ships
/// (colony ships, couriers, cruisers) — even though they all carry
/// `FactionOwner` after #297 — never confer sovereignty. Only the dedicated
/// Infrastructure Core ship qualifies.
pub fn system_owner(
    system: Entity,
    at_system: &Query<
        (&crate::galaxy::AtSystem, &FactionOwner),
        With<crate::ship::CoreShip>,
    >,
) -> Option<Entity> {
    for (at, owner) in at_system.iter() {
        if at.0 == system {
            return Some(owner.0);
        }
    }
    None
}

/// #297 (S-2): Resolve the faction entity owning `entity`.
///
/// Consults, in order:
/// 1. A [`FactionOwner`] component (canonical — applies to colony, ship,
///    SystemBuildings-bearing StarSystem, DeepSpaceStructure, Hostile).
/// 2. [`crate::ship::Ship::owner`] = [`crate::ship::Owner::Empire`] if the
///    entity is a Ship (transitional until the `Owner` enum is removed in a
///    follow-up; see plan `docs/plan-297-faction-owner-unification.md` §2D).
///
/// Returns `None` for wholly unaffiliated entities (e.g.
/// [`crate::ship::Owner::Neutral`] ships with no `FactionOwner`, or entities
/// that never received one).
pub fn entity_owner(world: &World, entity: Entity) -> Option<Entity> {
    let e = world.get_entity(entity).ok()?;
    if let Some(fo) = e.get::<FactionOwner>() {
        return Some(fo.0);
    }
    if let Some(ship) = e.get::<crate::ship::Ship>() {
        if let crate::ship::Owner::Empire(f) = ship.owner {
            return Some(f);
        }
    }
    None
}

/// #297 (S-2): System-facing variant of [`entity_owner`] for hot paths inside
/// Bevy systems where a `&World` is unavailable. Uses the same precedence.
///
/// Callers must provide read-only `FactionOwner` and `Ship` queries. This
/// helper is query-coherent (both queries are `&` — no `&mut` overlap risk).
pub fn entity_owner_from_query(
    entity: Entity,
    faction_owners: &Query<&FactionOwner>,
    ships: &Query<&crate::ship::Ship>,
) -> Option<Entity> {
    if let Ok(fo) = faction_owners.get(entity) {
        return Some(fo.0);
    }
    if let Ok(ship) = ships.get(entity) {
        if let crate::ship::Owner::Empire(f) = ship.owner {
            return Some(f);
        }
    }
    None
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

    // ---- should_engage_defensive (#169) ----

    /// War always engages, regardless of `being_attacked`.
    #[test]
    fn should_engage_defensive_war_always_true() {
        let v = FactionView::new(RelationState::War, 0.0);
        assert!(v.should_engage_defensive(false));
        assert!(v.should_engage_defensive(true));
    }

    /// Non-war + not being attacked: never engage. Defensive does not start
    /// fights from negative standing alone.
    #[test]
    fn should_engage_defensive_idle_negative_standing_does_not_engage() {
        for state in [RelationState::Neutral, RelationState::Peace, RelationState::Alliance] {
            let v = FactionView::new(state, -100.0);
            assert!(
                !v.should_engage_defensive(false),
                "Defensive must not preemptively engage in {state:?} at standing=-100"
            );
        }
    }

    /// Non-war + being attacked: always retaliate, even against Peace/Alliance
    /// (stale-relation tolerance).
    #[test]
    fn should_engage_defensive_retaliates_when_attacked() {
        for state in [RelationState::Neutral, RelationState::Peace, RelationState::Alliance] {
            for &standing in &[-100.0, 0.0, 100.0] {
                let v = FactionView::new(state, standing);
                assert!(
                    v.should_engage_defensive(true),
                    "Defensive must retaliate in {state:?} (standing={standing}) when attacked"
                );
            }
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

    // ---- #171: light-speed delayed diplomacy ----

    use crate::time_system::GameClock;

    /// Build a minimal App that has the resources/systems needed by
    /// [`tick_diplomatic_actions`]. Returns the app plus two spawned
    /// faction entities (`from`, `to`).
    fn diplo_app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.insert_resource(GameClock::new(0));
        app.init_resource::<FactionRelations>();
        app.add_systems(Update, tick_diplomatic_actions);
        let from = app.world_mut().spawn_empty().id();
        let to = app.world_mut().spawn_empty().id();
        (app, from, to)
    }

    /// Step the clock forward by `n` hexadies and run one update cycle so
    /// `tick_diplomatic_actions` sees the new time.
    fn diplo_tick(app: &mut App, n: i64) {
        app.world_mut().resource_mut::<GameClock>().elapsed += n;
        app.update();
    }

    #[test]
    fn declare_war_with_delay_sender_immediate_receiver_delayed() {
        let (mut app, a, b) = diplo_app();

        // Run the helper inside a system so we have access to Commands.
        app.add_systems(
            Update,
            (move |mut c: Commands,
                   mut r: ResMut<FactionRelations>,
                   clk: Res<GameClock>| {
                declare_war_with_delay(&mut c, &mut r, &clk, a, b, 60);
            })
                .before(tick_diplomatic_actions),
        );
        app.update(); // T=0: helper runs, sender now at War, pending spawned

        // Sender side already at War.
        assert_eq!(
            app.world().resource::<FactionRelations>().get(a, b).unwrap().state,
            RelationState::War
        );
        // Receiver side still default (Neutral).
        assert!(app.world().resource::<FactionRelations>().get(b, a).is_none());

        // Drop the helper system so subsequent updates don't keep firing it.
        // (We rebuild a fresh app instead — schedule mutation isn't allowed.)
    }

    #[test]
    fn declare_war_receiver_flips_after_arrival() {
        let (mut app, a, b) = diplo_app();
        // Schedule the message manually (one-time) so we can advance time.
        app.world_mut().resource_mut::<FactionRelations>().declare_war(a, b);
        app.world_mut().spawn(PendingDiplomaticAction {
            from: a,
            to: b,
            action: DiplomaticAction::DeclareWar,
            arrives_at: 60,
            one_way_delay_hexadies: 60,
        });

        // Before arrival.
        diplo_tick(&mut app, 30);
        assert!(app.world().resource::<FactionRelations>().get(b, a).is_none());

        // At arrival — clock=60.
        diplo_tick(&mut app, 30);
        let view = app
            .world()
            .resource::<FactionRelations>()
            .get(b, a)
            .expect("receiver view set on arrival");
        assert_eq!(view.state, RelationState::War);
    }

    #[test]
    fn break_alliance_with_delay_propagates() {
        let (mut app, a, b) = diplo_app();
        // Pre-set Alliance on both sides.
        {
            let mut r = app.world_mut().resource_mut::<FactionRelations>();
            r.set(a, b, FactionView::new(RelationState::Alliance, 50.0));
            r.set(b, a, FactionView::new(RelationState::Alliance, 50.0));
        }
        // Schedule break manually.
        app.world_mut().resource_mut::<FactionRelations>().break_alliance(a, b);
        app.world_mut().spawn(PendingDiplomaticAction {
            from: a,
            to: b,
            action: DiplomaticAction::BreakAlliance,
            arrives_at: 60,
            one_way_delay_hexadies: 60,
        });

        // Sender already at Peace.
        assert_eq!(
            app.world().resource::<FactionRelations>().get(a, b).unwrap().state,
            RelationState::Peace
        );
        // Receiver still Alliance.
        assert_eq!(
            app.world().resource::<FactionRelations>().get(b, a).unwrap().state,
            RelationState::Alliance
        );

        diplo_tick(&mut app, 60);
        assert_eq!(
            app.world().resource::<FactionRelations>().get(b, a).unwrap().state,
            RelationState::Peace
        );
    }

    #[test]
    fn propose_peace_one_way_then_round_trip() {
        let (mut app, a, b) = diplo_app();
        // Both sides at War (we want to verify peace transitions).
        {
            let mut r = app.world_mut().resource_mut::<FactionRelations>();
            r.set(a, b, FactionView::new(RelationState::War, -80.0));
            r.set(b, a, FactionView::new(RelationState::War, -80.0));
        }

        // Spawn proposal manually with one_way_delay=60.
        app.world_mut().spawn(PendingDiplomaticAction {
            from: a,
            to: b,
            action: DiplomaticAction::ProposePeace,
            arrives_at: 60,
            one_way_delay_hexadies: 60,
        });

        // Before arrival both sides still War.
        diplo_tick(&mut app, 30);
        assert_eq!(
            app.world().resource::<FactionRelations>().get(b, a).unwrap().state,
            RelationState::War
        );
        assert_eq!(
            app.world().resource::<FactionRelations>().get(a, b).unwrap().state,
            RelationState::War
        );

        // At T=60: receiver flips to Peace; sender still War.
        diplo_tick(&mut app, 30);
        assert_eq!(
            app.world().resource::<FactionRelations>().get(b, a).unwrap().state,
            RelationState::Peace
        );
        assert_eq!(
            app.world().resource::<FactionRelations>().get(a, b).unwrap().state,
            RelationState::War
        );

        // Acceptance return leg arrives at T=120.
        diplo_tick(&mut app, 60);
        assert_eq!(
            app.world().resource::<FactionRelations>().get(a, b).unwrap().state,
            RelationState::Peace
        );
    }

    #[test]
    fn propose_alliance_round_trip() {
        let (mut app, a, b) = diplo_app();
        // Both sides at Peace.
        {
            let mut r = app.world_mut().resource_mut::<FactionRelations>();
            r.set(a, b, FactionView::new(RelationState::Peace, 0.0));
            r.set(b, a, FactionView::new(RelationState::Peace, 0.0));
        }
        app.world_mut().spawn(PendingDiplomaticAction {
            from: a,
            to: b,
            action: DiplomaticAction::ProposeAlliance,
            arrives_at: 30,
            one_way_delay_hexadies: 30,
        });

        diplo_tick(&mut app, 30);
        assert_eq!(
            app.world().resource::<FactionRelations>().get(b, a).unwrap().state,
            RelationState::Alliance
        );
        assert_eq!(
            app.world().resource::<FactionRelations>().get(a, b).unwrap().state,
            RelationState::Peace
        );

        diplo_tick(&mut app, 30);
        assert_eq!(
            app.world().resource::<FactionRelations>().get(a, b).unwrap().state,
            RelationState::Alliance
        );
    }

    #[test]
    fn pending_action_at_zero_delay_lands_immediately() {
        let (mut app, a, b) = diplo_app();
        // delay_hexadies=0 (e.g. cohabitating capitals): both sides should
        // be in sync after the next update.
        app.world_mut().resource_mut::<FactionRelations>().declare_war(a, b);
        app.world_mut().spawn(PendingDiplomaticAction {
            from: a,
            to: b,
            action: DiplomaticAction::DeclareWar,
            arrives_at: 0,
            one_way_delay_hexadies: 0,
        });
        app.update();

        assert_eq!(
            app.world().resource::<FactionRelations>().get(a, b).unwrap().state,
            RelationState::War
        );
        assert_eq!(
            app.world().resource::<FactionRelations>().get(b, a).unwrap().state,
            RelationState::War
        );
    }

    #[test]
    fn surprise_attack_window_receiver_still_neutral() {
        // Sender declares war and a fleet is en route. Until the war
        // declaration arrives at the receiver, the receiver's view of the
        // sender is still Neutral/Peace and `can_attack_aggressive` from
        // the receiver's side returns false (Defensive ROE would hold fire).
        let (mut app, sender, receiver) = diplo_app();
        // Receiver previously had Peace with sender (positive standing).
        app.world_mut()
            .resource_mut::<FactionRelations>()
            .set(receiver, sender, FactionView::new(RelationState::Peace, 20.0));

        app.world_mut().resource_mut::<FactionRelations>().declare_war(sender, receiver);
        app.world_mut().spawn(PendingDiplomaticAction {
            from: sender,
            to: receiver,
            action: DiplomaticAction::DeclareWar,
            arrives_at: 60,
            one_way_delay_hexadies: 60,
        });

        // Mid-flight: receiver still sees Peace.
        diplo_tick(&mut app, 30);
        let receiver_view = app
            .world()
            .resource::<FactionRelations>()
            .get(receiver, sender)
            .unwrap();
        assert_eq!(receiver_view.state, RelationState::Peace);
        assert!(
            !receiver_view.can_attack_aggressive(),
            "receiver under Peace + standing>0 must not retaliate"
        );

        // After arrival: receiver also at War.
        diplo_tick(&mut app, 30);
        let receiver_view = app
            .world()
            .resource::<FactionRelations>()
            .get(receiver, sender)
            .unwrap();
        assert_eq!(receiver_view.state, RelationState::War);
        assert!(receiver_view.can_attack_aggressive());
    }

    #[test]
    fn negative_delay_clamped_to_zero() {
        // Ensure the helper coerces negative input to a 0-delay rather than
        // scheduling a message into the past indefinitely.
        let (mut app, a, b) = diplo_app();
        app.world_mut().resource_mut::<GameClock>().elapsed = 100;

        app.add_systems(
            Update,
            (move |mut c: Commands,
                   mut r: ResMut<FactionRelations>,
                   clk: Res<GameClock>| {
                declare_war_with_delay(&mut c, &mut r, &clk, a, b, -10);
            })
                .before(tick_diplomatic_actions),
        );
        app.update();

        // Pending action should have arrived in the same frame.
        assert_eq!(
            app.world().resource::<FactionRelations>().get(b, a).unwrap().state,
            RelationState::War
        );
    }

    #[test]
    fn faction_can_diplomacy_returns_false_for_unregistered_faction() {
        use crate::scripting::faction_api::{FactionRegistry, FactionTypeRegistry};

        let mut app = App::new();
        app.init_resource::<FactionRegistry>();
        app.init_resource::<FactionTypeRegistry>();
        let f = app.world_mut().spawn(crate::player::Faction {
            id: "unknown".into(),
            name: "Unknown".into(),
        }).id();

        // Run inside a system to get Query access.
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let result_w = result.clone();
        app.add_systems(
            Update,
            move |q: Query<&crate::player::Faction>,
                  fr: Res<FactionRegistry>,
                  tr: Res<FactionTypeRegistry>| {
                let v = faction_can_diplomacy(f, &q, &fr, &tr);
                *result_w.lock().unwrap() = Some(v);
            },
        );
        app.update();
        assert_eq!(*result.lock().unwrap(), Some(false));
    }

    #[test]
    fn faction_can_diplomacy_true_when_type_allows() {
        use crate::scripting::faction_api::{
            FactionDefinition, FactionRegistry, FactionTypeDefinition, FactionTypeRegistry,
        };

        let mut app = App::new();
        let mut freg = FactionRegistry::default();
        freg.factions.insert(
            "empire_x".into(),
            FactionDefinition {
                id: "empire_x".into(),
                name: "Empire X".into(),
                faction_type: Some("empire".into()),
                has_on_game_start: false,
            },
        );
        let mut treg = FactionTypeRegistry::default();
        treg.types.insert(
            "empire".into(),
            FactionTypeDefinition {
                id: "empire".into(),
                can_diplomacy: true,
                default_standing: 0.0,
                default_state: RelationState::Neutral,
                strength: 0.0,
                evasion: 0.0,
                default_hp: 0.0,
                default_max_hp: 0.0,
            },
        );
        app.insert_resource(freg);
        app.insert_resource(treg);

        let f = app.world_mut().spawn(crate::player::Faction {
            id: "empire_x".into(),
            name: "Empire X".into(),
        }).id();

        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let result_w = result.clone();
        app.add_systems(
            Update,
            move |q: Query<&crate::player::Faction>,
                  fr: Res<FactionRegistry>,
                  tr: Res<FactionTypeRegistry>| {
                let v = faction_can_diplomacy(f, &q, &fr, &tr);
                *result_w.lock().unwrap() = Some(v);
            },
        );
        app.update();
        assert_eq!(*result.lock().unwrap(), Some(true));
    }

    // ---- #297 (S-2): entity_owner helper ----

    /// Build a minimal `Ship` for owner-resolution tests. Most fields are
    /// filler — only `owner` is meaningful for the helper under test.
    fn make_ship(owner: crate::ship::Owner, home_port: Entity) -> crate::ship::Ship {
        crate::ship::Ship {
            name: "test".into(),
            design_id: "scout".into(),
            hull_id: "corvette".into(),
            modules: Vec::new(),
            owner,
            sublight_speed: 0.5,
            ftl_range: 0.0,
            player_aboard: false,
            home_port,
            design_revision: 0,
            fleet: None,
        }
    }

    #[test]
    fn entity_owner_returns_none_for_bare_entity() {
        let mut world = World::new();
        let e = world.spawn_empty().id();
        assert_eq!(entity_owner(&world, e), None);
    }

    #[test]
    fn entity_owner_resolves_faction_owner_component() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let colony_like = world.spawn(FactionOwner(empire)).id();
        assert_eq!(entity_owner(&world, colony_like), Some(empire));
    }

    #[test]
    fn entity_owner_resolves_ship_owner_empire_only() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let system = world.spawn_empty().id();
        let ship = world
            .spawn(make_ship(crate::ship::Owner::Empire(empire), system))
            .id();
        // No FactionOwner component — falls through to Ship.owner.
        assert_eq!(entity_owner(&world, ship), Some(empire));
    }

    #[test]
    fn entity_owner_prefers_faction_owner_over_ship_owner() {
        let mut world = World::new();
        let empire_a = world.spawn_empty().id();
        let empire_b = world.spawn_empty().id();
        let system = world.spawn_empty().id();
        // Ship has both — pathological but tests precedence.
        let ship = world
            .spawn((
                make_ship(crate::ship::Owner::Empire(empire_a), system),
                FactionOwner(empire_b),
            ))
            .id();
        assert_eq!(entity_owner(&world, ship), Some(empire_b));
    }

    #[test]
    fn entity_owner_returns_none_for_neutral_ship_without_component() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = world
            .spawn(make_ship(crate::ship::Owner::Neutral, system))
            .id();
        assert_eq!(entity_owner(&world, ship), None);
    }
}
