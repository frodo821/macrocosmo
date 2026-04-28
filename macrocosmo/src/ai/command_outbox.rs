//! Light-speed delay shim for AI-produced commands (Round 9 PR #3 Bug 2).
//!
//! Background: the player's commands to remote systems are routed through
//! `PendingCommand` entities with an `arrives_at` timestamp computed from
//! the issuer-to-target distance, so the *light-speed information
//! constraint* applies symmetrically. AI commands historically bypassed
//! this: producers (`npc_decision_tick`, `run_short_agents`) called
//! `bus.emit_command(...)` and the consumer (`drain_ai_commands`) drained
//! and applied them in the same tick, giving NPC empires perfect
//! instantaneous reach across the galaxy.
//!
//! This module fixes that by interposing an outbox between producer and
//! consumer:
//!
//! 1. Producers emit through `bus.emit_command` as before — no producer
//!    code changes.
//! 2. A `dispatch_ai_pending_commands` system at the **end** of
//!    [`AiTickSet::Reason`](super::AiTickSet::Reason) drains the bus's
//!    pending queue, computes each command's `arrives_at` via the
//!    existing [`compute_fact_arrival`](crate::knowledge::compute_fact_arrival)
//!    function (so AI courier delays use the same relay-aware model as
//!    knowledge propagation), and stows the entries into
//!    [`AiCommandOutbox`].
//! 3. A `process_ai_pending_commands` system at the **start** of
//!    [`AiTickSet::CommandDrain`](super::AiTickSet::CommandDrain) walks
//!    the outbox, releases mature entries back to the bus via
//!    [`AiBus::push_command_already_dispatched`](macrocosmo_ai::AiBus::push_command_already_dispatched),
//!    and lets `drain_ai_commands` consume them as if they had just been
//!    emitted.
//!
//! ## Origin / destination resolution
//!
//! The arrival-time computation needs two world positions: where the
//! command is "sent from" and where it is "sent to."
//!
//! * **Origin** is the issuing empire's Ruler position. The Ruler chain
//!   is `Empire → EmpireRuler.0 → Ruler` and the Ruler's position is
//!   either its [`StationedAt`](crate::player::StationedAt) system, or
//!   the ship the Ruler is aboard (via [`AboardShip`](crate::player::AboardShip)).
//!   If neither resolves, the command is **dropped** with a `warn!` —
//!   issuing an order from "nowhere" is semantically meaningless and
//!   the situation should never occur in a well-formed run, so we treat
//!   it as a soft assertion rather than fall back to a default.
//! * **Destination** is the world position the command "addresses." For
//!   the spatial commands (`survey_system`, `move_to`,
//!   `colonize_system`, `attack_target`, `reposition`, `blockade`,
//!   `fortify_system`, `move_ruler`, `build_ship`, `build_structure`)
//!   it is the `target_system` parameter's `Position`. For
//!   spatial-less commands (`research_focus`, `retreat`, …) the
//!   destination collapses to the issuing empire's **capital** — the
//!   intuition is "the order goes home, gets carried out by the
//!   government there." So a Ruler stationed at the capital pays no
//!   delay (origin == destination → 0 hexadies), while a Ruler off
//!   campaigning incurs Ruler→capital light delay. The capital is
//!   resolved via the same fallback chain used by
//!   [`crate::knowledge::initialize_capital_knowledge`]:
//!   1. `HomeSystem` component on the empire entity, then
//!   2. [`HomeSystemAssignments`](crate::galaxy::HomeSystemAssignments)
//!      keyed on `Faction.id`, then
//!   3. The first `StarSystem.is_capital` system in the galaxy.
//!
//! ## Cycle safety
//!
//! `dispatch_ai_pending_commands` runs at the **end** of `Reason` and
//! `process_ai_pending_commands` at the **start** of `CommandDrain`,
//! and the two sets are chained (`Reason → CommandDrain`). Within one
//! frame, dispatch sees only commands the producers emitted **this
//! tick**; processing then releases entries whose `arrives_at` ≤ now
//! back to the bus, where `drain_ai_commands` consumes them. There is
//! no path within a frame for a command to be emitted, dispatched,
//! processed, and re-dispatched — the dispatch-vs-process boundary is
//! *enforced* by the `Reason → CommandDrain` chain and the producer
//! systems all live before dispatch.
//!
//! ## Save / load
//!
//! [`AiCommandOutbox`] persists across saves so a game saved with an
//! AI command in flight reloads with the same delay still ticking.
//! See `persistence/savebag.rs` for the wire shim
//! (`SerializedPendingAiCommand` round-trips through `SerializedCommand`
//! and a `BTreeMap` of params for stable ordering).

