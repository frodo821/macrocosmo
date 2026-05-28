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

use crate::ai::command_params::{TARGET_SYSTEM, optional_system, ship_list, target_system};
use crate::ai::convert::{from_ai_entity, to_ai_faction};
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
pub fn command_targets_system(_kind: &str) -> bool {
    // #468 PR-1/PR-2/PR-3: every ship-control kind that used to live
    // here (`survey_system`, `colonize_system`, `reposition`,
    // `blockade`, `attack_target`, `move_ruler`, `load_deliverable`,
    // `unload_deliverable`, `colonize_planet`) now flows through the
    // `PendingAiShipCommand` per-ship holder with Ruler→ship light
    // delay — not Ruler→target_system. Nothing on this list is
    // legitimately spatial-target-bound anymore.
    //
    // `fortify_system` is a BUILD order (queue a combat ship at a
    // shipyard), not a ship order — it lives on the empire's
    // government side and pays Ruler→capital delay like every other
    // capital-bound command. Listing it here would have made it pay
    // Ruler→target light delay even though the order never reaches
    // the target system; it goes to the capital and the resulting
    // ship is queued at a shipyard.
    //
    // `build_ship` / `build_structure` may carry a `target_system`
    // hint in some policies but the order itself is processed at
    // the empire capital — they fall through to
    // capital-as-destination by design.
    //
    // The function is retained (rather than deleted outright) because
    // `resolve_destination_pos` still calls it on every command for
    // the spatial-vs-capital branch decision; it now uniformly
    // returns `false`, which routes every surviving non-ship-control
    // command to the capital. PR-4+ may delete the call site entirely
    // once the legacy outbox path shrinks further.
    false
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
    let entity = optional_system(&cmd.params, TARGET_SYSTEM)?;
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
/// `command_params::ship_list`). For the dispatch-time
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
    target_system(&cmd.params)
}

/// #468 PR-3: Extract the `target_planet` Entity from a `colonize_planet`
/// AI command. Returns `None` when the param is missing or wrong-typed —
/// the dispatcher treats that as "no marker to stamp" and lets the
/// holder fly without the planet-target dedup, mirroring the legacy
/// handler's debug! + drop behaviour.
pub fn extract_target_planet(cmd: &Command) -> Option<Entity> {
    use macrocosmo_ai::CommandValue;
    match cmd.params.get("target_planet")? {
        CommandValue::Entity(e) => Some(from_ai_entity(*e)),
        _ => None,
    }
}

/// #468 PR-3: Extract the courier ship Entity for `unload_deliverable`.
/// The kind historically accepts either `ship` or `ship_0` (indexed
/// `ship_count`/`ship_<i>` is the AI Short layer's standard shape; the
/// `ship` alias is a legacy carry-over from #446).
pub fn extract_unload_ship(cmd: &Command) -> Option<Entity> {
    use macrocosmo_ai::CommandValue;
    if let Some(CommandValue::Entity(e)) = cmd.params.get("ship") {
        return Some(from_ai_entity(*e));
    }
    if let Some(CommandValue::Entity(e)) = cmd.params.get("ship_0") {
        return Some(from_ai_entity(*e));
    }
    None
}

