//! Command drain consumer — converts AI bus commands into ECS game actions.
//!
//! Registered under [`AiTickSet::CommandDrain`](super::AiTickSet::CommandDrain).
//! Each tick, drains pending commands from the bus and applies them:
//!
//! - `attack_target` → find idle ships owned by the issuing faction, emit
//!   `MoveRequested` for the target system.
//! - `retreat` → find ships in hostile systems, emit `MoveRequested` back to
//!   the faction's home system (system with most colonies).
//! - `build_ship` → queue ship construction at a system with a shipyard.
//! - `fortify_system` → queue a default combat ship at a system with a shipyard.
//! - `research_focus` → set the empire's active research target.
//! - `build_structure` → queue a building at a colony.
//! - `reposition` / `blockade` → move ships to a target system.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::ai::command_handlers::build::{
    handle_build_deliverable, handle_build_ship, handle_build_structure, handle_fortify_system,
};
use crate::ai::command_handlers::military::handle_retreat;
use crate::ai::command_handlers::research::handle_research_focus;
use crate::ai::command_route::{CommandRoute, classify};
use crate::ai::emit::AiBusDrainer;
use crate::colony::building_queue::{BuildQueue, BuildingQueue, Buildings};
use crate::colony::system_buildings::SlotAssignment;
use crate::colony::{BuildingRegistry, Colony};
use crate::components::Position;
use crate::galaxy::{AtSystem, Hostile, Planet, Sovereignty, StarSystem};
use crate::player::{AboardShip, Empire, EmpireRuler, Faction};
use crate::ship::command_events::{
    ColonizeRequested, DeployDeliverableRequested, LoadDeliverableRequested, MoveRequested,
    NextCommandId, SurveyRequested,
};
use crate::ship::{CommandQueue, Owner, Ship, ShipState};
use crate::ship_design::ShipDesignRegistry;
use crate::technology::{ResearchQueue, TechTree};
use crate::time_system::GameClock;

/// Queued ruler boarding requests produced by `drain_ai_commands` and
/// consumed by [`process_ruler_boarding`]. This indirection avoids adding
/// mutable Ship access to `drain_ai_commands` (which would conflict with
/// the existing read-only Ship query).
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct PendingRulerBoarding {
    /// `(ruler_entity, ship_entity, target_system)`
    pub requests: Vec<(Entity, Entity, Entity)>,
}

/// Extra queries needed by build / research / structure commands, bundled
/// into a `SystemParam` to keep `drain_ai_commands` under Bevy's 16-param
/// limit.
#[derive(SystemParam)]
pub struct BuildResearchParams<'w, 's> {
    pub(in crate::ai) design_registry: Option<Res<'w, ShipDesignRegistry>>,
    pub(in crate::ai) building_registry: Option<Res<'w, BuildingRegistry>>,
    /// #532 F1: deliverable / structure registry borrow used by
    /// [`handle_build_deliverable`] to resolve cost / build_time /
    /// cargo_size / display_name for deliverable ids such as
    /// `"infrastructure_core"`. The deliverable id space is distinct
    /// from `ShipDesignRegistry` — `define_deliverable { id =
    /// "infrastructure_core" }` lives here, while
    /// `define_ship_design { id = "infrastructure_core_v1" }` lives in
    /// `design_registry`. Before the F1 fold-in the handler resolved
    /// deliverables through `design_registry`; tests masked the bug by
    /// injecting a fake `ShipDesignDefinition` with the deliverable id,
    /// so production Rule 3.5 frontier core deployment silently stalled.
    pub(in crate::ai) deliverable_registry: Option<Res<'w, crate::deep_space::DeliverableRegistry>>,
    /// #470: Ship build orders live on the **Colony** entity (mirror of the
    /// player UI in `system_panel::mod.rs` — the player-facing flow always
    /// picks the first colony in the system as the host). Pre-#470 the AI
    /// queried `Query<&mut BuildQueue>` keyed by `StarSystem` entity, which
    /// always returned `Err` because `BuildQueue` is spawned only on Colony
    /// — `queue_ship_at_shipyard` / `handle_build_deliverable` silently
    /// dropped every order.
    ///
    /// **Stricter than the player UI**: the AI's host-colony pick filters
    /// on `FactionOwner == issuing empire` in addition to the system
    /// match (see [`pick_host_colony`]). Player UI relies on an upstream
    /// `is_own_system` gate (only the player's own systems show the
    /// ship-construction panel), so its first-colony pick is safe by
    /// construction. The AI does NOT have an equivalent upstream gate —
    /// `dispatch_ai_pending_commands` and the outbox accept any owned
    /// system, and a colony in that system may belong to a different
    /// empire in conquered / split-ownership scenarios. The stricter
    /// filter here prevents an AI from pushing build orders into another
    /// faction's production queue.
    pub(in crate::ai) build_queues: Query<
        'w,
        's,
        (
            Entity,
            &'static Colony,
            &'static crate::faction::FactionOwner,
            &'static mut BuildQueue,
        ),
    >,
    pub(in crate::ai) station_ships: Query<
        'w,
        's,
        (
            Entity,
            &'static Ship,
            &'static ShipState,
            &'static SlotAssignment,
        ),
    >,
    pub(in crate::ai) sys_mods_q: Query<'w, 's, &'static crate::galaxy::SystemModifiers>,
    pub(in crate::ai) empire_tech:
        Query<'w, 's, (&'static mut TechTree, &'static mut ResearchQueue), With<Empire>>,
    pub(in crate::ai) colonies: Query<
        'w,
        's,
        (
            Entity,
            &'static Colony,
            &'static Buildings,
            &'static mut BuildingQueue,
        ),
    >,
    pub(in crate::ai) planets: Query<'w, 's, &'static Planet>,
    /// System-level building queues + slot state, used by the system-
    /// building branch of `handle_build_structure` to route shipyard /
    /// port / lab orders to the correct queue.
    pub(in crate::ai) system_builds: Query<
        'w,
        's,
        (
            Entity,
            &'static crate::colony::SystemBuildings,
            &'static mut crate::colony::SystemBuildingQueue,
        ),
        With<StarSystem>,
    >,
    /// Tracks which systems host a Core-equipped ship. Required gate for
    /// system-building construction (#370): shipyard / port / lab are only
    /// buildable in systems with an Infrastructure Core.
    pub(in crate::ai) core_at_system:
        Query<'w, 's, &'static crate::galaxy::AtSystem, With<crate::ship::CoreShip>>,
}

// #468 PR-3: `DeliverableParams` retired — every handler that used it
// (`handle_load_deliverable`, `handle_unload_deliverable`,
// `handle_colonize_planet`) migrated to the per-ship
// `PendingAiShipCommand` pipeline. The drain-side functions take
// only the message writers they need, plumbed through
// `DrainShipCommandWriters`.

/// #446: Bundle of "stamp the new ECS message" infrastructure (writers,
/// command-id allocator, clock) used by every dispatch arm in
/// `drain_ai_commands`. Bundling these into a single `SystemParam` keeps
/// the system under Bevy's 16-param limit when new arms are added.
#[derive(SystemParam)]
pub struct CommandStamp<'w> {
    move_writer: MessageWriter<'w, MoveRequested>,
    next_cmd_id: ResMut<'w, NextCommandId>,
    clock: Res<'w, GameClock>,
}

/// Drain AI commands from the bus and apply them to the game world.
///
/// #468 PR-3 shrunk this surface: 5 ship-control kinds (`attack_target`,
/// `move_ruler`, `load_deliverable`, `unload_deliverable`,
/// `colonize_planet`) migrated to `drain_ai_ship_commands`, so the
/// per-kind `handle_*` functions and their SystemParam dependencies
/// (`empire_rulers`, `ruler_q`, `pending_boarding`, `deliverable`)
/// dropped out of this signature. What remains is the government-side
/// command surface (build / research / retreat / fortify) plus
/// debug-drop arms for stale legacy emissions of the migrated kinds.
pub fn drain_ai_commands(
    mut drainer: AiBusDrainer,
    ships: Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    sovereignty: Query<(Entity, &Sovereignty), With<StarSystem>>,
    hostiles: Query<&AtSystem, With<Hostile>>,
    empires: Query<(Entity, &Faction), With<Empire>>,
    positions: Query<&Position>,
    mut stamp: CommandStamp,
    mut build_research: BuildResearchParams,
) {
    let commands = drainer.drain_commands();
    if commands.is_empty() {
        return;
    }
    let now = stamp.clock.elapsed;

    for cmd in commands {
        let kind_str = cmd.kind.as_str();

        match classify(&cmd.kind) {
            CommandRoute::StaleShipControl => {
                // #468 PR-1/PR-2/PR-3: this kind was migrated to the
                // per-ship `PendingAiShipCommand` pipeline (consumed by
                // `drain_ai_ship_commands`). A command reaching this arm
                // means an upstream call path bypassed the dispatcher —
                // log + drop rather than silently re-dispatch.
                debug!(
                    "drain_ai_commands: stale {} from faction {:?} hit legacy \
                     dispatch; expected `drain_ai_ship_commands` to handle this",
                    kind_str, cmd.issuer
                );
            }
            CommandRoute::Retreat => {
                handle_retreat(
                    &cmd.issuer,
                    &ships,
                    &hostiles,
                    &sovereignty,
                    &empires,
                    &positions,
                    &mut stamp.move_writer,
                    &mut stamp.next_cmd_id,
                    now,
                );
            }
            CommandRoute::BuildShip => {
                handle_build_ship(
                    &cmd.issuer,
                    &cmd.params,
                    &sovereignty,
                    &empires,
                    &mut build_research,
                );
            }
            CommandRoute::FortifySystem => {
                handle_fortify_system(
                    &cmd.issuer,
                    &cmd.params,
                    &sovereignty,
                    &empires,
                    &mut build_research,
                );
            }
            CommandRoute::ResearchFocus => {
                handle_research_focus(&cmd.issuer, &cmd.params, &empires, &mut build_research);
            }
            CommandRoute::BuildStructure => {
                handle_build_structure(
                    &cmd.issuer,
                    &cmd.params,
                    &sovereignty,
                    &empires,
                    &mut build_research,
                );
            }
            CommandRoute::BuildDeliverable => {
                handle_build_deliverable(
                    &cmd.issuer,
                    &cmd.params,
                    &sovereignty,
                    &empires,
                    &mut build_research,
                );
            }
            CommandRoute::DeployDeliverableMacro => {
                // Macro command — decomposed by the Short layer (#447). The
                // consumer-side arm exists so an undecomposed `deploy_deliverable`
                // doesn't slip past the dispatcher silently; if we ever see one
                // here it indicates the Short layer didn't pick it up.
                debug!(
                    "deploy_deliverable from faction {:?} reached consumer undecomposed; \
                     expected the Short layer to expand this macro into primitives",
                    cmd.issuer
                );
            }
            CommandRoute::Unknown => {
                debug!(
                    "AI command '{}' from faction {:?} not handled by drain_ai_commands",
                    kind_str, cmd.issuer
                );
            }
        }
    }
}