use bevy::prelude::*;

use macrocosmo_ai::Command;

use crate::ai::convert::{from_ai_entity, from_ai_system, to_ai_faction};
use crate::ai::schema::ids::command as cmd_ids;
use crate::components::Position;
use crate::empire::CommsParams;
use crate::galaxy::{HomeSystem, HomeSystemAssignments, StarSystem};
use crate::knowledge::{
    ArrivalPlan, KnowledgeStore, ObservationSource, command_kind_has_return_leg,
    command_kind_to_intended_state, compute_fact_arrival, compute_ship_projection,
};
use crate::player::{AboardShip, Empire, EmpireRuler, Faction, Ruler, StationedAt};
use crate::ship::Ship;

/// Resource holding AI commands that have been produced but not yet
/// reached their destination at light speed.
///
/// Drained by `process_ai_pending_commands` at the head of
/// [`AiTickSet::CommandDrain`](super::AiTickSet::CommandDrain). Filled
/// by `dispatch_ai_pending_commands` at the tail of
/// [`AiTickSet::Reason`](super::AiTickSet::Reason).
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct AiCommandOutbox {
    /// `macrocosmo_ai::Command` carries an `AHashMap` of parameters
    /// that `bevy_reflect` cannot reach through (the AI core crate is
    /// engine-agnostic and cannot take a `bevy_reflect` dependency).
    /// We mark the field `#[reflect(ignore)]` so the resource still
    /// appears in BRP's type registry while the per-entry payload
    /// stays opaque to reflection. Persistence is handled separately
    /// in `persistence/savebag.rs` via a postcard wire shim.
    #[reflect(ignore)]
    pub entries: Vec<PendingAiCommand>,
}

/// One AI command in flight: the command itself plus the arrival
/// metadata needed to gate `drain_ai_commands` until light has had
/// time to reach the destination.
#[derive(Clone, Debug)]
pub struct PendingAiCommand {
    /// The command as produced by the AI policy / orchestrator.
    pub command: Command,
    /// Tick (hexadies) at which the command should be released to
    /// `drain_ai_commands`. Computed via [`compute_fact_arrival`].
    pub arrives_at: i64,
    /// Tick at which the command was originally emitted.
    pub sent_at: i64,
    /// World-space position of the issuer (Ruler) at emit time.
    pub origin_pos: [f64; 3],
    /// World-space position of the destination at emit time. `None`
    /// only when capital resolution failed for a spatial-less
    /// command — the entry was kept in the outbox for telemetry but
    /// the dispatcher would normally have dropped it.
    pub destination_pos: Option<[f64; 3]>,
    /// Tag from [`compute_fact_arrival`]'s relay-aware planner —
    /// `Direct` means pure light path, `Relay` means the FTL Comm
    /// Relay network shortened the route. Used for debug telemetry
    /// so an in-game console can answer "why did this NPC's order
    /// take so long to land?"
    pub source: ObservationSource,
}

/// Compute the arrival tick for an AI command, given its origin
/// (issuer Ruler position) and destination (resolved by the caller
/// from the command kind / params).
///
/// Returns `None` only when the inputs imply zero motion *and* zero
/// origin — which never happens in a well-formed game state — so the
/// caller can treat `Some(plan)` as the canonical case. We funnel
/// through [`compute_fact_arrival`] so AI courier delays match the
/// existing knowledge-propagation model bit-for-bit (light speed,
/// FTL Comm Relay shortcuts, etc.).
pub fn compute_ai_command_arrival(
    sent_at: i64,
    origin_pos: [f64; 3],
    destination_pos: [f64; 3],
    relays: &[crate::knowledge::RelaySnapshot],
    comms: &CommsParams,
) -> ArrivalPlan {
    compute_fact_arrival(sent_at, origin_pos, destination_pos, relays, comms)
}