/// #468 PR-3: Pick an idle transport ship at the Ruler's current system
/// for `move_ruler`. Mirrors the selection logic the legacy
/// `handle_move_ruler` used: owned by this empire, mobile, in-system at
/// the Ruler's system, empty command queue, no Ruler already aboard.
///
/// Returns `None` when the Ruler is missing, already aboard a ship, or
/// no transport is available — the caller drops the command with a
/// `debug!` rather than re-queueing.
///
/// **Why this kind needs dispatcher-side selection** (NICE-TO-FIX #2
/// fold-in): every other ship-control kind receives a chosen ship via
/// `ship_<i>` params — the AI Short layer's policy step picks the
/// candidate ship at the call site. `move_ruler` is the exception
/// because the AI policy emits it as soon as the *Ruler decides to
/// move*; the ship selection is a downstream concern that depends on
/// which transport happens to be idle at the Ruler's system at
/// dispatch time. Pushing the selection up into the policy would mean
/// re-emitting whenever the chosen ship becomes unavailable. Keeping
/// it in the dispatcher is cheaper and matches the legacy
/// `handle_move_ruler` precedent.
///
/// **Adding more self-selecting kinds in PR-4+**: if a future kind
/// needs similar dispatcher-side ship selection, add a new
/// [`ShipKindStrategy`] variant (e.g. `SelfSelecting(fn selector)`)
/// rather than copying this function — the variant lets the selection
/// logic stay in `command_outbox.rs` alongside the dispatch table
/// rather than spreading across modules.
fn select_move_ruler_transport(
    empire_entity: Entity,
    params: &mut DispatchParams,
) -> Option<Entity> {
    use crate::ship::{Owner, ShipState};
    let ruler_entity = params.empire_rulers.get(empire_entity).ok()?.0;
    let (stationed, aboard) = params.rulers.get(ruler_entity).ok()?;
    if aboard.is_some() {
        return None;
    }
    let ruler_system = stationed?.system;
    // Query the world for ships at the ruler's system. We synthesise the
    // query from the existing `params.ships` (read-only `&Ship`) plus
    // the world's `ShipState` / `CommandQueue` — but DispatchParams only
    // has `&Ship` and `&Position`, so we have to extend the SystemParam.
    // Use Bevy's `world_unsafe_cell` indirectly: the simpler path is to
    // accept that this selection lives inside the SystemParam (we'll add
    // a `ship_state_query` field below to keep types clean).
    params
        .ship_state_query
        .iter()
        .find(|(_, ship, state, queue)| {
            ship.owner == Owner::Empire(empire_entity)
                && !ship.is_immobile()
                && matches!(state, ShipState::InSystem { system } if *system == ruler_system)
                && queue.commands.is_empty()
                && !ship.ruler_aboard
        })
        .map(|(e, _, _, _)| e)
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
    /// #468 PR-3: ship + state + queue triplet used by
    /// `select_move_ruler_transport` to pick an idle transport at the
    /// Ruler's current system. `Ship` is already accessible through
    /// `ships` above, but bundling it here with `ShipState` +
    /// `CommandQueue` keeps the read-only access pattern explicit and
    /// lets the helper iterate without re-fetching components per
    /// entity.
    pub ship_state_query: Query<
        'w,
        's,
        (
            Entity,
            &'static Ship,
            &'static crate::ship::ShipState,
            &'static crate::ship::CommandQueue,
        ),
    >,
}

/// #468 PR-3 fold-in (HIGH C complete): per-kind strategy for the
/// ship-control dispatch table in `dispatch_ai_pending_commands`.
///
/// Each variant captures everything the dispatcher needs to know about a
/// kind that ISN'T already in the generic `dispatch_ship_command_per_ship`
/// path. Adding a new ship-control kind in PR-4+ is:
///
///   1. Add one row to `dispatch_table` in `dispatch_ai_pending_commands`
///      pairing the new `CommandKindId` with the appropriate variant
///      below (or a new variant if the kind has truly novel routing
///      needs).
///   2. Add one matching arm to the drain table in
///      `command_consumer::drain_ai_ship_commands` so the matured
///      holder finds its apply function.
///
/// No other dispatcher code changes — the cascade of `if cmd.kind == X`
/// arms that PR-2 used has been folded into the table-driven lookup.
enum ShipKindStrategy {
    /// `target_system` from cmd params, optional marker via the supplied
    /// factory function. Used by `survey_system` (factory:
    /// `PendingAssignment::survey_system`) and `colonize_system`
    /// (factory: `PendingAssignment::colonize_system`).
    SystemMarker(fn(Entity, Entity, i64) -> crate::ai::assignments::PendingAssignment),
    /// `target_planet` from cmd params, stamps
    /// `PendingAssignment::colonize_planet` capturing the planet. The
    /// dispatcher also propagates the planet into the
    /// `PendingAiShipCommand` holder so the drain emits
    /// `ColonizeRequested { planet: Some(p) }`.
    PlanetMarker,
    /// `target_system` from cmd params, no marker. Used for movement /
    /// cargo orders (Reposition, Blockade, AttackTarget,
    /// LoadDeliverable).
    NoMarker,
    /// `unload_deliverable`: no meaningful `target_system`; the
    /// dispatcher uses `ship.home_port` as a stable sentinel. The
    /// dedup scan in `npc_decision.rs` skips this kind entirely.
    UnloadSentinel,
    /// `move_ruler`: emitted before a transport ship is chosen; the
    /// dispatcher selects an idle transport at the Ruler's current
    /// system via `select_move_ruler_transport`.
    MoveRuler,
}