// #468 PR-3: `handle_attack_target` removed — the attack_target arm of
// `drain_ai_commands` now logs and drops stale legacy emissions; the
// canonical dispatch path is the per-ship `PendingAiShipCommand`
// pipeline + `apply_move_to_ship("attack_target", ...)`.

// #468 PR-2: `handle_colonize_system` removed — the colonize_system arm
// of `drain_ai_commands` now logs and drops stale legacy emissions; the
// canonical dispatch path is the per-ship `PendingAiShipCommand`
// pipeline + `apply_colonize_to_ship`.

// #468 PR-2: `handle_reposition` and `handle_blockade` removed — both
// kinds are now produced via the per-ship `PendingAiShipCommand`
// pipeline and consumed by `apply_reposition_to_ship` /
// `apply_blockade_to_ship`. `dispatch_ships_to_target` was deleted in
// the PR-2 review fold-in (it had no live callers); PR-3 will choose
// between `apply_move_to_ship` and a freshly-extracted helper for
// `attack_target` / `fortify_system` based on whichever shape fits.

// ---------------------------------------------------------------------------
// Deliverable family handlers (#446)
// ---------------------------------------------------------------------------
//
// These handlers bridge AI bus commands to the existing ECS event surface.
// They do **not** add new game mechanics — the underlying flow (queue a
// deliverable in a colony BuildingQueue, board it onto a ship via
// `LoadDeliverableRequested`, drop it via `DeployDeliverableRequested`) is
// already exercised by the player-controlled and Lua-controlled paths. The
// AI handlers reuse the same events so all three command sources converge
// on a single set of authoritative system handlers.

// #468 PR-3: `handle_load_deliverable`, `handle_unload_deliverable`,
// and `handle_colonize_planet` removed — all three migrated to the
// per-ship `PendingAiShipCommand` pipeline. The `*Requested` events
// they used to write now flow through `apply_load_deliverable_to_ship`,
// `apply_unload_deliverable_to_ship`, and `apply_colonize_to_ship`
// respectively.

// #468 PR-3: `handle_move_ruler` removed — `move_ruler` migrated to
// the per-ship `PendingAiShipCommand` pipeline. The dispatcher
// selects the transport ship (mirroring the legacy
// "ruler-system + idle + mobile + no-ruler-aboard" filter); the
// drain pushes `PendingRulerBoarding` and emits `MoveRequested`.

/// Process pending ruler boarding requests. Inserts `AboardShip` on the
/// ruler and sets `ruler_aboard = true` on the ship.
pub fn process_ruler_boarding(
    mut commands: Commands,
    mut pending: ResMut<PendingRulerBoarding>,
    mut ships: Query<&mut Ship>,
) {
    for (ruler_entity, ship_entity, _target) in pending.requests.drain(..) {
        commands
            .entity(ruler_entity)
            .insert(AboardShip { ship: ship_entity });
        if let Ok(mut ship) = ships.get_mut(ship_entity) {
            ship.ruler_aboard = true;
        }
    }
}

// ---------------------------------------------------------------------------
// #468 PR-1: per-ship light-speed delayed AI command (survey_system only)
// ---------------------------------------------------------------------------
//
// Background: the previous AI command light-speed shim computed `arrives_at`
// from the issuer Ruler to the command's `target_system`. For ship-control
// commands (survey_system / colonize_system / …) that's the wrong distance
// — the order has to reach the *ship*, not the target. A Ruler at home A
// dispatching a scout already at frontier B should incur ~0 light-delay
// (the scout is "right next to" the system whose authority routes the
// order, conceptually), not the round trip A→B that the old code paid.
//
// PR-1 migrates the `survey_system` arm as a proof of concept. PR-2 / PR-3
// follow with the remaining ship-control kinds. The new wiring is:
//
//   `dispatch_ai_pending_commands`
//     (sees `survey_system` from the bus)
//     ↓ branches out: per ship_<i>, spawn `PendingAiShipCommand` with
//       `arrives_at = sent_at + light_delay_ruler_to_ship(ruler, ship)`,
//       insert `PendingAssignment` marker NOW (preserves dedup contract
//       with `npc_decision.rs`'s outbox-scan).
//   `drain_ai_ship_commands`   (runs at start of `AiTickSet::CommandDrain`,
//                               before `drain_ai_commands`)
//     ↓ for entries where `clock.elapsed >= arrives_at`: write
//       `SurveyRequested`, despawn the holder entity.
//
// Runtime-only — not `Reflect`, not persisted. Mirrors `PendingScriptedCommand`
// in `scripting::gamestate_scope`: in-flight commands are frame-transient
// and surviving save/load is a non-goal for pre-alpha. SAVE_VERSION does
// not bump.

/// In-flight AI-issued ship command awaiting light-speed arrival.
///
/// Spawned by [`crate::ai::command_outbox::dispatch_ai_pending_commands`]
/// at the moment the AI policy emits the command; drained by
/// [`drain_ai_ship_commands`] once `clock.elapsed >= arrives_at`. The
/// `PendingAssignment` marker on `ship` is inserted at the same time the
/// holder is spawned, *not* at arrival — that keeps the dedup contract at
/// `npc_decision.rs:566` intact across the courier window.
///
/// Fields the drain side actually consumes are stored directly rather
/// than the full [`macrocosmo_ai::Command`] — the `params` map (for an
/// N-ship multi-target survey) is ~hundreds of bytes per holder and the
/// drain only reads `target_system`. PR-2/3 extends this struct with
/// kind-specific extras (e.g. `target_planet: Option<Entity>` for
/// `colonize_planet`) rather than cloning the whole command.
#[derive(Component, Debug)]
pub struct PendingAiShipCommand {
    /// Which AI command kind this holder represents. Drain dispatch
    /// branches on this string-interned id (cheap `Arc<str>` clone).
    pub kind: macrocosmo_ai::CommandKindId,
    /// Star system the order targets (= the `target_system` param the
    /// AI command carried at emission). Used by both the drain
    /// (for `SurveyRequested.target_system`) and the dedup scan in
    /// `npc_decision.rs`. For ship-control kinds without a meaningful
    /// system target (e.g. `unload_deliverable`, which deploys at the
    /// ship's *current* position) this is the ship's `home_port` as a
    /// stable sentinel — the apply function ignores it and the dedup
    /// scan does not include the kind in its per-empire maps.
    pub target_system: Entity,
    /// #468 PR-3: planet the order targets (for `colonize_planet`).
    /// `None` for every other kind. The apply function for
    /// `colonize_planet` writes `ColonizeRequested { planet: Some(p) }`
    /// when this is set, vs `planet: None` for `colonize_system`
    /// (which lets the handler pick the best planet at settlement
    /// time). Carrying the planet here avoids re-emitting it through
    /// the command params or re-fetching it from a cached
    /// AssignmentTarget at maturity.
    pub target_planet: Option<Entity>,
    /// The specific ship this holder targets. For multi-ship commands the
    /// outbox spawns one `PendingAiShipCommand` per ship, each with its
    /// own Ruler→ship `arrives_at`.
    pub ship: Entity,
    /// Empire entity that issued the command (= dispatcher empire).
    pub issuer_empire: Entity,
    /// Clock tick (hexadies) when the command was emitted.
    pub sent_at: i64,
    /// Clock tick at which this command becomes deliverable. Computed via
    /// [`crate::physics::light_delay_ruler_to_ship`].
    pub arrives_at: i64,
}

/// SystemParam bundle for the drain-side writers + Ruler-boarding push
/// channel. Bundled because `drain_ai_ship_commands` already needs the
/// command query + ships query + clock + next_cmd_id; PR-3's 9-kind
/// dispatch table would otherwise blow past Bevy's 16-param limit.
#[derive(bevy::ecs::system::SystemParam)]
pub struct DrainShipCommandWriters<'w> {
    pub survey: Option<MessageWriter<'w, SurveyRequested>>,
    pub colonize: Option<MessageWriter<'w, ColonizeRequested>>,
    pub move_: Option<MessageWriter<'w, MoveRequested>>,
    pub load: Option<MessageWriter<'w, LoadDeliverableRequested>>,
    pub deploy: Option<MessageWriter<'w, DeployDeliverableRequested>>,
    /// PR-3: `move_ruler` apply path queues `(ruler, ship,
    /// target_system)` here instead of mutating Ship+Ruler directly
    /// — the boarding step needs `&mut Ship` which conflicts with
    /// the read-only Ship query above. Mirrors the legacy
    /// `handle_move_ruler` → `process_ruler_boarding` indirection.
    pub pending_boarding: ResMut<'w, PendingRulerBoarding>,
}