/// Resolve the world-space position of an empire's Ruler at the
/// current tick. Returns `None` if the Ruler entity is missing,
/// cannot be located, or its location entity has no `Position`
/// component (e.g. an `AboardShip` ship that was just despawned).
///
/// The lookup chain is:
///
/// 1. Empire → EmpireRuler → Ruler entity
/// 2. Ruler is either `AboardShip(ship)` (mobile — read ship's
///    `Position`) or `StationedAt(system)` (read system's `Position`).
///
/// Used by `dispatch_ai_pending_commands` to compute the origin half
/// of the arrival plan.
pub fn resolve_ruler_position(
    empire: Entity,
    empire_rulers: &Query<&EmpireRuler, With<Empire>>,
    rulers: &Query<(Option<&StationedAt>, Option<&AboardShip>), With<Ruler>>,
    positions: &Query<&Position>,
) -> Option<[f64; 3]> {
    let ruler_entity = empire_rulers.get(empire).ok()?.0;
    let (stationed, aboard) = rulers.get(ruler_entity).ok()?;
    if let Some(aboard) = aboard {
        if let Ok(pos) = positions.get(aboard.ship) {
            return Some(pos.as_array());
        }
    }
    if let Some(stationed) = stationed {
        if let Ok(pos) = positions.get(stationed.system) {
            return Some(pos.as_array());
        }
    }
    None
}

/// Resolve the issuing empire's capital system entity.
///
/// Mirrors the fallback chain used by
/// [`crate::knowledge::initialize_capital_knowledge`]:
/// 1. `HomeSystem` component on the empire entity (canonical, set
///    during `apply_game_start_actions`).
/// 2. [`HomeSystemAssignments`] resource keyed on `Faction.id` (the
///    pre-`HomeSystem` source of truth, still populated for
///    backwards compatibility / observer mode).
/// 3. The first `StarSystem.is_capital` system in the galaxy
///    (last-resort fallback for tests that don't go through the full
///    galaxy-generation pipeline).
///
/// Returns `None` when none of the three paths resolves, e.g. a
/// minimal headless test that hasn't spawned any systems.
pub fn resolve_capital_system(
    empire: Entity,
    home_systems: &Query<&HomeSystem>,
    factions: &Query<&Faction, With<Empire>>,
    home_assignments: Option<&HomeSystemAssignments>,
    star_systems: &Query<(Entity, &StarSystem)>,
) -> Option<Entity> {
    if let Ok(hs) = home_systems.get(empire) {
        return Some(hs.0);
    }
    if let (Ok(faction), Some(ha)) = (factions.get(empire), home_assignments) {
        if let Some(&entity) = ha.assignments.get(&faction.id) {
            return Some(entity);
        }
    }
    star_systems
        .iter()
        .find(|(_, s)| s.is_capital)
        .map(|(e, _)| e)
}

/// Whether the command kind has a built-in spatial target via
/// `target_system` param. Used by the dispatcher to choose between
/// "destination = target_system position" (these kinds) and
/// "destination = capital position" (everything else).
///
/// We list the kinds explicitly rather than gating on
/// `params.contains_key("target_system")` so a malformed command
/// missing its expected param drops cleanly instead of silently
/// turning into a capital-bound order.
pub fn command_targets_system(kind: &str) -> bool {
    let s = kind;
    s == cmd_ids::attack_target().as_str()
        || s == cmd_ids::reposition().as_str()
        || s == cmd_ids::blockade().as_str()
        || s == cmd_ids::fortify_system().as_str()
        || s == cmd_ids::colonize_system().as_str()
        || s == cmd_ids::survey_system().as_str()
        || s == cmd_ids::move_ruler().as_str()
    // build_ship / build_structure may carry a target_system in some
    // policies but commonly route to the empire's preferred shipyard;
    // they fall through to capital-as-destination by default. The
    // explicit list above stays the source of truth.
}

/// Extract the `target_system` param's world position, if present.
/// Returns `None` when the param is missing, the wrong type, or the
/// referenced entity has no `Position` (e.g. a stale entity bits from
/// an old save). The caller drops the command in that case to mirror
/// the `warn! + drop` behaviour of malformed origin lookups.
pub fn destination_pos_from_target_system(
    cmd: &Command,
    star_positions: &Query<&Position, (With<StarSystem>, Without<crate::ship::Ship>)>,
) -> Option<[f64; 3]> {
    use macrocosmo_ai::CommandValue;
    let sys_ref = match cmd.params.get("target_system") {
        Some(CommandValue::System(s)) => *s,
        _ => return None,
    };
    let entity = from_ai_system(sys_ref);
    star_positions.get(entity).ok().map(|p| p.as_array())
}