impl ShipKindStrategy {
    /// Apply this strategy: extract kind-specific params from `cmd`,
    /// resolve target/ship overrides, and invoke
    /// `dispatch_ship_command_per_ship` once per ship covered by the
    /// command.
    fn dispatch(
        &self,
        cmd: &Command,
        empire_entity: Entity,
        origin_pos: [f64; 3],
        now: i64,
        commands_buf: &mut Commands,
        params: &mut DispatchParams,
    ) {
        match self {
            ShipKindStrategy::SystemMarker(factory) => {
                dispatch_ship_command_per_ship(
                    cmd,
                    empire_entity,
                    origin_pos,
                    now,
                    commands_buf,
                    params,
                    Some(*factory),
                    None,
                    None,
                    None,
                );
            }
            ShipKindStrategy::PlanetMarker => {
                // Missing `target_planet` is a malformed-command
                // condition (the AI Short layer always sets it before
                // emitting); warn-and-drop to mirror the legacy
                // `handle_colonize_planet` behaviour.
                let target_planet = match extract_target_planet(cmd) {
                    Some(p) => p,
                    None => {
                        warn!(
                            "colonize_planet dispatch: missing target_planet for empire {:?}",
                            empire_entity
                        );
                        return;
                    }
                };
                let factory = move |faction: Entity, _sys: Entity, now: i64| {
                    crate::ai::assignments::PendingAssignment::colonize_planet(
                        faction,
                        target_planet,
                        now,
                    )
                };
                dispatch_ship_command_per_ship(
                    cmd,
                    empire_entity,
                    origin_pos,
                    now,
                    commands_buf,
                    params,
                    Some(factory),
                    None,
                    Some(target_planet),
                    None,
                );
            }
            ShipKindStrategy::NoMarker => {
                dispatch_ship_command_per_ship::<
                    fn(Entity, Entity, i64) -> crate::ai::assignments::PendingAssignment,
                >(
                    cmd,
                    empire_entity,
                    origin_pos,
                    now,
                    commands_buf,
                    params,
                    None,
                    None,
                    None,
                    None,
                );
            }
            ShipKindStrategy::UnloadSentinel => {
                let ship_entity = match extract_unload_ship(cmd) {
                    Some(e) => e,
                    None => {
                        warn!(
                            "unload_deliverable dispatch: missing ship/ship_0 param for empire {:?}",
                            empire_entity
                        );
                        return;
                    }
                };
                let sentinel = match params.ships.get(ship_entity) {
                    Ok(s) => s.home_port,
                    Err(_) => {
                        debug!(
                            "unload_deliverable dispatch: ship {:?} despawned before dispatch",
                            ship_entity
                        );
                        return;
                    }
                };
                dispatch_ship_command_per_ship::<
                    fn(Entity, Entity, i64) -> crate::ai::assignments::PendingAssignment,
                >(
                    cmd,
                    empire_entity,
                    origin_pos,
                    now,
                    commands_buf,
                    params,
                    None,
                    Some(sentinel),
                    None,
                    Some(ship_entity),
                );
            }
            ShipKindStrategy::MoveRuler => {
                // Sanity-check target_system is present up front so the
                // warn names the right kind (the helper also calls
                // `extract_target_system` and would bail otherwise).
                if extract_target_system(cmd).is_none() {
                    warn!(
                        "move_ruler dispatch: missing target_system for empire {:?}",
                        empire_entity
                    );
                    return;
                }
                let ship_entity = match select_move_ruler_transport(empire_entity, params) {
                    Some(e) => e,
                    None => {
                        debug!(
                            "move_ruler dispatch: no idle transport at Ruler's system for empire {:?}",
                            empire_entity
                        );
                        return;
                    }
                };
                dispatch_ship_command_per_ship::<
                    fn(Entity, Entity, i64) -> crate::ai::assignments::PendingAssignment,
                >(
                    cmd,
                    empire_entity,
                    origin_pos,
                    now,
                    commands_buf,
                    params,
                    None,
                    None,
                    None,
                    Some(ship_entity),
                );
            }
        }
    }
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
    mut commands_buf: Commands,
    mut params: DispatchParams,
) {
    let now = clock.elapsed;
    let raw_drained = bus.drain_commands();
    if raw_drained.is_empty() {
        return;
    }

    // #444 hotfix: eager macro decomposition.
    //
    // Mid Rule 3.5 (#444) and #446/#447 emit *macro* commands like
    // `deploy_deliverable` and `colonize_system` that need to be
    // expanded into the 4-step primitive chain (`build_deliverable
    // → load_deliverable → reposition → unload_deliverable`) +
    // `colonize_planet` tail before the dispatcher can route them.
    //
    // Today the only place that runs the game-side
    // `StaticDecompositionRegistry` is `run_short_agents` via
    // `CampaignReactiveShort::tick`, and the Mid layer never goes
    // through a Campaign — it emits straight onto the bus. Without
    // this expansion the macros reach `drain_ai_commands`, get
    // logged with the "reached consumer undecomposed" debug message,
    // and silently drop, which is the core symptom of #444 (NPC
    // empires never move).
    //
    // The expansion is recursive (`deploy_deliverable` macros further
    // into primitives, etc.) but capped at `MAX_EXPANSION_DEPTH = 4`
    // levels so a registry mistake (`A → [A, B]`) can't lock the
    // dispatcher in an infinite loop. The cap is well above the
    // worst real chain (`colonize_system → deploy_deliverable +
    // colonize_planet → 4 primitives + colonize_planet`, depth 2),
    // so production paths are unaffected.
    //
    // Macros that already have a primitive entry in `dispatch_table`
    // (today: `colonize_system`) are intentionally NOT expanded —
    // they keep their legacy in-place `PendingAssignment` semantics
    // and the existing colonize-system flow continues to work. The
    // skip-list lives in `dispatch_table_primitive_kinds()` so it
    // stays in sync with the routing table below.
    let decomp_registry = crate::ai::decomposition_rules::build_default_registry();
    let drained = expand_macros_eagerly(raw_drained, &decomp_registry, now);
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

    // #468 PR-3 (fold-in): ship-control kind routing table. Each row
    // pairs a `CommandKindId` with a `ShipKindStrategy` describing the
    // per-kind dispatch shape. The dispatcher looks the cmd up in the
    // table and calls `strategy.dispatch(...)` — adding a new
    // ship-control kind in PR-4+ is a single new row in this slice
    // plus a matching drain-side handler in
    // `command_consumer::drain_ai_ship_commands`.
    //
    // The strategy variants encode the kind-specific bits the
    // dispatcher would otherwise have to special-case inline:
    //   * `SystemMarker(fn)` — stamps `PendingAssignment` keyed on the
    //     command's `target_system`; the function pointer chooses
    //     between Survey / ColonizeSystem.
    //   * `PlanetMarker` — extracts `target_planet` from params, stamps
    //     `PendingAssignment::colonize_planet`, and propagates the
    //     planet through `target_planet_override`.
    //   * `NoMarker` — pure movement / cargo orders (Reposition,
    //     Blockade, AttackTarget, LoadDeliverable). Idempotent under
    //     duplicate dispatch because the apply path validates ship
    //     state.
    //   * `UnloadSentinel` — `unload_deliverable` has no
    //     `target_system` and `select_move_ruler_transport`-style ship
    //     resolution: the dispatcher uses `ship.home_port` as a stable
    //     sentinel for the holder. The dedup scan skips this kind.
    //   * `MoveRuler` — boarding kind; dispatcher selects an idle
    //     transport at the Ruler's current system instead of consuming
    //     a `ship_<i>` param (the kind is emitted before any ship is
    //     chosen).
    let dispatch_table: &[(macrocosmo_ai::CommandKindId, ShipKindStrategy)] = &[
        (
            cmd_ids::survey_system(),
            ShipKindStrategy::SystemMarker(
                crate::ai::assignments::PendingAssignment::survey_system,
            ),
        ),
        (
            cmd_ids::colonize_system(),
            ShipKindStrategy::SystemMarker(
                crate::ai::assignments::PendingAssignment::colonize_system,
            ),
        ),
        (cmd_ids::colonize_planet(), ShipKindStrategy::PlanetMarker),
        (cmd_ids::reposition(), ShipKindStrategy::NoMarker),
        (cmd_ids::blockade(), ShipKindStrategy::NoMarker),
        (cmd_ids::attack_target(), ShipKindStrategy::NoMarker),
        (cmd_ids::load_deliverable(), ShipKindStrategy::NoMarker),
        (
            cmd_ids::unload_deliverable(),
            ShipKindStrategy::UnloadSentinel,
        ),
        (cmd_ids::move_ruler(), ShipKindStrategy::MoveRuler),
    ];

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

        // #468 PR-1/PR-2/PR-3: ship-control commands branch onto the
        // new per-ship `PendingAiShipCommand` pipeline. Light delay is
        // Ruler→ship (not Ruler→target_system), and the marker
        // insertion (when applicable) happens NOW (not at arrival) so
        // the `npc_decision.rs` outbox-dedup scan still sees in-flight
        // commands during the courier window.
        if let Some((_, strategy)) = dispatch_table.iter().find(|(k, _)| cmd.kind == *k) {
            strategy.dispatch(
                &cmd,
                empire_entity,
                origin_pos,
                now,
                &mut commands_buf,
                &mut params,
            );
            continue;
        }

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

/// #468 PR-2/PR-3: generic per-ship dispatcher for AI ship-control commands.
///
/// PR-1 introduced this shape for `survey_system`; PR-2 generalised it for
/// `colonize_system`, `reposition`, and `blockade`; PR-3 widens it again so
/// `attack_target`, `move_ruler`, `load_deliverable`, `unload_deliverable`,
/// and `colonize_planet` route through the same path. The kind-specific
/// bits are isolated to a handful of arguments:
///
/// * `assignment_factory` — `Some(closure)` for kinds that participate in
///   the per-faction "don't double-dispatch" dedup contract (Survey,
///   Colonize, ColonizePlanet); `None` for kinds that don't (Reposition,
///   Blockade, AttackTarget, MoveRuler, Load/UnloadDeliverable — movement
///   / cargo orders aren't "decisions" the AI needs to remember; a second
///   dispatch is idempotent because `apply_*_to_ship` validates ship
///   state and bails on duplicates). Widened from `Option<fn>` to
///   `Option<impl Fn(...)>` in PR-3 (HIGH B fold-in) so `colonize_planet`
///   can capture the planet entity it needs to stamp the marker.
/// * `target_system_override` — `Some(entity)` for kinds where target
///   resolution differs from the command's `target_system` param
///   (today: `unload_deliverable`, which has no target_system and uses
///   the ship's `home_port` as a stable sentinel for the holder). `None`
///   means read `target_system` from the command params and bail if
///   absent.
/// * `target_planet` — `Some(entity)` for `colonize_planet`, propagated
///   into the holder so the drain emits `ColonizeRequested { planet:
///   Some(p) }`. `None` for every other kind.
/// * `ship_entity_override` — `Some(entity)` for kinds that don't carry
///   `ship_<i>` params in the command (today: `move_ruler`, which is
///   emitted with just `target_system` and the dispatcher selects the
///   transport ship from the Ruler's current system). `None` falls back
///   to `command_params::ship_list`.
///
/// For each ship that survives the resolution above:
///   * read the ship's `Position` (= dispatcher's *real* idea of where
///     the order has to travel — the snapshot fallback path is reserved
///     for the projection write only)
///   * compute `arrives_at = sent_at + light_delay_ruler_to_ship(ruler, ship)`
///   * spawn `PendingAiShipCommand` with the per-ship arrival
///   * optionally insert a `PendingAssignment` on the ship NOW (the
///     dedup contract at `npc_decision.rs` requires the marker be
///     visible while the courier window is open)
///   * write the dispatcher's `ShipProjection` belief
fn dispatch_ship_command_per_ship<F>(
    cmd: &Command,
    empire_entity: Entity,
    origin_pos: [f64; 3],
    now: i64,
    commands_buf: &mut Commands,
    params: &mut DispatchParams,
    assignment_factory: Option<F>,
    target_system_override: Option<Entity>,
    target_planet: Option<Entity>,
    ship_entity_override: Option<Entity>,
) where
    F: Fn(Entity, Entity, i64) -> crate::ai::assignments::PendingAssignment,
{
    use crate::ai::command_consumer::PendingAiShipCommand;
    use crate::physics::light_delay_ruler_to_ship;

    let kind_str = cmd.kind.as_str();

    // PR-3: most kinds carry `target_system` in their params; a few
    // (unload_deliverable) don't and the override lets the caller
    // supply a sentinel. Either way the dispatcher needs a stable
    // entity to stuff into the holder.
    let target_system = match target_system_override.or_else(|| extract_target_system(cmd)) {
        Some(t) => t,
        None => {
            warn!(
                "{} dispatch: missing target_system for empire {:?}",
                kind_str, empire_entity
            );
            return;
        }
    };

    // PR-3: most kinds carry `ship_<i>` params from the policy's ship
    // selection step; `move_ruler` is emitted before the ship is
    // chosen (the dispatcher picks an idle transport at the Ruler's
    // current system) so the caller passes the resolved ship here.
    let ship_list: Vec<Entity> = match ship_entity_override {
        Some(e) => vec![e],
        None => ship_list(&cmd.params),
    };
    if ship_list.is_empty() {
        debug!(
            "{} dispatch: no ships in command for empire {:?}",
            kind_str, empire_entity
        );
        return;
    }

    for ship_entity in ship_list {
        // Ship position is ground-truth ECS `Position` — that's what the
        // *order* has to physically reach. The snapshot-based projection
        // belief still flows through `compute_ship_projection` below so
        // the dispatcher's KnowledgeStore reflects what the empire
        // *thinks* about the ship's state, not where the order travels.
        let ship_pos = match params.positions.get(ship_entity) {
            Ok(p) => p.as_array(),
            Err(_) => {
                debug!(
                    "{} dispatch: ship {:?} has no Position (despawned?)",
                    kind_str, ship_entity
                );
                continue;
            }
        };

        let arrives_at = now + light_delay_ruler_to_ship(origin_pos, ship_pos);

        // #475 projection write — preserves the per-empire belief update
        // moved out of the legacy `build_projection_inputs` site. The
        // snapshot lookup and fallback chain mirror the old path; we
        // write one projection per ship in the command.
        let snapshot = params
            .knowledge_stores
            .get(empire_entity)
            .ok()
            .and_then(|store| store.get_ship(ship_entity).cloned());
        let fallback_system = params.ships.get(ship_entity).ok().map(|s| s.home_port);
        let target_system_pos = params
            .star_positions
            .get(target_system)
            .ok()
            .map(|p| p.as_array());
        let projection_ship_pos = match snapshot.as_ref().map(|s| &s.last_known_state) {
            Some(crate::knowledge::ShipSnapshotState::Loitering { position }) => *position,
            _ => snapshot
                .as_ref()
                .and_then(|s| s.last_known_system)
                .or(fallback_system)
                .and_then(|sys| params.star_positions.get(sys).ok().map(|p| p.as_array()))
                .unwrap_or(origin_pos),
        };
        let intended_state = crate::knowledge::command_kind_to_intended_state(kind_str);
        let has_return_leg = crate::knowledge::command_kind_has_return_leg(kind_str);

        // Hotfix (#490/#528 fold-in): skip the projection write when
        // the kind has no `intended_state` to express
        // (`load_deliverable` / `unload_deliverable` today). PR #528's
        // eager macro decomposition for `deploy_deliverable` emits the
        // primitive chain `build → load → reposition → unload` into the
        // outbox simultaneously; without this guard, the trailing
        // `unload_deliverable` projection (with `intended_state = None`,
        // `intended_system = ship.home_port` sentinel) would clobber
        // the meaningful `reposition` extrapolation that just ran a few
        // commands earlier — leaving the renderer with no dashed
        // extrapolation line and freezing the ship marker at the
        // origin. The player dispatch path already skips spatial-less
        // commands per #493 (`dispatcher_skips_spatial_less_commands`);
        // this aligns AI dispatch with the same contract.
        if intended_state.is_none() {
            debug!(
                "{} dispatch: skipping projection write for ship {:?} (kind has no intended_state — \
                 spatial-less primitive; preserves any prior reposition extrapolation in the same tick)",
                kind_str, ship_entity
            );
            // Spawn the in-flight holder anyway (it's still needed for
            // command_consumer apply / dedup), but fall through past
            // the projection write below.
            commands_buf.spawn(PendingAiShipCommand {
                kind: cmd.kind.clone(),
                target_system,
                target_planet,
                ship: ship_entity,
                issuer_empire: empire_entity,
                sent_at: now,
                arrives_at,
            });
            if let Some(ref factory) = assignment_factory {
                commands_buf
                    .entity(ship_entity)
                    .insert(factory(empire_entity, target_system, now));
            }
            continue;
        }

        let projection = compute_ship_projection(
            ship_entity,
            snapshot.as_ref(),
            origin_pos,
            projection_ship_pos,
            target_system_pos,
            intended_state,
            Some(target_system),
            has_return_leg,
            fallback_system,
            now,
        );
        if let Ok(mut store) = params.knowledge_stores.get_mut(empire_entity) {
            store.update_projection(projection);
        }

        // Spawn the in-flight holder and (for dedup-aware kinds) stamp
        // the marker. Stamping NOW (not at arrival) is load-bearing: the
        // `npc_decision.rs` outbox scan reads
        // `Query<&PendingAiShipCommand>` to decide whether to re-emit
        // a command for the same target during the courier window.
        //
        // Only the kind + target_system are stored — the full `cmd`
        // (with its AHashMap of params and stale ship_<i> list) is not
        // needed downstream. Per-ship multi-target commands spawn N
        // holders here; cloning the whole command would N× the
        // param-map allocations for no benefit.
        commands_buf.spawn(PendingAiShipCommand {
            kind: cmd.kind.clone(),
            target_system,
            target_planet,
            ship: ship_entity,
            issuer_empire: empire_entity,
            sent_at: now,
            arrives_at,
        });
        if let Some(ref factory) = assignment_factory {
            // PR-3: factory is now `impl Fn`, so it can capture
            // `target_planet` (for `colonize_planet`) or whatever
            // other per-kind context the marker needs. The factory's
            // second argument is the marker's `target` entity — for
            // System-target kinds (Survey, ColonizeSystem) we pass
            // `target_system`; for `colonize_planet` we'd pass the
            // planet directly, but in practice the factory closes
            // over the planet entity and ignores this arg (see the
            // call site in `dispatch_ai_pending_commands`).
            commands_buf
                .entity(ship_entity)
                .insert(factory(empire_entity, target_system, now));
        }
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

/// #444 hotfix: cap on macro-expansion recursion depth used by
/// [`expand_macros_eagerly`]. Set higher than the worst real chain
/// (`colonize_system` → `deploy_deliverable` + `colonize_planet`
/// → 4 primitives + `colonize_planet`, depth 2) so a future macro
/// addition does not force a constant bump; but small enough that
/// a buggy registry rule (`A → [A, …]`) self-terminates rather than
/// hanging the dispatcher.
const MAX_MACRO_EXPANSION_DEPTH: usize = 4;

/// #444 hotfix: list of `CommandKindId`s the dispatcher routes as
/// primitives via the `dispatch_table` in
/// [`dispatch_ai_pending_commands`]. These kinds are intentionally
/// **not** expanded by [`expand_macros_eagerly`] even when the
/// decomposition registry knows a rule for them — the primitive
/// dispatch path is the contract callers rely on for
/// `colonize_system` (Mid Rule 3 emits it expecting
/// `PendingAssignment::colonize_system` semantics, not a
/// 4-step deploy chain). Add a row here only after auditing
/// `dispatch_table` to confirm the kind has a routing entry.
fn dispatch_table_primitive_kinds() -> &'static [&'static str] {
    // Hard-coded as static strings (rather than calling cmd_ids::*)
    // because `cmd_ids::xxx()` returns an `Arc<str>`-backed
    // `CommandKindId`, which has nontrivial init cost and complicates
    // a `const` slice. The string literals are checked against the
    // registry's IDs by `dispatch_table_skip_list_matches_table()`
    // in tests, so a typo in either place fails CI.
    &["colonize_system"]
}

/// #444 hotfix: recursively expand macro commands against the
/// decomposition registry, skipping kinds that ship a primitive
/// route via `dispatch_table` (see
/// [`dispatch_table_primitive_kinds`]).
///
/// Returns a flat `Vec<Command>` of primitives + skipped macros in
/// the original emit order. Macros that the registry knows about
/// are dropped from the output and replaced by their expansions
/// (recursively, up to `MAX_MACRO_EXPANSION_DEPTH` levels).
/// Macros without a registry rule pass through unchanged so future
/// rule additions are a single registry change.
///
/// `now` is the current `GameClock.elapsed` tick — forwarded to
/// each rule's `expand` function so the synthetic primitives carry
/// the same `at` as the macro they came from.
pub fn expand_macros_eagerly(
    drained: Vec<Command>,
    registry: &macrocosmo_ai::StaticDecompositionRegistry,
    now: i64,
) -> Vec<Command> {
    use macrocosmo_ai::DecompositionRegistry;
    let skip_list = dispatch_table_primitive_kinds();
    let plan_state = macrocosmo_ai::PlanState::default();
    let mut out: Vec<Command> = Vec::with_capacity(drained.len());
    // Depth-first work stack: each entry pairs a command with its
    // current recursion depth. Pushing children with `depth + 1`
    // means a cycle (`A → A`) hits `MAX_MACRO_EXPANSION_DEPTH` and
    // self-terminates rather than spinning forever.
    let mut stack: Vec<(Command, usize)> = drained.into_iter().rev().map(|c| (c, 0)).collect();
    while let Some((cmd, depth)) = stack.pop() {
        let kind_str = cmd.kind.as_str();
        if skip_list.iter().any(|k| *k == kind_str) {
            out.push(cmd);
            continue;
        }
        if depth >= MAX_MACRO_EXPANSION_DEPTH {
            // Bail out: refuse to recurse further. Pass the command
            // through unchanged so downstream layers can log /
            // diagnose. `warn!` because a real workload should
            // never hit this — only a buggy rule can.
            warn!(
                "AI macro expansion hit depth cap ({}) for kind={}; \
                 stopping recursion and forwarding macro unchanged",
                MAX_MACRO_EXPANSION_DEPTH, kind_str
            );
            out.push(cmd);
            continue;
        }
        match registry.lookup(&cmd.kind) {
            Some(rule) => {
                let children = (rule.expand)(&cmd, &plan_state, now);
                // Push children in reverse so they pop in original
                // order — keeps the emit / dispatch ordering
                // identical to the rule's `Vec<Command>`.
                for child in children.into_iter().rev() {
                    stack.push((child, depth + 1));
                }
            }
            None => {
                out.push(cmd);
            }
        }
    }
    out
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
        // #468 PR-3: every ship-control kind now flows through the
        // per-ship `PendingAiShipCommand` pipeline. The legacy
        // outbox's target_system path no longer carries any kind, so
        // every input reports `false`. `fortify_system` was
        // mis-categorised as spatial pre-PR-3 — it's a BUILD order
        // routed to the capital like research_focus, NOT to the
        // target system the new ship eventually fortifies.
        for kind in [
            "attack_target",
            "survey_system",
            "colonize_system",
            "reposition",
            "blockade",
            "move_ruler",
            "fortify_system",
            "load_deliverable",
            "unload_deliverable",
            "colonize_planet",
            "research_focus",
            "retreat",
            "declare_war",
        ] {
            assert!(
                !command_targets_system(kind),
                "no kind should be routed through the spatial outbox path post-#468 PR-3; \
                 got true for {kind}",
            );
        }
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