/// Start-of-`AiTickSet::CommandDrain` system: walk the
/// [`PendingAiShipCommand`] entities and emit the corresponding typed
/// `*Requested` message for any whose `arrives_at` has elapsed. Despawns
/// the holder entity once dispatched.
///
/// PR-1 handled `survey_system`; PR-2 added `colonize_system`,
/// `reposition`, and `blockade`; PR-3 adds `attack_target`,
/// `move_ruler`, `load_deliverable`, `unload_deliverable`, and
/// `colonize_planet`. The kind→apply dispatch is a small table that
/// PR-4+ can grow with one row per new kind (HIGH C fold-in: replaces
/// the if/else cascade from PR-2).
///
/// Unrecognised kinds are dropped defensively with a `debug!` — only
/// kinds produced through `dispatch_ship_command_per_ship` should
/// reach this point. Stale markers are stripped on the unknown path
/// so the ship doesn't get permanently locked out.
pub fn drain_ai_ship_commands(
    mut commands_buf: Commands,
    pending_q: Query<(Entity, &PendingAiShipCommand)>,
    ships: Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    ship_positions: Query<&crate::components::Position, With<Ship>>,
    // #468 PR-3 NICE-TO-FIX #5/#6 fold-in: cargo holds for the
    // load/unload prechecks. Read-only Without<StarSystem> filter
    // keeps the query disjoint from the colony's deliverable
    // stockpile reads on the same world.
    ship_cargos: Query<&crate::ship::Cargo, With<Ship>>,
    // #468 PR-3 NICE-TO-FIX #7 fold-in: deliverable stockpiles on
    // target systems for the load precheck. Empty stockpile means the
    // emit would immediately reject downstream and the AI re-emits
    // each tick spamming logs — gate it here.
    stockpiles: Query<&crate::colony::DeliverableStockpile>,
    empire_rulers: Query<&EmpireRuler, With<Empire>>,
    clock: Res<GameClock>,
    mut writers: DrainShipCommandWriters,
    mut next_cmd_id: ResMut<NextCommandId>,
) {
    let now = clock.elapsed;
    let survey_kind = crate::ai::schema::ids::command::survey_system();
    let colonize_kind = crate::ai::schema::ids::command::colonize_system();
    let reposition_kind = crate::ai::schema::ids::command::reposition();
    let blockade_kind = crate::ai::schema::ids::command::blockade();
    let attack_kind = crate::ai::schema::ids::command::attack_target();
    let move_ruler_kind = crate::ai::schema::ids::command::move_ruler();
    let load_kind = crate::ai::schema::ids::command::load_deliverable();
    let unload_kind = crate::ai::schema::ids::command::unload_deliverable();
    let colonize_planet_kind = crate::ai::schema::ids::command::colonize_planet();

    // Collect mature entries first so we can despawn while iterating
    // without invalidating the query cursor.
    let mut mature: Vec<(Entity, MaturedHolder)> = Vec::new();
    for (holder_entity, pending) in &pending_q {
        if pending.arrives_at <= now {
            mature.push((
                holder_entity,
                MaturedHolder {
                    kind: pending.kind.clone(),
                    target_system: pending.target_system,
                    target_planet: pending.target_planet,
                    ship: pending.ship,
                    issuer_empire: pending.issuer_empire,
                },
            ));
        }
    }

    for (holder_entity, m) in mature {
        let kind_str = m.kind.as_str();
        if kind_str == survey_kind.as_str() {
            apply_survey_to_ship(
                m.ship,
                m.issuer_empire,
                m.target_system,
                &ships,
                writers.survey.as_mut(),
                &mut next_cmd_id,
                &mut commands_buf,
                now,
            );
        } else if kind_str == colonize_kind.as_str() {
            apply_colonize_to_ship(
                m.ship,
                m.issuer_empire,
                m.target_system,
                None, // colonize_system: handler picks the best planet
                &ships,
                writers.colonize.as_mut(),
                &mut next_cmd_id,
                &mut commands_buf,
                now,
            );
        } else if kind_str == colonize_planet_kind.as_str() {
            // PR-3: colonize_planet routes through the same apply
            // function as colonize_system but passes the planet
            // entity through to `ColonizeRequested.planet`. The
            // settlement handler honours `Some(p)` by targeting that
            // exact planet; `None` lets it pick the best in the
            // system. Marker hygiene + reject-branch cleanup are
            // identical (both kinds stamp
            // `PendingAssignment::Colonize`).
            apply_colonize_to_ship(
                m.ship,
                m.issuer_empire,
                m.target_system,
                m.target_planet,
                &ships,
                writers.colonize.as_mut(),
                &mut next_cmd_id,
                &mut commands_buf,
                now,
            );
        } else if kind_str == reposition_kind.as_str() {
            // PR-3 HIGH A fold-in: inlined the apply_reposition_to_ship
            // wrapper. Both reposition and blockade are 1-line shims
            // around `apply_move_to_ship` so we call it directly.
            apply_move_to_ship(
                "reposition",
                m.ship,
                m.issuer_empire,
                m.target_system,
                &ships,
                writers.move_.as_mut(),
                &mut next_cmd_id,
                now,
            );
        } else if kind_str == blockade_kind.as_str() {
            apply_move_to_ship(
                "blockade",
                m.ship,
                m.issuer_empire,
                m.target_system,
                &ships,
                writers.move_.as_mut(),
                &mut next_cmd_id,
                now,
            );
        } else if kind_str == attack_kind.as_str() {
            // PR-3: attack_target ⇒ MoveRequested for the chosen
            // ship. Same wire shape as reposition / blockade — the
            // apply path validates eligibility and writes one move
            // event. No marker (combat orders are
            // policy-emit-each-tick by design).
            apply_move_to_ship(
                "attack_target",
                m.ship,
                m.issuer_empire,
                m.target_system,
                &ships,
                writers.move_.as_mut(),
                &mut next_cmd_id,
                now,
            );
        } else if kind_str == move_ruler_kind.as_str() {
            apply_move_ruler_to_ship(
                m.ship,
                m.issuer_empire,
                m.target_system,
                &ships,
                &empire_rulers,
                writers.move_.as_mut(),
                &mut writers.pending_boarding,
                &mut next_cmd_id,
                now,
            );
        } else if kind_str == load_kind.as_str() {
            apply_load_deliverable_to_ship(
                m.ship,
                m.issuer_empire,
                m.target_system,
                &ships,
                &stockpiles,
                writers.load.as_mut(),
                &mut next_cmd_id,
                now,
            );
        } else if kind_str == unload_kind.as_str() {
            apply_unload_deliverable_to_ship(
                m.ship,
                m.issuer_empire,
                &ships,
                &ship_positions,
                &ship_cargos,
                writers.deploy.as_mut(),
                &mut next_cmd_id,
                now,
            );
        } else {
            // Defensive: only kinds migrated through `dispatch_ship_command_per_ship`
            // should produce holders. Any other kind here means an
            // upstream bug — log + drop rather than silently dispatch
            // through an unknown path. Also strip the stale
            // `PendingAssignment` so the ship is not permanently
            // excluded from future AI dispatches.
            debug!(
                "drain_ai_ship_commands: unexpected kind {} for ship {:?}; dropping",
                m.kind, m.ship
            );
            commands_buf
                .entity(m.ship)
                .remove::<crate::ai::assignments::PendingAssignment>();
        }

        commands_buf.entity(holder_entity).despawn();
    }
}

/// #468 PR-1/PR-3: lightweight snapshot of a matured
/// [`PendingAiShipCommand`] used inside [`drain_ai_ship_commands`] so the
/// drain loop can despawn / mutate via `Commands` without holding a
/// borrow on the source query.
struct MaturedHolder {
    kind: macrocosmo_ai::CommandKindId,
    target_system: Entity,
    /// #468 PR-3: planet target for `colonize_planet` (None for every
    /// other kind).
    target_planet: Option<Entity>,
    ship: Entity,
    issuer_empire: Entity,
}

/// Apply a matured `survey_system` PendingAiShipCommand: validate the ship
/// is still eligible (owned by the issuer, in-system, idle) and write the
/// `SurveyRequested` message.
///
/// The `PendingAssignment` marker was inserted at outbox-spawn time to
/// preserve dedup across the courier window. On any early-return path
/// here (ship despawned / owner-changed / non-idle / no writer) the
/// marker MUST be removed — otherwise the ship is permanently excluded
/// from future AI survey dispatches because the dedup scan at
/// `npc_decision.rs:566` filters by `PendingAssignment`. Pre-#468 the
/// legacy `handle_survey_requested` path cleared the marker on its own
/// reject branches; on the new path, no `SurveyRequested` is even
/// written when these gates fail, so we drop the marker ourselves.
fn apply_survey_to_ship(
    ship_entity: Entity,
    empire_entity: Entity,
    target_system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    survey_writer: Option<&mut MessageWriter<SurveyRequested>>,
    next_cmd_id: &mut NextCommandId,
    commands_buf: &mut Commands,
    now: i64,
) {
    let Some(writer) = survey_writer else {
        warn!("drain_ai_ship_commands: SurveyRequested writer unavailable");
        commands_buf
            .entity(ship_entity)
            .remove::<crate::ai::assignments::PendingAssignment>();
        return;
    };

    let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
        debug!(
            "drain_ai_ship_commands: ship {:?} despawned before arrival",
            ship_entity
        );
        // Ship is gone — the `PendingAssignment` was on its
        // (despawned) entity, so no further cleanup is required.
        return;
    };
    if ship.owner != Owner::Empire(empire_entity) {
        debug!(
            "drain_ai_ship_commands: ship {:?} no longer owned by empire {:?}",
            ship_entity, empire_entity
        );
        commands_buf
            .entity(ship_entity)
            .remove::<crate::ai::assignments::PendingAssignment>();
        return;
    }
    if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
        debug!(
            "drain_ai_ship_commands: ship {:?} not idle at arrival (queue_len={})",
            ship_entity,
            queue.commands.len(),
        );
        commands_buf
            .entity(ship_entity)
            .remove::<crate::ai::assignments::PendingAssignment>();
        return;
    }

    writer.write(SurveyRequested {
        command_id: next_cmd_id.allocate(),
        ship: ship_entity,
        target_system,
        issued_at: now,
    });

    info!(
        "drain_ai_ship_commands: survey_system delivered to ship {:?} → system {:?} for empire {:?}",
        ship_entity, target_system, empire_entity
    );
}