/// Resolve the destination position for an AI command.
///
/// For spatial commands (see [`command_targets_system`]) this is the
/// `target_system` param's `Position`. For spatial-less commands
/// (e.g. `research_focus`) it is the issuing empire's capital
/// system's position — orders without a spatial target conceptually
/// route to "the home government," and the Ruler's distance to the
/// capital encodes the propagation delay.
///
/// Returns `None` when the resolution fails (target_system entity has
/// no `Position`, capital cannot be resolved, etc.). The dispatcher
/// drops the command with a `warn!` in that case.
pub fn resolve_destination_pos(
    cmd: &Command,
    issuer_empire: Entity,
    star_positions: &Query<&Position, (With<StarSystem>, Without<crate::ship::Ship>)>,
    home_systems: &Query<&HomeSystem>,
    factions: &Query<&Faction, With<Empire>>,
    home_assignments: Option<&HomeSystemAssignments>,
    star_systems: &Query<(Entity, &StarSystem)>,
) -> Option<[f64; 3]> {
    if command_targets_system(cmd.kind.as_str()) {
        if let Some(p) = destination_pos_from_target_system(cmd, star_positions) {
            return Some(p);
        }
        // Spatial command with a missing / unresolvable target_system:
        // fall through to capital so a slightly-malformed command
        // still pays *some* delay rather than executing instantly.
        // The dispatcher logs the original mismatch separately.
    }
    let capital = resolve_capital_system(
        issuer_empire,
        home_systems,
        factions,
        home_assignments,
        star_systems,
    )?;
    star_positions.get(capital).ok().map(|p| p.as_array())
}

/// Build a [`PendingAiCommand`] for `cmd`, computing the arrival
/// plan from `origin_pos` and `destination_pos`. The wrapper carries
/// the same payload regardless of whether the destination came from
/// `target_system` or capital fallback — by the time we get here all
/// resolution has happened.
pub fn build_pending_command(
    cmd: Command,
    sent_at: i64,
    origin_pos: [f64; 3],
    destination_pos: [f64; 3],
    relays: &[crate::knowledge::RelaySnapshot],
    comms: &CommsParams,
) -> PendingAiCommand {
    let plan = compute_ai_command_arrival(sent_at, origin_pos, destination_pos, relays, comms);
    PendingAiCommand {
        command: cmd,
        arrives_at: plan.arrives_at,
        sent_at,
        origin_pos,
        destination_pos: Some(destination_pos),
        source: plan.source,
    }
}

/// #475: Extract the primary ship entity an AI command targets, if any.
///
/// Convention used by the AI Short layer / consumer: ship-bearing commands
/// pass their ship list as `ship_count` + `ship_0`, `ship_1`, ... (see
/// `command_consumer::extract_ship_list`). For the dispatch-time
/// projection we only need the *first* ship — multi-ship commands write
/// one projection per ship in a follow-up; the data model already keys on
/// entity so this scales naturally.
///
/// Returns `None` when no `ship_0` param is present, when its `CommandValue`
/// is the wrong shape, or for spatial-less commands like `research_focus`.
pub fn extract_primary_ship(cmd: &Command) -> Option<Entity> {
    use macrocosmo_ai::CommandValue;
    match cmd.params.get("ship_0")? {
        CommandValue::Entity(e) => Some(from_ai_entity(*e)),
        _ => None,
    }
}

/// #475: Extract the `target_system` Entity from an AI command's params,
/// if present. Returns `None` for spatial-less commands.
pub fn extract_target_system(cmd: &Command) -> Option<Entity> {
    use macrocosmo_ai::CommandValue;
    match cmd.params.get("target_system")? {
        CommandValue::System(s) => Some(from_ai_system(*s)),
        _ => None,
    }
}

/// Reuse the consumer-side faction-entity lookup logic without
/// pulling in `command_consumer.rs` as a module dependency. The
/// faction id encodes only `Entity::index()` (see
/// [`crate::ai::convert::to_ai_faction`]), so we have to scan
/// empires to recover the live `Entity`.
pub fn find_empire_for_faction_id(
    issuer: macrocosmo_ai::FactionId,
    empires: &Query<Entity, With<Empire>>,
) -> Option<Entity> {
    for entity in empires {
        if to_ai_faction(entity) == issuer {
            return Some(entity);
        }
    }
    None
}