/// #468 PR-2/PR-3: Apply a matured `colonize_system` /
/// `colonize_planet` PendingAiShipCommand.
///
/// Mirrors `apply_survey_to_ship`: validate the ship is still eligible
/// (owned by the issuer, in-system, idle) and write the
/// `ColonizeRequested` message. The `PendingAssignment` marker was
/// inserted at outbox-spawn time with `AssignmentKind::Colonize`; on
/// every reject branch we strip the marker so the ship is not
/// permanently excluded from future AI colonize dispatches (the dedup
/// scan in `npc_decision.rs` filters by `PendingAssignment`).
///
/// `target_planet` is forwarded into the event:
///   * `None` for `colonize_system` — the consumer-side colonization
///     handler picks the best planet in the target system. Same
///     convention the legacy `handle_colonize_system` used.
///   * `Some(planet)` for `colonize_planet` — the settlement handler
///     targets that exact planet. The two kinds share this apply
///     function (and the same `AssignmentKind::Colonize` marker
///     family); only the `planet` field differs.
fn apply_colonize_to_ship(
    ship_entity: Entity,
    empire_entity: Entity,
    target_system: Entity,
    target_planet: Option<Entity>,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    colonize_writer: Option<&mut MessageWriter<ColonizeRequested>>,
    next_cmd_id: &mut NextCommandId,
    commands_buf: &mut Commands,
    now: i64,
) {
    let Some(writer) = colonize_writer else {
        warn!("drain_ai_ship_commands: ColonizeRequested writer unavailable");
        commands_buf
            .entity(ship_entity)
            .remove::<crate::ai::assignments::PendingAssignment>();
        return;
    };

    let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
        debug!(
            "drain_ai_ship_commands: colonize ship {:?} despawned before arrival",
            ship_entity
        );
        return;
    };
    if ship.owner != Owner::Empire(empire_entity) {
        debug!(
            "drain_ai_ship_commands: colonize ship {:?} no longer owned by empire {:?}",
            ship_entity, empire_entity
        );
        commands_buf
            .entity(ship_entity)
            .remove::<crate::ai::assignments::PendingAssignment>();
        return;
    }
    if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
        debug!(
            "drain_ai_ship_commands: colonize ship {:?} not idle at arrival (queue_len={})",
            ship_entity,
            queue.commands.len(),
        );
        commands_buf
            .entity(ship_entity)
            .remove::<crate::ai::assignments::PendingAssignment>();
        return;
    }

    writer.write(ColonizeRequested {
        command_id: next_cmd_id.allocate(),
        ship: ship_entity,
        target_system,
        planet: target_planet,
        issued_at: now,
    });

    info!(
        "drain_ai_ship_commands: colonize delivered to ship {:?} → system {:?} planet {:?} for empire {:?}",
        ship_entity, target_system, target_planet, empire_entity
    );
}

/// #468 PR-2/PR-3: shared movement-order delivery for `reposition`,
/// `blockade`, and `attack_target`. All three are pure
/// MoveRequested writes after the idle / owned / not-already-there
/// gates, so the body is the same; the `cmd_name` argument keeps the
/// `info!` line distinguishable in logs. (PR-3 HIGH A fold-in: the
/// per-kind 1-line wrappers `apply_reposition_to_ship` /
/// `apply_blockade_to_ship` were inlined at the dispatch site.)
fn apply_move_to_ship(
    cmd_name: &str,
    ship_entity: Entity,
    empire_entity: Entity,
    target_system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    move_writer: Option<&mut MessageWriter<MoveRequested>>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let Some(writer) = move_writer else {
        warn!(
            "drain_ai_ship_commands: MoveRequested writer unavailable for {}",
            cmd_name
        );
        return;
    };

    let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
        debug!(
            "drain_ai_ship_commands: {} ship {:?} despawned before arrival",
            cmd_name, ship_entity
        );
        return;
    };
    if ship.owner != Owner::Empire(empire_entity) {
        debug!(
            "drain_ai_ship_commands: {} ship {:?} no longer owned by empire {:?}",
            cmd_name, ship_entity, empire_entity
        );
        return;
    }
    if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
        debug!(
            "drain_ai_ship_commands: {} ship {:?} not idle at arrival (queue_len={})",
            cmd_name,
            ship_entity,
            queue.commands.len(),
        );
        return;
    }
    if let ShipState::InSystem { system } = state {
        if *system == target_system {
            debug!(
                "drain_ai_ship_commands: {} ship {:?} already at target {:?}; skipping",
                cmd_name, ship_entity, target_system
            );
            return;
        }
    }

    writer.write(MoveRequested {
        command_id: next_cmd_id.allocate(),
        ship: ship_entity,
        target: target_system,
        issued_at: now,
    });

    info!(
        "drain_ai_ship_commands: {} delivered to ship {:?} → system {:?} for empire {:?}",
        cmd_name, ship_entity, target_system, empire_entity
    );
}

/// #468 PR-3: Apply a matured `move_ruler` PendingAiShipCommand.
///
/// Boarding contract: the dispatcher selected a transport ship at the
/// Ruler's current system (so Ruler→ship light delay ≈ 0). At
/// maturity:
///   * Validate the ship is still eligible (owned, mobile, in-system,
///     idle, not already carrying the Ruler);
///   * Validate the Ruler is still stationed (not already aboard);
///   * Push the `(ruler, ship, target_system)` triple into
///     `PendingRulerBoarding` for `process_ruler_boarding` to apply
///     (mutating `&mut Ship.ruler_aboard` + inserting `AboardShip`);
///   * Emit `MoveRequested` so the ship transits to `target_system`.
///
/// No `PendingAssignment` marker (boarding is a movement-class
/// order). Reject paths early-return without marker bookkeeping —
/// the dispatcher would re-emit on the next decision tick if the AI
/// still wants the Ruler moved.
#[allow(clippy::too_many_arguments)]
fn apply_move_ruler_to_ship(
    ship_entity: Entity,
    empire_entity: Entity,
    target_system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empire_rulers: &Query<&EmpireRuler, With<Empire>>,
    move_writer: Option<&mut MessageWriter<MoveRequested>>,
    pending_boarding: &mut PendingRulerBoarding,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let Some(writer) = move_writer else {
        warn!("drain_ai_ship_commands: MoveRequested writer unavailable for move_ruler");
        return;
    };

    let Ok(empire_ruler) = empire_rulers.get(empire_entity) else {
        debug!(
            "drain_ai_ship_commands: move_ruler empire {:?} has no EmpireRuler",
            empire_entity
        );
        return;
    };
    let ruler_entity = empire_ruler.0;

    let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
        debug!(
            "drain_ai_ship_commands: move_ruler ship {:?} despawned before arrival",
            ship_entity
        );
        return;
    };
    if ship.owner != Owner::Empire(empire_entity) {
        debug!(
            "drain_ai_ship_commands: move_ruler ship {:?} no longer owned by empire {:?}",
            ship_entity, empire_entity
        );
        return;
    }
    if ship.ruler_aboard {
        debug!(
            "drain_ai_ship_commands: move_ruler ship {:?} already carrying a Ruler",
            ship_entity
        );
        return;
    }
    if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
        debug!(
            "drain_ai_ship_commands: move_ruler ship {:?} not idle at arrival (queue_len={})",
            ship_entity,
            queue.commands.len(),
        );
        return;
    }
    if let ShipState::InSystem { system } = state {
        if *system == target_system {
            debug!(
                "drain_ai_ship_commands: move_ruler ship {:?} already at target {:?}; skipping",
                ship_entity, target_system
            );
            return;
        }
    }

    pending_boarding
        .requests
        .push((ruler_entity, ship_entity, target_system));

    writer.write(MoveRequested {
        command_id: next_cmd_id.allocate(),
        ship: ship_entity,
        target: target_system,
        issued_at: now,
    });

    info!(
        "drain_ai_ship_commands: move_ruler boarding ruler {:?} onto ship {:?} → system {:?} for empire {:?}",
        ruler_entity, ship_entity, target_system, empire_entity
    );
}

/// #468 PR-3: Apply a matured `load_deliverable` PendingAiShipCommand.
///
/// Bridges to the existing `LoadDeliverableRequested` ECS event the
/// same way the legacy `handle_load_deliverable` did. The
/// `stockpile_index` defaults to 0 — the previous handler accepted an
/// optional override via the command's `stockpile_index` param; PR-3
/// drops the override (the AI emitters never set it and the holder
/// carries only `target_system` + `ship`). When a future policy
/// needs an explicit index, extend `PendingAiShipCommand` with the
/// field (additive — no save-format impact since the holder isn't
/// persisted).
///
/// No `PendingAssignment` marker — `load_deliverable` is a per-tick
/// idempotent cargo order. The original handler validated owner and
/// stockpile presence; PR-3 keeps the owner check.
///
/// #468 PR-3 NICE-TO-FIX #7 fold-in: empty-stockpile dedup gate.
/// Previously the AI re-emitted each tick spamming logs until either
/// the stockpile filled or the underlying metric flipped — the
/// downstream handler always Rejected with "stockpile index out of
/// range". Gating here drops the emit so the policy can re-emit next
/// tick without producing reject events, mirroring the legacy
/// `handle_load_deliverable` precheck.
fn apply_load_deliverable_to_ship(
    ship_entity: Entity,
    empire_entity: Entity,
    target_system: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    stockpiles: &Query<&crate::colony::DeliverableStockpile>,
    load_writer: Option<&mut MessageWriter<LoadDeliverableRequested>>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let Some(writer) = load_writer else {
        warn!("drain_ai_ship_commands: LoadDeliverableRequested writer unavailable");
        return;
    };

    let Ok((_, ship, _, _)) = ships.get(ship_entity) else {
        debug!(
            "drain_ai_ship_commands: load_deliverable ship {:?} despawned before arrival",
            ship_entity
        );
        return;
    };
    if ship.owner != Owner::Empire(empire_entity) {
        debug!(
            "drain_ai_ship_commands: load_deliverable ship {:?} not owned by empire {:?}",
            ship_entity, empire_entity
        );
        return;
    }

    // NICE-TO-FIX #7: skip the emit when the target system has no
    // DeliverableStockpile component or the stockpile is empty at
    // stockpile_index = 0. The downstream handler validates and
    // Rejects in this case but each Reject costs an event +
    // CommandExecuted write; the AI would re-emit each tick.
    let stockpile_empty = match stockpiles.get(target_system) {
        Ok(sp) => sp.items.is_empty(),
        Err(_) => true,
    };
    if stockpile_empty {
        debug!(
            "drain_ai_ship_commands: load_deliverable skipped — system {:?} has no stockpile items \
             (would Reject downstream)",
            target_system
        );
        return;
    }

    writer.write(LoadDeliverableRequested {
        command_id: next_cmd_id.allocate(),
        ship: ship_entity,
        system: target_system,
        stockpile_index: 0,
        issued_at: now,
    });

    info!(
        "drain_ai_ship_commands: load_deliverable delivered to ship {:?} → system {:?} for empire {:?}",
        ship_entity, target_system, empire_entity
    );
}

/// #468 PR-3: Apply a matured `unload_deliverable` PendingAiShipCommand.
///
/// Bridges to the existing `DeployDeliverableRequested` ECS event.
/// The deploy event takes a `[f64; 3]` position that
/// `handle_deploy_deliverable_requested` validates against the ship's
/// actual `Position` (within DEPLOY_POSITION_EPSILON). We pass the
/// ship's current `Position` from the dedicated `ship_positions`
/// query so the equality check passes — the legacy handler did the
/// same.
///
/// `target_system` in the holder is a sentinel (= ship's `home_port`
/// set at dispatch time) — unload has no meaningful system target, so
/// we ignore it here. The dedup scan in `npc_decision.rs` also skips
/// unload kinds, so the sentinel doesn't pollute anything.
///
/// #468 PR-3 NICE-TO-FIX #5 / #6 fold-in: precheck the ship's cargo
/// (item_index = 0 must be present) and ShipState (InSystem |
/// Loitering only — InFTL / SubLight / Boarding etc. would cause
/// downstream defer + re-inject log noise). The legacy
/// `handle_unload_deliverable` had a cargo-index sanity check; PR-3's
/// migration dropped it and the AI was spamming logs each tick until
/// the metric flipped.
fn apply_unload_deliverable_to_ship(
    ship_entity: Entity,
    empire_entity: Entity,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    ship_positions: &Query<&crate::components::Position, With<Ship>>,
    ship_cargos: &Query<&crate::ship::Cargo, With<Ship>>,
    deploy_writer: Option<&mut MessageWriter<DeployDeliverableRequested>>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let Some(writer) = deploy_writer else {
        warn!("drain_ai_ship_commands: DeployDeliverableRequested writer unavailable");
        return;
    };

    let Ok((_, ship, state, _)) = ships.get(ship_entity) else {
        debug!(
            "drain_ai_ship_commands: unload_deliverable ship {:?} despawned before arrival",
            ship_entity
        );
        return;
    };
    if ship.owner != Owner::Empire(empire_entity) {
        debug!(
            "drain_ai_ship_commands: unload_deliverable ship {:?} not owned by empire {:?}",
            ship_entity, empire_entity
        );
        return;
    }

    // NICE-TO-FIX #6: deploy only makes sense from a stationary state.
    // A ship in InFTL / SubLight / Boarding triggers downstream
    // defer + re-inject; the AI ends up spamming the same command for
    // the entire travel window. Cheap to gate here.
    if !matches!(
        state,
        ShipState::InSystem { .. } | ShipState::Loitering { .. }
    ) {
        debug!(
            "drain_ai_ship_commands: unload_deliverable skipped — ship {:?} not InSystem/Loitering \
             (state would trigger downstream defer + re-inject)",
            ship_entity
        );
        return;
    }

    // NICE-TO-FIX #5: cargo precheck — item_index = 0 must exist
    // before we ask the handler to deploy. The legacy
    // cargo-index sanity check used to do this; dropped during the
    // PR-3 migration. Without the gate the handler Rejects each tick
    // until the metric flips, generating log noise.
    let cargo_has_item = ship_cargos
        .get(ship_entity)
        .map(|c| c.items.first().is_some())
        .unwrap_or(false);
    if !cargo_has_item {
        debug!(
            "drain_ai_ship_commands: unload_deliverable skipped — ship {:?} has no cargo item at \
             index 0 (would Reject downstream)",
            ship_entity
        );
        return;
    }

    let position = ship_positions
        .get(ship_entity)
        .map(|p| p.as_array())
        .unwrap_or([0.0, 0.0, 0.0]);

    writer.write(DeployDeliverableRequested {
        command_id: next_cmd_id.allocate(),
        ship: ship_entity,
        position,
        item_index: 0,
        issued_at: now,
    });

    info!(
        "drain_ai_ship_commands: unload_deliverable delivered to ship {:?} at {:?} for empire {:?}",
        ship_entity, position, empire_entity
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::convert::to_ai_faction;
    use crate::ai::plugin::AiBusResource;
    use crate::ai::schema;
    use crate::ai::schema::ids::command as cmd_ids;
    use crate::colony::BuildKind;
    use crate::components::Position;
    use crate::technology::TechId;
    use crate::time_system::{GameClock, GameSpeed};
    use macrocosmo_ai::{Command, CommandValue, WarningMode};
    use macrocosmo_core::amount::Amt;

    #[derive(Resource, Reflect)]
    #[reflect(Resource)]
    struct MoveCount(usize);

    fn count_moves(mut reader: MessageReader<MoveRequested>, mut count: ResMut<MoveCount>) {
        for _msg in reader.read() {
            count.0 += 1;
        }
    }

    /// Minimal app with AI bus and clock for command consumer tests.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.init_resource::<NextCommandId>();
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        app.init_resource::<PendingRulerBoarding>();
        app.add_message::<MoveRequested>();
        // #446 / #468 PR-3: the AI drain-side writers tap these
        // existing pre-Phase-2 events. Register them up front so the
        // SystemParam writers in `DrainShipCommandWriters` and
        // `CommandStamp` resolve cleanly even in the minimal headless
        // test app.
        app.add_message::<LoadDeliverableRequested>();
        app.add_message::<DeployDeliverableRequested>();
        app.add_message::<ColonizeRequested>();
        app.add_message::<SurveyRequested>();
        app.add_systems(Startup, schema::declare_all);
        app.update();
        app
    }

    /// #468 PR-3: `attack_target` now flows through the per-ship
    /// `PendingAiShipCommand` pipeline. We spawn a matured holder
    /// directly and assert `drain_ai_ship_commands` emits one
    /// `MoveRequested` for the chosen ship — same wire shape as the
    /// legacy `handle_attack_target` path that this test replaced.
    #[test]
    fn attack_target_dispatches_idle_ships() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let origin_sys = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let target_sys = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();

        let ship_entity = world
            .spawn((
                Ship {
                    name: "NPC Scout".into(),
                    design_id: "scout".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: origin_sys,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: origin_sys },
                CommandQueue::default(),
            ))
            .id();

        world.spawn(PendingAiShipCommand {
            kind: cmd_ids::attack_target(),
            target_system: target_sys,
            target_planet: None,
            ship: ship_entity,
            issuer_empire: empire_entity,
            sent_at: 0,
            arrives_at: 0,
        });

        app.insert_resource(MoveCount(0));
        app.add_systems(Update, (drain_ai_ship_commands, count_moves).chain());
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(
            count, 1,
            "attack_target should emit 1 MoveRequested through drain_ai_ship_commands"
        );
    }

    /// #468 PR-3: `attack_target` apply path drops ships whose owner
    /// changed between dispatch and arrival (the dispatcher trusted
    /// the policy at emit time, but ownership can change mid-courier).
    #[test]
    fn attack_target_skips_ships_not_owned_by_faction() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_a = world
            .spawn((
                Empire {
                    name: "Empire A".into(),
                },
                Faction::new("empire_a", "Empire A"),
            ))
            .id();

        let empire_b = world
            .spawn((
                Empire {
                    name: "Empire B".into(),
                },
                Faction::new("empire_b", "Empire B"),
            ))
            .id();

        let origin = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let target = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();

        // Ship owned by empire_b — apply path must drop the move.
        let b_ship = world
            .spawn((
                Ship {
                    name: "B's Ship".into(),
                    design_id: "scout".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_b),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: origin,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: origin },
                CommandQueue::default(),
            ))
            .id();

        // Holder targeted at empire_a's faction with empire_b's ship.
        world.spawn(PendingAiShipCommand {
            kind: cmd_ids::attack_target(),
            target_system: target,
            target_planet: None,
            ship: b_ship,
            issuer_empire: empire_a,
            sent_at: 0,
            arrives_at: 0,
        });

        app.insert_resource(MoveCount(0));
        app.add_systems(Update, (drain_ai_ship_commands, count_moves).chain());
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(
            count, 0,
            "empire_b's ship should not be dispatched by empire_a's attack_target holder"
        );
    }

    /// Helper: create a minimal ShipDesignRegistry with a combat design.
    fn test_design_registry() -> ShipDesignRegistry {
        use crate::ship_design::ShipDesignDefinition;
        let mut registry = ShipDesignRegistry::default();
        registry.insert(ShipDesignDefinition {
            id: "corvette_mk1".into(),
            name: "Corvette Mk1".into(),
            description: String::new(),
            hull_id: "corvette".into(),
            modules: vec![],
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::new(0, 500),
            build_cost_minerals: Amt::units(100),
            build_cost_energy: Amt::units(50),
            build_time: 30,
            hp: 100.0,
            sublight_speed: 0.1,
            ftl_range: 5.0,
            revision: 0,
            is_direct_buildable: true,
        });
        registry
    }

    /// Helper: create a BuildingRegistry with a test mine building.
    fn test_building_registry() -> BuildingRegistry {
        use crate::scripting::building_api::BuildingDefinition;
        use std::collections::HashMap;
        let mut registry = BuildingRegistry::default();
        registry.insert(BuildingDefinition {
            id: "mine".into(),
            name: "Mine".into(),
            description: String::new(),
            minerals_cost: Amt::units(50),
            energy_cost: Amt::units(10),
            build_time: 15,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::units(5),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
            colony_slots: None,
        });
        registry
    }

    #[test]
    fn build_ship_queues_order_at_shipyard_system() {
        use crate::scripting::building_api::BuildingDefinition;
        use std::collections::HashMap;

        let mut app = test_app();
        // Insert design + building registries
        app.insert_resource(test_design_registry());

        // Building registry with shipyard modifier
        let mut breg = BuildingRegistry::default();
        breg.insert(BuildingDefinition {
            id: "shipyard".into(),
            name: "Shipyard".into(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 30,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: vec![crate::modifier::ParsedModifier {
                target: "system.shipyard_build_parallel_slots".into(),
                base_add: 1.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            is_system_building: true,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: Some("station_shipyard_v1".into()),
            colony_slots: None,
        });
        app.insert_resource(breg);

        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        // SystemModifiers with shipyard_build_parallel_slots seeded so has_shipyard check passes.
        let mut sys_mods = crate::galaxy::SystemModifiers::default();
        sys_mods
            .shipyard_build_parallel_slots
            .push_modifier(crate::modifier::Modifier {
                id: "test_shipyard".into(),
                label: "Test Shipyard".into(),
                base_add: macrocosmo_core::amount::SignedAmt::units(1),
                multiplier: macrocosmo_core::amount::SignedAmt::ZERO,
                add: macrocosmo_core::amount::SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: None,
            });

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty {
                    owner: Some(Owner::Empire(empire_entity)),
                    control_score: 1.0,
                },
                sys_mods,
            ))
            .id();

        // #470: BuildQueue lives on Colony, not StarSystem. Spawn a planet
        // + colony so `queue_ship_at_shipyard` can find a host colony.
        let planet_entity = world
            .spawn((crate::galaxy::Planet {
                name: "Home I".into(),
                system: sys_entity,
                planet_type: "terrestrial".into(),
            },))
            .id();
        let colony_entity = world
            .spawn((
                Colony {
                    planet: planet_entity,
                    growth_rate: 0.0,
                },
                BuildQueue::default(),
                crate::faction::FactionOwner(empire_entity),
            ))
            .id();

        // Spawn a station ship with shipyard design at that system
        world.spawn((
            Ship {
                name: "Shipyard Station".into(),
                design_id: "station_shipyard_v1".into(),
                hull_id: "station".into(),
                modules: vec![],
                owner: Owner::Empire(empire_entity),
                sublight_speed: 0.0,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: sys_entity,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: sys_entity },
            SlotAssignment(0),
            CommandQueue::default(),
        ));

        // Emit build_ship command
        let cmd = Command::new(cmd_ids::build_ship(), faction_id, 10)
            .with_param("design_id", CommandValue::Str("corvette_mk1".into()));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        // Check that a build order was added to the colony's BuildQueue.
        let queue = app.world().get::<BuildQueue>(colony_entity).unwrap();
        assert_eq!(queue.queue.len(), 1, "should have 1 build order");
        assert_eq!(queue.queue[0].design_id, "corvette_mk1");
    }

    #[test]
    fn research_focus_sets_active_research() {
        use crate::technology::{TechCost, Technology};

        let mut app = test_app();
        let world = app.world_mut();

        let tech_tree = TechTree::from_vec(vec![Technology {
            id: TechId("test_tech".into()),
            name: "Test Tech".into(),
            description: String::new(),
            branch: "test".into(),
            cost: TechCost {
                research: Amt::units(100),
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
            },
            prerequisites: vec![],
            dangerous: false,
        }]);

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
                tech_tree,
                ResearchQueue::default(),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let cmd = Command::new(cmd_ids::research_focus(), faction_id, 10)
            .with_param("tech_id", CommandValue::Str("test_tech".into()));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        let rq = app.world().get::<ResearchQueue>(empire_entity).unwrap();
        assert_eq!(
            rq.current,
            Some(TechId("test_tech".into())),
            "should be researching test_tech"
        );
    }

    #[test]
    fn research_focus_auto_picks_available_tech() {
        use crate::technology::{TechCost, Technology};

        let mut app = test_app();
        let world = app.world_mut();

        let tech_tree = TechTree::from_vec(vec![Technology {
            id: TechId("auto_pick_tech".into()),
            name: "Auto Pick".into(),
            description: String::new(),
            branch: "test".into(),
            cost: TechCost {
                research: Amt::units(100),
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
            },
            prerequisites: vec![],
            dangerous: false,
        }]);

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
                tech_tree,
                ResearchQueue::default(),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        // No tech_id param — should auto-pick
        let cmd = Command::new(cmd_ids::research_focus(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        let rq = app.world().get::<ResearchQueue>(empire_entity).unwrap();
        assert!(rq.current.is_some(), "should have auto-picked a tech");
    }

    #[test]
    fn build_structure_queues_building_at_colony() {
        let mut app = test_app();
        app.insert_resource(test_building_registry());

        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty {
                    owner: Some(Owner::Empire(empire_entity)),
                    control_score: 1.0,
                },
            ))
            .id();

        let planet_entity = world
            .spawn(Planet {
                name: "Test Planet".into(),
                planet_type: "terran".into(),
                system: sys_entity,
            })
            .id();

        world.spawn((
            Colony {
                planet: planet_entity,
                growth_rate: 0.01,
            },
            Buildings {
                slots: vec![None, None, None],
            },
            BuildingQueue::default(),
        ));

        let cmd = Command::new(cmd_ids::build_structure(), faction_id, 10)
            .with_param("building_id", CommandValue::Str("mine".into()));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        // Verify a building order was queued
        let mut found = false;
        for (_, _, _, bq) in app
            .world_mut()
            .query::<(Entity, &Colony, &Buildings, &BuildingQueue)>()
            .iter(app.world())
        {
            if !bq.queue.is_empty() {
                assert_eq!(bq.queue[0].building_id.as_str(), "mine");
                assert_eq!(bq.queue[0].target_slot, 0);
                found = true;
            }
        }
        assert!(found, "should have queued a building order");
    }

    /// #468 PR-2: `reposition` is dispatched through the per-ship
    /// `PendingAiShipCommand` pipeline. We construct a matured holder
    /// directly and assert `drain_ai_ship_commands` emits one
    /// `MoveRequested` — same wire shape as the legacy
    /// `handle_reposition` path that this test replaced.
    #[test]
    fn reposition_dispatches_ships() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let origin = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();
        let target = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();
        let ship_entity = world
            .spawn((
                Ship {
                    name: "NPC Ship".into(),
                    design_id: "corvette".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: origin,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: origin },
                CommandQueue::default(),
            ))
            .id();

        // Spawn a matured holder (arrives_at = sent_at = 0) so the very
        // next drain pass picks it up.
        world.spawn(PendingAiShipCommand {
            kind: cmd_ids::reposition(),
            target_system: target,
            target_planet: None,
            ship: ship_entity,
            issuer_empire: empire_entity,
            sent_at: 0,
            arrives_at: 0,
        });

        app.insert_resource(MoveCount(0));
        app.add_systems(Update, (drain_ai_ship_commands, count_moves).chain());
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(
            count, 1,
            "reposition should emit 1 MoveRequested through drain_ai_ship_commands"
        );
    }

    /// #468 PR-2: same shape as `reposition_dispatches_ships` but for
    /// the `blockade` kind. Both share `apply_move_to_ship` underneath.
    #[test]
    fn blockade_dispatches_ships() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let origin = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();
        let target = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();
        let ship_entity = world
            .spawn((
                Ship {
                    name: "NPC Ship".into(),
                    design_id: "corvette".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: origin,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: origin },
                CommandQueue::default(),
            ))
            .id();

        world.spawn(PendingAiShipCommand {
            kind: cmd_ids::blockade(),
            target_system: target,
            target_planet: None,
            ship: ship_entity,
            issuer_empire: empire_entity,
            sent_at: 0,
            arrives_at: 0,
        });

        app.insert_resource(MoveCount(0));
        app.add_systems(Update, (drain_ai_ship_commands, count_moves).chain());
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(
            count, 1,
            "blockade should emit 1 MoveRequested through drain_ai_ship_commands"
        );
    }

    #[test]
    fn fortify_system_auto_picks_combat_design() {
        use crate::scripting::building_api::BuildingDefinition;
        use std::collections::HashMap;

        let mut app = test_app();
        app.insert_resource(test_design_registry());

        // Building registry with shipyard modifier
        let mut breg = BuildingRegistry::default();
        breg.insert(BuildingDefinition {
            id: "shipyard".into(),
            name: "Shipyard".into(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 30,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: vec![crate::modifier::ParsedModifier {
                target: "system.shipyard_build_parallel_slots".into(),
                base_add: 1.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            is_system_building: true,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: Some("station_shipyard_v1".into()),
            colony_slots: None,
        });
        app.insert_resource(breg);

        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let mut sys_mods = crate::galaxy::SystemModifiers::default();
        sys_mods
            .shipyard_build_parallel_slots
            .push_modifier(crate::modifier::Modifier {
                id: "test_shipyard".into(),
                label: "Test Shipyard".into(),
                base_add: macrocosmo_core::amount::SignedAmt::units(1),
                multiplier: macrocosmo_core::amount::SignedAmt::ZERO,
                add: macrocosmo_core::amount::SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: None,
            });

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty {
                    owner: Some(Owner::Empire(empire_entity)),
                    control_score: 1.0,
                },
                sys_mods,
            ))
            .id();

        // #470: BuildQueue lives on Colony, not StarSystem.
        let planet_entity = world
            .spawn((crate::galaxy::Planet {
                name: "Home I".into(),
                system: sys_entity,
                planet_type: "terrestrial".into(),
            },))
            .id();
        let colony_entity = world
            .spawn((
                Colony {
                    planet: planet_entity,
                    growth_rate: 0.0,
                },
                BuildQueue::default(),
                crate::faction::FactionOwner(empire_entity),
            ))
            .id();

        // Shipyard station ship
        world.spawn((
            Ship {
                name: "Shipyard Station".into(),
                design_id: "station_shipyard_v1".into(),
                hull_id: "station".into(),
                modules: vec![],
                owner: Owner::Empire(empire_entity),
                sublight_speed: 0.0,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: sys_entity,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: sys_entity },
            SlotAssignment(0),
            CommandQueue::default(),
        ));

        // No design_id param — should auto-pick combat design
        let cmd = Command::new(cmd_ids::fortify_system(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        let queue = app.world().get::<BuildQueue>(colony_entity).unwrap();
        assert_eq!(queue.queue.len(), 1, "fortify should queue 1 ship");
        assert_eq!(queue.queue[0].design_id, "corvette_mk1");
    }

    // ── retreat tests ────────────────────────────────────────────────

    /// Collect (ship, target) pairs from MoveRequested messages.
    #[derive(Resource, Default, Reflect)]
    #[reflect(Resource)]
    struct MoveTargets(Vec<(Entity, Entity)>);

    fn collect_move_targets(
        mut reader: MessageReader<MoveRequested>,
        mut targets: ResMut<MoveTargets>,
    ) {
        for msg in reader.read() {
            targets.0.push((msg.ship, msg.target));
        }
    }

    /// Helper: spawn a ship owned by `empire` at `system`.
    fn spawn_ship_at(world: &mut World, empire: Entity, system: Entity, name: &str) -> Entity {
        world
            .spawn((
                Ship {
                    name: name.into(),
                    design_id: "scout".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: system,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system },
                CommandQueue::default(),
            ))
            .id()
    }

    /// Helper: spawn a star system with position and sovereignty.
    fn spawn_system_with_sov(
        world: &mut World,
        name: &str,
        pos: [f64; 3],
        owner: Option<Entity>,
    ) -> Entity {
        let owner = owner.map(|e| Owner::Empire(e));
        world
            .spawn((
                StarSystem {
                    name: name.into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from(pos),
                Sovereignty {
                    owner,
                    control_score: 1.0,
                },
            ))
            .id()
    }

    #[test]
    fn retreat_picks_nearest_safe_system() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((Empire { name: "E".into() }, Faction::new("e", "E")))
            .id();

        // Hostile system where ship is located (at origin).
        let hostile_sys = spawn_system_with_sov(world, "Hostile", [0.0, 0.0, 0.0], Some(empire));
        // Near safe system at distance 5.
        let near_safe = spawn_system_with_sov(world, "NearSafe", [3.0, 4.0, 0.0], Some(empire));
        // Far safe system at distance 13.
        let _far_safe = spawn_system_with_sov(world, "FarSafe", [12.0, 5.0, 0.0], Some(empire));

        // Mark hostile_sys as having hostile presence.
        world.spawn((Hostile, AtSystem(hostile_sys)));

        let ship = spawn_ship_at(world, empire, hostile_sys, "Scout");

        // Issue retreat command.
        let faction_id = to_ai_faction(empire);
        let cmd = Command::new(cmd_ids::retreat(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.init_resource::<MoveTargets>();
        app.add_systems(Update, (drain_ai_commands, collect_move_targets).chain());
        app.update();

        let targets = app.world().resource::<MoveTargets>();
        assert_eq!(targets.0.len(), 1, "should retreat 1 ship");
        assert_eq!(
            targets.0[0],
            (ship, near_safe),
            "should pick nearest safe system"
        );
    }

    #[test]
    fn retreat_distributes_ships_to_nearest_rally() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((Empire { name: "E".into() }, Faction::new("e", "E")))
            .id();

        // Two hostile systems at different locations.
        let hostile_a = spawn_system_with_sov(world, "HostileA", [0.0, 0.0, 0.0], Some(empire));
        let hostile_b = spawn_system_with_sov(world, "HostileB", [20.0, 0.0, 0.0], Some(empire));

        // Two safe systems: one near hostile_a, one near hostile_b.
        let safe_left = spawn_system_with_sov(world, "SafeLeft", [-5.0, 0.0, 0.0], Some(empire));
        let safe_right = spawn_system_with_sov(world, "SafeRight", [25.0, 0.0, 0.0], Some(empire));

        world.spawn((Hostile, AtSystem(hostile_a)));
        world.spawn((Hostile, AtSystem(hostile_b)));

        let ship_a = spawn_ship_at(world, empire, hostile_a, "ShipA");
        let ship_b = spawn_ship_at(world, empire, hostile_b, "ShipB");

        let faction_id = to_ai_faction(empire);
        let cmd = Command::new(cmd_ids::retreat(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.init_resource::<MoveTargets>();
        app.add_systems(Update, (drain_ai_commands, collect_move_targets).chain());
        app.update();

        let targets = app.world().resource::<MoveTargets>();
        assert_eq!(targets.0.len(), 2, "should retreat 2 ships");

        // Each ship goes to its nearest safe system.
        let ship_a_target = targets
            .0
            .iter()
            .find(|(s, _)| *s == ship_a)
            .map(|(_, t)| *t);
        let ship_b_target = targets
            .0
            .iter()
            .find(|(s, _)| *s == ship_b)
            .map(|(_, t)| *t);
        assert_eq!(
            ship_a_target,
            Some(safe_left),
            "ship_a should go to safe_left"
        );
        assert_eq!(
            ship_b_target,
            Some(safe_right),
            "ship_b should go to safe_right"
        );
    }

    #[test]
    fn retreat_skips_ships_already_in_transit() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((Empire { name: "E".into() }, Faction::new("e", "E")))
            .id();

        let hostile_sys = spawn_system_with_sov(world, "Hostile", [0.0, 0.0, 0.0], Some(empire));
        let _safe_sys = spawn_system_with_sov(world, "Safe", [5.0, 0.0, 0.0], Some(empire));

        world.spawn((Hostile, AtSystem(hostile_sys)));

        // Ship already in FTL transit — should not be re-commanded.
        world.spawn((
            Ship {
                name: "In Transit".into(),
                design_id: "scout".into(),
                hull_id: "corvette".into(),
                modules: vec![],
                owner: Owner::Empire(empire),
                sublight_speed: 0.1,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: hostile_sys,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InFTL {
                origin_system: hostile_sys,
                destination_system: _safe_sys,
                departed_at: 5,
                arrival_at: 15,
            },
            CommandQueue::default(),
        ));

        let faction_id = to_ai_faction(empire);
        let cmd = Command::new(cmd_ids::retreat(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.init_resource::<MoveTargets>();
        app.add_systems(Update, (drain_ai_commands, collect_move_targets).chain());
        app.update();

        let targets = app.world().resource::<MoveTargets>();
        assert_eq!(
            targets.0.len(),
            0,
            "should not retreat ships already in transit"
        );
    }

    #[test]
    fn retreat_falls_back_to_hostile_owned_system() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((Empire { name: "E".into() }, Faction::new("e", "E")))
            .id();

        // All owned systems have hostiles.
        let hostile_a = spawn_system_with_sov(world, "HostileA", [0.0, 0.0, 0.0], Some(empire));
        let hostile_b = spawn_system_with_sov(world, "HostileB", [10.0, 0.0, 0.0], Some(empire));

        world.spawn((Hostile, AtSystem(hostile_a)));
        world.spawn((Hostile, AtSystem(hostile_b)));

        let ship = spawn_ship_at(world, empire, hostile_a, "Scout");

        let faction_id = to_ai_faction(empire);
        let cmd = Command::new(cmd_ids::retreat(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.init_resource::<MoveTargets>();
        app.add_systems(Update, (drain_ai_commands, collect_move_targets).chain());
        app.update();

        let targets = app.world().resource::<MoveTargets>();
        // Should still retreat — falls back to the nearest owned system (hostile_b,
        // since ship is already at hostile_a).
        assert_eq!(
            targets.0.len(),
            1,
            "should retreat even when all systems are hostile"
        );
        assert_eq!(
            targets.0[0],
            (ship, hostile_b),
            "should pick nearest owned system as fallback"
        );
    }

    // ── #446: deliverable family handlers ───────────────────────────────

    // The wrapped event types don't implement `Reflect` (they're plain
    // `Message` skeletons), so these collector resources stay
    // non-reflective — they only exist inside the test module.
    #[derive(Resource, Default)]
    struct LoadEvents(Vec<LoadDeliverableRequested>);

    #[derive(Resource, Default)]
    struct DeployEvents(Vec<DeployDeliverableRequested>);

    #[derive(Resource, Default)]
    struct ColonizeEvents(Vec<ColonizeRequested>);

    fn collect_load_events(
        mut reader: MessageReader<LoadDeliverableRequested>,
        mut events: ResMut<LoadEvents>,
    ) {
        for ev in reader.read() {
            events.0.push(ev.clone());
        }
    }

    fn collect_deploy_events(
        mut reader: MessageReader<DeployDeliverableRequested>,
        mut events: ResMut<DeployEvents>,
    ) {
        for ev in reader.read() {
            events.0.push(ev.clone());
        }
    }

    fn collect_colonize_events(
        mut reader: MessageReader<ColonizeRequested>,
        mut events: ResMut<ColonizeEvents>,
    ) {
        for ev in reader.read() {
            events.0.push(ev.clone());
        }
    }

    /// Helper: a minimal deliverable definition in the deliverable
    /// registry. Mirrors the shape used by `infrastructure_core` in the
    /// production Lua scripts (cost / build_time small enough that
    /// tests don't have to advance the queue).
    ///
    /// #532 F1: this previously returned a `ShipDesignRegistry` with a
    /// fake `ShipDesignDefinition` keyed on the deliverable id —
    /// masking the bug that the handler looked up the wrong registry.
    /// After F1 the handler resolves via `DeliverableRegistry`, so the
    /// helper inserts a real `DeliverableDefinition` instead.
    fn test_deliverable_registry() -> crate::deep_space::DeliverableRegistry {
        use crate::deep_space::{
            DeliverableMetadata, DeliverableRegistry, ResourceCost, StructureDefinition,
        };
        let mut registry = DeliverableRegistry::default();
        registry.insert(StructureDefinition {
            id: "infra_core".into(),
            name: "Infrastructure Core".into(),
            description: String::new(),
            max_hp: 1.0,
            energy_drain: Amt::ZERO,
            capabilities: std::collections::HashMap::new(),
            prerequisites: None,
            deliverable: Some(DeliverableMetadata {
                cost: ResourceCost {
                    minerals: Amt::units(20),
                    energy: Amt::units(10),
                },
                build_time: 5,
                cargo_size: 1,
                scrap_refund: 0.25,
                // These per-handler tests do not exercise the deploy /
                // spawn pipeline that consumes `spawns_as_ship`.
                spawns_as_ship: None,
            }),
            upgrade_to: Vec::new(),
            upgrade_from: None,
            on_built: None,
            on_upgraded: None,
        });
        registry
    }

    #[test]
    fn build_deliverable_queues_order_at_owned_system() {
        let mut app = test_app();
        app.insert_resource(test_deliverable_registry());

        let world = app.world_mut();
        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();
        let faction_id = to_ai_faction(empire_entity);

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty {
                    owner: Some(Owner::Empire(empire_entity)),
                    control_score: 1.0,
                },
            ))
            .id();

        // #470: BuildQueue (incl. Deliverable orders) lives on Colony.
        let planet_entity = world
            .spawn((crate::galaxy::Planet {
                name: "Home I".into(),
                system: sys_entity,
                planet_type: "terrestrial".into(),
            },))
            .id();
        let colony_entity = world
            .spawn((
                Colony {
                    planet: planet_entity,
                    growth_rate: 0.0,
                },
                BuildQueue::default(),
                crate::faction::FactionOwner(empire_entity),
            ))
            .id();

        let cmd = Command::new(cmd_ids::build_deliverable(), faction_id, 10)
            .with_param("definition_id", CommandValue::Str("infra_core".into()));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        let queue = app.world().get::<BuildQueue>(colony_entity).unwrap();
        assert_eq!(queue.queue.len(), 1, "should have queued 1 deliverable");
        assert_eq!(queue.queue[0].design_id, "infra_core");
        assert!(
            matches!(queue.queue[0].kind, BuildKind::Deliverable { .. }),
            "kind should be Deliverable, got {:?}",
            queue.queue[0].kind
        );
    }

    #[test]
    fn build_deliverable_dedups_same_definition_per_system() {
        let mut app = test_app();
        app.insert_resource(test_deliverable_registry());

        let world = app.world_mut();
        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();
        let faction_id = to_ai_faction(empire_entity);

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty {
                    owner: Some(Owner::Empire(empire_entity)),
                    control_score: 1.0,
                },
            ))
            .id();

        // #470: BuildQueue lives on Colony.
        let planet_entity = world
            .spawn((crate::galaxy::Planet {
                name: "Home I".into(),
                system: sys_entity,
                planet_type: "terrestrial".into(),
            },))
            .id();
        let colony_entity = world
            .spawn((
                Colony {
                    planet: planet_entity,
                    growth_rate: 0.0,
                },
                BuildQueue::default(),
                crate::faction::FactionOwner(empire_entity),
            ))
            .id();

        // Emit twice — second emit should be skipped by dedup.
        for _ in 0..2 {
            let cmd = Command::new(cmd_ids::build_deliverable(), faction_id.clone(), 10)
                .with_param("definition_id", CommandValue::Str("infra_core".into()));
            world.resource_mut::<AiBusResource>().0.emit_command(cmd);
        }

        app.add_systems(Update, drain_ai_commands);
        app.update();

        let queue = app.world().get::<BuildQueue>(colony_entity).unwrap();
        assert_eq!(
            queue.queue.len(),
            1,
            "second build_deliverable should be deduped"
        );
    }

    /// #468 PR-3: `load_deliverable` migrated to the per-ship
    /// `PendingAiShipCommand` pipeline. We spawn a matured holder
    /// directly and assert `drain_ai_ship_commands` emits one
    /// `LoadDeliverableRequested` event.
    #[test]
    fn load_deliverable_emits_event_with_explicit_index() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                // #468 PR-3 NICE-TO-FIX #7: precheck requires a
                // non-empty DeliverableStockpile on the target
                // system. Seed one so the dedup gate lets the emit
                // through.
                crate::colony::DeliverableStockpile {
                    items: vec![crate::ship::CargoItem::Deliverable {
                        definition_id: "test_item".into(),
                    }],
                },
            ))
            .id();

        let ship_entity = world
            .spawn((
                Ship {
                    name: "Courier".into(),
                    design_id: "courier".into(),
                    hull_id: "courier".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: sys_entity,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: sys_entity },
                CommandQueue::default(),
            ))
            .id();

        world.spawn(PendingAiShipCommand {
            kind: cmd_ids::load_deliverable(),
            target_system: sys_entity,
            target_planet: None,
            ship: ship_entity,
            issuer_empire: empire_entity,
            sent_at: 0,
            arrives_at: 0,
        });

        app.init_resource::<LoadEvents>();
        app.add_systems(
            Update,
            (drain_ai_ship_commands, collect_load_events).chain(),
        );
        app.update();

        let events = app.world().resource::<LoadEvents>();
        assert_eq!(events.0.len(), 1, "should emit 1 LoadDeliverableRequested");
        assert_eq!(events.0[0].ship, ship_entity);
        assert_eq!(events.0[0].system, sys_entity);
        assert_eq!(events.0[0].stockpile_index, 0);
    }

    /// #468 PR-3: `unload_deliverable` migrated to the per-ship
    /// pipeline. The dispatcher uses the ship's `home_port` as a
    /// stable sentinel for `target_system` (since unload has no
    /// meaningful system target); the apply path reads the ship's
    /// realtime `Position` for the event's deploy coordinates so the
    /// downstream `handle_deploy_deliverable_requested` position
    /// check passes.
    #[test]
    fn unload_deliverable_emits_deploy_event_at_ship_position() {
        use crate::ship::{Cargo, CargoItem};

        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([7.0, 8.0, 9.0]),
            ))
            .id();

        let ship_entity = world
            .spawn((
                Ship {
                    name: "Courier".into(),
                    design_id: "courier".into(),
                    hull_id: "courier".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: sys_entity,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: sys_entity },
                CommandQueue::default(),
                Position::from([7.0, 8.0, 9.0]),
                Cargo {
                    minerals: Amt::ZERO,
                    energy: Amt::ZERO,
                    items: vec![CargoItem::Deliverable {
                        definition_id: "infra_core".into(),
                    }],
                },
            ))
            .id();

        world.spawn(PendingAiShipCommand {
            kind: cmd_ids::unload_deliverable(),
            target_system: sys_entity, // sentinel — ignored by the apply path
            target_planet: None,
            ship: ship_entity,
            issuer_empire: empire_entity,
            sent_at: 0,
            arrives_at: 0,
        });

        app.init_resource::<DeployEvents>();
        app.add_systems(
            Update,
            (drain_ai_ship_commands, collect_deploy_events).chain(),
        );
        app.update();

        let events = app.world().resource::<DeployEvents>();
        assert_eq!(
            events.0.len(),
            1,
            "should emit 1 DeployDeliverableRequested"
        );
        assert_eq!(events.0[0].ship, ship_entity);
        assert_eq!(events.0[0].item_index, 0);
        assert_eq!(
            events.0[0].position,
            [7.0, 8.0, 9.0],
            "deploy position should mirror the ship's realtime Position",
        );
    }

    /// #468 PR-3: `colonize_planet` migrated to the per-ship
    /// pipeline; `apply_colonize_to_ship` honours
    /// `target_planet = Some(p)` and writes
    /// `ColonizeRequested.planet = Some(p)`. This pins the
    /// planet-target marker shape — vs `colonize_system` which
    /// writes `planet: None` and lets the handler pick.
    #[test]
    fn colonize_planet_emits_colonize_with_explicit_planet() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let planet_entity = world
            .spawn(Planet {
                name: "Test Planet".into(),
                planet_type: "terran".into(),
                system: sys_entity,
            })
            .id();

        let ship_entity = world
            .spawn((
                Ship {
                    name: "Colony Ship".into(),
                    design_id: "colony".into(),
                    hull_id: "colony".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: sys_entity,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: sys_entity },
                CommandQueue::default(),
            ))
            .id();

        world.spawn(PendingAiShipCommand {
            kind: cmd_ids::colonize_planet(),
            target_system: sys_entity,
            target_planet: Some(planet_entity),
            ship: ship_entity,
            issuer_empire: empire_entity,
            sent_at: 0,
            arrives_at: 0,
        });

        app.init_resource::<ColonizeEvents>();
        app.add_systems(
            Update,
            (drain_ai_ship_commands, collect_colonize_events).chain(),
        );
        app.update();

        let events = app.world().resource::<ColonizeEvents>();
        assert_eq!(events.0.len(), 1, "should emit 1 ColonizeRequested");
        assert_eq!(events.0[0].ship, ship_entity);
        assert_eq!(events.0[0].target_system, sys_entity);
        assert_eq!(
            events.0[0].planet,
            Some(planet_entity),
            "colonize_planet should set planet=Some(...) (vs colonize_system which sets None)"
        );
    }
}