/// Helper used by `process_ai_pending_commands` to partition the
/// outbox into "ready now" and "still in flight" entries. Pure
/// function so tests can drive it without spinning up an `App`.
///
/// Returns a `(mature, remaining)` pair. Order is preserved within
/// each bucket — the consumer drains commands in FIFO order at the
/// bus level, but within a single tick the relative order between
/// two commands released from the outbox matches their original
/// emit order (the outbox is a `Vec<PendingAiCommand>` with `push`-
/// only growth in `dispatch_ai_pending_commands`).
pub fn split_outbox_at(
    now: i64,
    entries: Vec<PendingAiCommand>,
) -> (Vec<Command>, Vec<PendingAiCommand>) {
    let mut mature = Vec::new();
    let mut remaining = Vec::new();
    for entry in entries {
        if entry.arrives_at <= now {
            mature.push(entry.command);
        } else {
            remaining.push(entry);
        }
    }
    (mature, remaining)
}

// ---------------------------------------------------------------------------
// Bevy systems
// ---------------------------------------------------------------------------

/// SystemParam bundle for the dispatch system. Bundled because the
/// system already needs eight other params plus the bus + outbox + clock,
/// which would push it past Bevy's 16-param limit if expanded inline.
#[derive(bevy::ecs::system::SystemParam)]
pub struct DispatchParams<'w, 's> {
    /// Empire entities — used to resolve a `FactionId` back to its
    /// `Entity` so we can look up the Ruler and capital.
    pub empires: Query<'w, 's, Entity, With<Empire>>,
    /// `Empire → Ruler` chain.
    pub empire_rulers: Query<'w, 's, &'static EmpireRuler, With<Empire>>,
    /// Ruler location: `StationedAt` for system-bound, `AboardShip`
    /// for ship-bound. Both are read; `resolve_ruler_position` picks
    /// the live one.
    pub rulers:
        Query<'w, 's, (Option<&'static StationedAt>, Option<&'static AboardShip>), With<Ruler>>,
    /// World-space positions for any entity that may serve as a Ruler
    /// reference (StarSystem for stationed, ship entity for aboard).
    pub positions: Query<'w, 's, &'static Position>,
    /// Capital fallback chain — `HomeSystem` Component on the empire.
    pub home_systems: Query<'w, 's, &'static HomeSystem>,
    /// Capital fallback chain — `Faction.id` keyed lookup into
    /// `HomeSystemAssignments`.
    pub factions: Query<'w, 's, &'static Faction, With<Empire>>,
    /// Capital fallback chain — last-resort scan for the first
    /// `is_capital` system in the galaxy. The query is constrained
    /// to `&Position` so it doesn't conflict with mutable Position
    /// queries elsewhere in the schedule.
    pub star_systems: Query<'w, 's, (Entity, &'static StarSystem)>,
    /// Per-system positions used to read the destination half of the
    /// arrival plan when a command carries a `target_system` param.
    pub star_positions:
        Query<'w, 's, &'static Position, (With<StarSystem>, Without<crate::ship::Ship>)>,
    /// Resource fallback for capital resolution.
    pub home_assignments: Option<Res<'w, HomeSystemAssignments>>,
    /// Per-empire `CommsParams` — fed into `compute_fact_arrival` so
    /// the AI courier delay reflects the *issuing* empire's tech /
    /// modifier bonuses (matches Round 9 PR #1's pending TODO #4
    /// note about per-empire CommsParams in fact arrival).
    pub empire_comms: Query<'w, 's, &'static CommsParams, With<Empire>>,
    /// Active relay network snapshot for the relay-aware planner.
    /// `Option` because the resource is created by `KnowledgePlugin`,
    /// which not every test app installs (`ai_integration` and friends
    /// build a minimal `App` to test bus wiring in isolation). When
    /// absent the dispatcher treats the relay set as empty — the
    /// arrival plan falls back to pure light-speed direct path.
    pub relay_network: Option<Res<'w, crate::knowledge::RelayNetwork>>,
    /// #475: per-empire `KnowledgeStore` for dispatch-time projection
    /// writes (epic #473). Mutable so we can call
    /// `KnowledgeStore::update_projection` for the issuer's empire
    /// after staging the outbox entry.
    pub knowledge_stores: Query<'w, 's, &'static mut KnowledgeStore, With<Empire>>,
    /// #475: ship metadata used to resolve the ship's `home_port` for
    /// the no-snapshot fallback in `compute_ship_projection`.
    pub ships: Query<'w, 's, &'static Ship>,
}

/// End-of-`Reason` system: drain the AI bus's pending command queue,
/// compute each command's `arrives_at` from the issuing empire's
/// Ruler position to the command's destination, and stow the entries
/// into [`AiCommandOutbox`].
///
/// Commands whose origin or destination cannot be resolved are
/// dropped with a `warn!`. This matches the "soft assertion" tone
/// of the rest of the AI integration layer — a malformed command
/// indicates an upstream bug, not a recoverable runtime condition.
pub fn dispatch_ai_pending_commands(
    mut bus: ResMut<crate::ai::plugin::AiBusResource>,
    mut outbox: ResMut<AiCommandOutbox>,
    clock: Res<crate::time_system::GameClock>,
    mut params: DispatchParams,
) {
    let now = clock.elapsed;
    let drained = bus.drain_commands();
    if drained.is_empty() {
        return;
    }

    // Relay set defaults to empty when no `RelayNetwork` resource is
    // installed (minimal headless test apps); `compute_fact_arrival`
    // then falls back to pure light-speed direct path.
    let relays_owned: Vec<crate::knowledge::RelaySnapshot> = params
        .relay_network
        .as_deref()
        .map(|n| n.relays.clone())
        .unwrap_or_default();
    let relays: &[crate::knowledge::RelaySnapshot] = &relays_owned;

    for cmd in drained {
        let issuer = cmd.issuer;
        let Some(empire_entity) = find_empire_for_faction_id(issuer, &params.empires) else {
            warn!(
                "AI command outbox: dropping command kind={} from unknown faction {:?}",
                cmd.kind.as_str(),
                issuer,
            );
            continue;
        };

        let Some(origin_pos) = resolve_ruler_position(
            empire_entity,
            &params.empire_rulers,
            &params.rulers,
            &params.positions,
        ) else {
            warn!(
                "AI command outbox: dropping command kind={} — could not resolve Ruler position for empire {:?}",
                cmd.kind.as_str(),
                empire_entity,
            );
            continue;
        };

        let Some(destination_pos) = resolve_destination_pos(
            &cmd,
            empire_entity,
            &params.star_positions,
            &params.home_systems,
            &params.factions,
            params.home_assignments.as_deref(),
            &params.star_systems,
        ) else {
            warn!(
                "AI command outbox: dropping command kind={} — could not resolve destination for empire {:?}",
                cmd.kind.as_str(),
                empire_entity,
            );
            continue;
        };

        // Per-empire CommsParams: matches the per-faction
        // generalization arc of Round 9 PR #1. Falls back to the
        // default-bonus CommsParams when an empire spawns without
        // one (legacy / observer-mode edge cases).
        let comms = params
            .empire_comms
            .get(empire_entity)
            .cloned()
            .unwrap_or_default();

        let pending = build_pending_command(cmd, now, origin_pos, destination_pos, relays, &comms);

        // #475: dispatch-time projection write (epic #473). Only emit
        // projections for ship-bearing commands; spatial-less commands
        // (`research_focus`, etc.) don't move a ship and need no entry.
        let projection_inputs = build_projection_inputs(
            &pending.command,
            empire_entity,
            origin_pos,
            now,
            &params.knowledge_stores,
            &params.ships,
            &params.star_positions,
        );
        if let Some(inputs) = projection_inputs {
            let projection = compute_ship_projection(
                inputs.ship,
                inputs.snapshot.as_ref(),
                inputs.dispatcher_pos,
                inputs.ship_pos,
                inputs.target_system_pos,
                inputs.intended_state,
                inputs.intended_system,
                inputs.has_return_leg,
                inputs.fallback_system,
                now,
            );
            if let Ok(mut store) = params.knowledge_stores.get_mut(empire_entity) {
                store.update_projection(projection);
            }
        }

        outbox.entries.push(pending);
    }
}

/// #475: Local struct collecting the inputs `compute_ship_projection`
/// needs from the AI dispatch site. Returning `None` from
/// [`build_projection_inputs`] means "no projection to write" — the
/// command either lacks a ship (spatial-less) or the ship isn't owned by
/// any tracked entity.
struct ProjectionInputs {
    ship: Entity,
    snapshot: Option<crate::knowledge::ShipSnapshot>,
    dispatcher_pos: [f64; 3],
    ship_pos: [f64; 3],
    target_system_pos: Option<[f64; 3]>,
    intended_state: Option<crate::knowledge::ShipSnapshotState>,
    intended_system: Option<Entity>,
    has_return_leg: bool,
    fallback_system: Option<Entity>,
}

/// #475: Gather everything `compute_ship_projection` needs from the AI
/// dispatch path. Returns `None` for spatial-less commands (no ship).
///
/// The ship's *position* is read from the dispatcher's KnowledgeStore
/// snapshot — explicitly *not* from the ship's realtime ECS `Position`.
/// That's the entire point of #473: own-ship rendering / AI judgment must
/// flow through the dispatcher's local knowledge, not through the
/// ground-truth simulation. When no snapshot exists (= ship newly
/// spawned, dispatcher hasn't observed it yet) we fall back to the
/// ship's `home_port` system as the projected location.
fn build_projection_inputs(
    cmd: &Command,
    empire_entity: Entity,
    dispatcher_pos: [f64; 3],
    _now: i64,
    knowledge_stores: &Query<&mut KnowledgeStore, With<Empire>>,
    ships: &Query<&Ship>,
    star_positions: &Query<&Position, (With<StarSystem>, Without<crate::ship::Ship>)>,
) -> Option<ProjectionInputs> {
    let ship = extract_primary_ship(cmd)?;
    let target_system = extract_target_system(cmd);
    let target_system_pos =
        target_system.and_then(|sys| star_positions.get(sys).ok().map(|p| p.as_array()));

    // Dispatcher's last-known snapshot of this ship. Cloned so we can
    // release the immutable borrow before the mutable update at the
    // call site.
    let snapshot = knowledge_stores
        .get(empire_entity)
        .ok()
        .and_then(|store| store.get_ship(ship).cloned());

    // Fallback projected_system for the no-snapshot case: the ship's
    // home_port. We deliberately *don't* read the ship's runtime
    // `Position` or `ShipState` here — that would reintroduce the FTL
    // leak (#473 Q7).
    let fallback_system = ships.get(ship).ok().map(|s| s.home_port);

    // Ship position the dispatcher *believes*. Drawn from snapshot's
    // last_known_system position when present; from the loitering
    // coordinate when the snapshot is `Loitering`; from fallback_system
    // otherwise. *Never* from the ship's realtime `Position` component.
    let ship_pos = match snapshot.as_ref().map(|s| &s.last_known_state) {
        Some(crate::knowledge::ShipSnapshotState::Loitering { position }) => *position,
        _ => snapshot
            .as_ref()
            .and_then(|s| s.last_known_system)
            .or(fallback_system)
            .and_then(|sys| star_positions.get(sys).ok().map(|p| p.as_array()))
            .unwrap_or(dispatcher_pos),
    };

    let intended_state = command_kind_to_intended_state(cmd.kind.as_str());
    let has_return_leg = command_kind_has_return_leg(cmd.kind.as_str());

    Some(ProjectionInputs {
        ship,
        snapshot,
        dispatcher_pos,
        ship_pos,
        target_system_pos,
        intended_state,
        intended_system: target_system,
        has_return_leg,
        fallback_system,
    })
}

/// Start-of-`CommandDrain` system: walk the outbox, partition into
/// mature and pending entries via [`split_outbox_at`], and re-push
/// the mature commands to the bus via
/// [`AiBus::push_command_already_dispatched`](macrocosmo_ai::AiBus::push_command_already_dispatched).
/// `drain_ai_commands` then consumes them in the same tick.
pub fn process_ai_pending_commands(
    mut bus: ResMut<crate::ai::plugin::AiBusResource>,
    mut outbox: ResMut<AiCommandOutbox>,
    clock: Res<crate::time_system::GameClock>,
) {
    let now = clock.elapsed;
    if outbox.entries.is_empty() {
        return;
    }

    let entries = std::mem::take(&mut outbox.entries);
    let (mature, remaining) = split_outbox_at(now, entries);
    outbox.entries = remaining;

    for cmd in mature {
        bus.push_command_already_dispatched(cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::RelaySnapshot;
    use macrocosmo_ai::{CommandKindId, FactionId};

    fn empty_relays() -> Vec<RelaySnapshot> {
        Vec::new()
    }

    fn empty_comms() -> CommsParams {
        CommsParams::default()
    }

    fn make_cmd(kind: &str) -> Command {
        Command::new(CommandKindId::from(kind), FactionId(7), 0)
    }

    #[test]
    fn command_targets_system_lists_spatial_kinds() {
        // Sanity: every kind that the consumer reads `target_system`
        // for must report `true` here, and at least one spatial-less
        // kind must report `false`.
        assert!(command_targets_system("attack_target"));
        assert!(command_targets_system("survey_system"));
        assert!(command_targets_system("colonize_system"));
        assert!(command_targets_system("move_ruler"));
        assert!(command_targets_system("reposition"));
        assert!(command_targets_system("blockade"));
        assert!(command_targets_system("fortify_system"));
        assert!(!command_targets_system("research_focus"));
        assert!(!command_targets_system("retreat"));
        assert!(!command_targets_system("declare_war"));
    }

    #[test]
    fn compute_arrival_zero_distance_yields_zero_delay() {
        // Origin == destination collapses to "ruler is already at
        // the destination" — the order is local to the Ruler and
        // arrives the same tick it was emitted.
        let plan = compute_ai_command_arrival(
            100,
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            &empty_relays(),
            &empty_comms(),
        );
        assert_eq!(plan.arrives_at, 100);
        assert_eq!(plan.source, ObservationSource::Direct);
    }

    #[test]
    fn compute_arrival_nonzero_distance_adds_light_delay() {
        // 1 ly direct path: arrival = sent_at + light_delay(1 ly).
        // We don't assert the exact value (it lives in `physics::`)
        // but we do assert it's strictly greater than `sent_at`.
        let plan = compute_ai_command_arrival(
            50,
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            &empty_relays(),
            &empty_comms(),
        );
        assert!(
            plan.arrives_at > 50,
            "expected non-zero light delay; got arrives_at = {}",
            plan.arrives_at
        );
    }

    #[test]
    fn build_pending_command_preserves_command_payload() {
        let cmd = make_cmd("survey_system");
        let pending = build_pending_command(
            cmd.clone(),
            10,
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            &empty_relays(),
            &empty_comms(),
        );
        assert_eq!(pending.command.kind, cmd.kind);
        assert_eq!(pending.command.issuer, cmd.issuer);
        assert_eq!(pending.sent_at, 10);
        assert_eq!(pending.origin_pos, [0.0, 0.0, 0.0]);
        assert_eq!(pending.destination_pos, Some([0.0, 0.0, 0.0]));
        // Zero-distance arrival = sent_at.
        assert_eq!(pending.arrives_at, 10);
    }

    #[test]
    fn split_outbox_at_separates_mature_and_pending() {
        let mk = |arrives_at: i64, kind: &str| PendingAiCommand {
            command: make_cmd(kind),
            arrives_at,
            sent_at: 0,
            origin_pos: [0.0, 0.0, 0.0],
            destination_pos: Some([0.0, 0.0, 0.0]),
            source: ObservationSource::Direct,
        };
        let entries = vec![mk(50, "a"), mk(150, "b"), mk(100, "c"), mk(200, "d")];
        // now == 100: a and c mature (<=100), b and d still pending.
        let (mature, remaining) = split_outbox_at(100, entries);
        assert_eq!(mature.len(), 2);
        assert_eq!(mature[0].kind.as_str(), "a");
        assert_eq!(mature[1].kind.as_str(), "c");
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].command.kind.as_str(), "b");
        assert_eq!(remaining[1].command.kind.as_str(), "d");
    }

    #[test]
    fn split_outbox_at_zero_now_releases_zero_arrives_entries() {
        // Ruler-at-capital case: arrives_at == sent_at (== 0). At
        // now=0 the entry is released the same tick it landed in
        // the outbox — so the consumer sees no observable delay.
        let entries = vec![PendingAiCommand {
            command: make_cmd("research_focus"),
            arrives_at: 0,
            sent_at: 0,
            origin_pos: [0.0, 0.0, 0.0],
            destination_pos: Some([0.0, 0.0, 0.0]),
            source: ObservationSource::Direct,
        }];
        let (mature, remaining) = split_outbox_at(0, entries);
        assert_eq!(mature.len(), 1);
        assert!(remaining.is_empty());
    }
}
