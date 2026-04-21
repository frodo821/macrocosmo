//! #334: Event-driven command dispatch — message types, `CommandId`, and
//! the [`CommandEventsPlugin`] that registers them with Bevy.
//!
//! This module is the **skeleton** for the command dispatch refactor. Phase 1
//! only wires `MoveRequested` + `MoveToCoordinatesRequested` through the new
//! dispatcher / handler path; the other request types are pre-declared so
//! follow-up phases (Phase 2/3/4) only add handlers, not new message types.
//!
//! Bevy 0.18 renamed `Event` → `Message` (`MessageReader`, `MessageWriter`,
//! `App::add_message`). This module uses the new terminology throughout.
//!
//! See `docs/plan-334-command-dispatch-event-driven.md` §2.1 for the full
//! design rationale (per-variant types vs. single enum).

use bevy::prelude::*;

use super::{Owner, ReportMode};
use crate::amount::Amt;

// ---------------------------------------------------------------------------
// CommandId + allocator
// ---------------------------------------------------------------------------

/// Stable command identifier — allocated by the dispatcher, stitched into
/// `CommandRequested` messages and the terminal `CommandExecuted` so
/// `CommandLog` and (future) #268 relay dedup can match them without string
/// keys. Monotonic per-game-session.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommandId(pub u64);

impl CommandId {
    pub const ZERO: CommandId = CommandId(0);
}

/// Monotonic counter resource that hands out fresh [`CommandId`]s. Reset to
/// zero on a fresh game (implicit via `Default`); persistence of this
/// counter is intentionally *not* a save-format concern — command ids do
/// not need to survive save/load (in-flight messages are frame-transient).
#[derive(Resource, Debug, Default)]
pub struct NextCommandId(pub u64);

impl NextCommandId {
    /// Allocate a fresh [`CommandId`]. Returns strictly-monotonic values;
    /// the first call returns `CommandId(1)` so `CommandId(0)` can be used
    /// as a reserved / sentinel value if ever needed.
    pub fn allocate(&mut self) -> CommandId {
        self.0 = self.0.saturating_add(1);
        CommandId(self.0)
    }
}

// ---------------------------------------------------------------------------
// CommandKind + CommandResult (post-execution signal)
// ---------------------------------------------------------------------------

/// Discriminator carried on [`CommandExecuted`] so subscribers that only
/// care "command X finished" don't have to match on each variant's tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandKind {
    Move,
    MoveToCoordinates,
    Survey,
    Colonize,
    Scout,
    LoadDeliverable,
    DeployDeliverable,
    CoreDeploy,
    TransferToStructure,
    LoadFromScrapyard,
    Attack,
}

/// Terminal disposition of a dispatched command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Handler completed the semantic mutation successfully.
    Ok,
    /// Handler detected a late condition (race, state change, target
    /// despawn) and rolled back. `reason` is a short log-friendly key.
    Rejected { reason: String },
    /// Handler split the command — e.g. the async route planner spawned a
    /// `PendingRoute` and will finalize later, or an auto-inserted MoveTo
    /// prefix was queued. A follow-up `CommandExecuted` (with `Ok` or
    /// `Rejected`) will arrive later.
    Deferred,
}

// ---------------------------------------------------------------------------
// CommandRequested messages — one per QueuedCommand variant
// ---------------------------------------------------------------------------

/// Request to move a ship to a target star system (#108 MoveTo — FTL chain
/// -> hybrid FTL+sublight -> sublight fallback). Emitted by the dispatcher
/// after it validates that the ship exists, is Docked or Loitering, is not
/// immobile, and that the target system exists.
#[derive(Message, Debug, Clone)]
pub struct MoveRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub target: Entity,
    pub issued_at: i64,
}

/// Request to sublight-travel to an arbitrary deep-space coordinate (#185).
#[derive(Message, Debug, Clone)]
pub struct MoveToCoordinatesRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub target: [f64; 3],
    pub issued_at: i64,
}

/// Skeleton for Phase 2 migration of Survey. No handler yet.
#[derive(Message, Debug, Clone)]
pub struct SurveyRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub target_system: Entity,
    pub issued_at: i64,
}

/// Skeleton for Phase 2 migration of Colonize.
#[derive(Message, Debug, Clone)]
pub struct ColonizeRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub target_system: Entity,
    pub planet: Option<Entity>,
    pub issued_at: i64,
}

/// Skeleton for Phase 3 migration of Scout.
#[derive(Message, Debug, Clone)]
pub struct ScoutRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub target_system: Entity,
    pub observation_duration: i64,
    pub report_mode: ReportMode,
    pub issued_at: i64,
}

/// Skeleton for Phase 2 migration of LoadDeliverable.
#[derive(Message, Debug, Clone)]
pub struct LoadDeliverableRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub system: Entity,
    pub stockpile_index: usize,
    pub issued_at: i64,
}

/// Skeleton for Phase 2 migration of DeployDeliverable.
#[derive(Message, Debug, Clone)]
pub struct DeployDeliverableRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub position: [f64; 3],
    pub item_index: usize,
    pub issued_at: i64,
}

/// Skeleton for Phase 2 — replaces `PendingCoreDeploys` resource plumbing.
#[derive(Message, Debug, Clone)]
pub struct CoreDeployRequested {
    pub command_id: CommandId,
    pub deployer: Entity,
    pub target_system: Entity,
    pub deploy_pos: [f64; 3],
    pub faction_owner: Option<Entity>,
    pub owner: Owner,
    pub design_id: String,
    pub submitted_at: i64,
}

/// Skeleton for Phase 2 migration of TransferToStructure.
#[derive(Message, Debug, Clone)]
pub struct TransferToStructureRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub structure: Entity,
    pub minerals: Amt,
    pub energy: Amt,
    pub issued_at: i64,
}

/// Skeleton for Phase 2 migration of LoadFromScrapyard.
#[derive(Message, Debug, Clone)]
pub struct LoadFromScrapyardRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub structure: Entity,
    pub issued_at: i64,
}

/// Skeleton for #219 / #220 (defensive platform combat). No handler yet —
/// pre-declared so the plugin registers it and future work only adds the
/// handler system.
#[derive(Message, Debug, Clone)]
pub struct AttackRequested {
    pub command_id: CommandId,
    pub attacker: Entity,
    pub target: Entity,
    pub issued_at: i64,
}

// ---------------------------------------------------------------------------
// CommandExecuted — single tagged message consumed by log / gamestate bridge
// ---------------------------------------------------------------------------

/// Emitted by handlers (and the async `poll_pending_routes` system for
/// deferred routes) when a command reaches a terminal state. Consumed by:
/// - `bridge_command_executed_to_log` — updates [`crate::communication::CommandLog`].
/// - (Phase 4) `bridge_command_executed_to_gamestate` — enqueues Lua
///   `on_command_completed` hook payloads.
#[derive(Message, Debug, Clone)]
pub struct CommandExecuted {
    pub command_id: CommandId,
    pub kind: CommandKind,
    pub ship: Entity,
    pub result: CommandResult,
    pub completed_at: i64,
}

// ---------------------------------------------------------------------------
// CommandCompletedContext — Phase 4: EventContext carrying a CommandExecuted
// payload for the Lua `on("macrocosmo:command_completed", ...)` hook.
// ---------------------------------------------------------------------------

/// Typed `EventContext` payload built by
/// [`crate::ship::bridges::bridge_command_executed_to_gamestate`] from a
/// terminal [`CommandExecuted`] message. Exposes the command identity /
/// result as a flat Lua table so user scripts can read
/// `evt.command_id`, `evt.kind`, `evt.ship`, `evt.result`, `evt.reason`,
/// `evt.completed_at` from the standard `on(event_id, ...)` callback.
///
/// # Invariants
///
/// * Only constructed for **terminal** results (`Ok` / `Rejected`).
///   `Deferred` pairs do not produce a context — the follow-up
///   `CommandExecuted` is the one that fires the hook. See plan §10 R8.
/// * Values are serialised as `String` to match the
///   [`crate::event_system::EventContext::payload_get`] contract used by
///   `on(id, filter, fn)` structural filters — that filter path flattens
///   everything to strings and we mirror the existing `LuaDefinedEventContext`
///   shape rather than inventing a new serialisation.
/// * Never calls Lua code; only `lua.create_table()` + `table.set(...)`.
#[derive(Clone, Debug)]
pub struct CommandCompletedContext {
    pub command_id: CommandId,
    pub kind: CommandKind,
    pub ship: Entity,
    pub result_tag: &'static str,
    pub reason: Option<String>,
    pub completed_at: i64,
}

impl CommandCompletedContext {
    /// Map `CommandKind` to the string Lua scripts use to compare `evt.kind`.
    /// Lowercase snake-case matches what `request_command(kind, args)`
    /// accepts on the setter side (Phase 4 Commit 2) so a modder can write
    /// `if evt.kind == "move" then ... end` after calling
    /// `gs:request_command("move", ...)`.
    pub fn kind_str(kind: CommandKind) -> &'static str {
        match kind {
            CommandKind::Move => "move",
            CommandKind::MoveToCoordinates => "move_to_coordinates",
            CommandKind::Survey => "survey",
            CommandKind::Colonize => "colonize",
            CommandKind::Scout => "scout",
            CommandKind::LoadDeliverable => "load_deliverable",
            CommandKind::DeployDeliverable => "deploy_deliverable",
            CommandKind::CoreDeploy => "core_deploy",
            CommandKind::TransferToStructure => "transfer_to_structure",
            CommandKind::LoadFromScrapyard => "load_from_scrapyard",
            CommandKind::Attack => "attack",
        }
    }

    /// Build from a terminal `CommandExecuted`. Returns `None` when the
    /// result is `Deferred` — plan §10 R8 bans double-fires.
    pub fn from_executed(ev: &CommandExecuted) -> Option<Self> {
        let (tag, reason) = match &ev.result {
            CommandResult::Ok => ("ok", None),
            CommandResult::Rejected { reason } => ("rejected", Some(reason.clone())),
            CommandResult::Deferred => return None,
        };
        Some(Self {
            command_id: ev.command_id,
            kind: ev.kind,
            ship: ev.ship,
            result_tag: tag,
            reason,
            completed_at: ev.completed_at,
        })
    }
}

impl crate::event_system::EventContext for CommandCompletedContext {
    fn event_id(&self) -> &str {
        crate::event_system::COMMAND_COMPLETED_EVENT
    }

    fn to_lua_table(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Table> {
        let t = lua.create_table()?;
        t.set("event_id", crate::event_system::COMMAND_COMPLETED_EVENT)?;
        // Command id as decimal string — mirror the `LuaDefinedEventContext`
        // discipline (everything is strings, filter-compatible).
        t.set("command_id", self.command_id.0.to_string())?;
        t.set("kind", Self::kind_str(self.kind))?;
        t.set("ship", self.ship.to_bits().to_string())?;
        t.set("result", self.result_tag)?;
        if let Some(r) = &self.reason {
            t.set("reason", r.as_str())?;
        }
        t.set("completed_at", self.completed_at.to_string())?;
        Ok(t)
    }

    fn payload_get(&self, key: &str) -> Option<std::borrow::Cow<'_, str>> {
        use std::borrow::Cow;
        match key {
            "kind" => Some(Cow::Borrowed(Self::kind_str(self.kind))),
            "result" => Some(Cow::Borrowed(self.result_tag)),
            "reason" => self.reason.as_deref().map(Cow::Borrowed),
            "command_id" => Some(Cow::Owned(self.command_id.0.to_string())),
            "ship" => Some(Cow::Owned(self.ship.to_bits().to_string())),
            "completed_at" => Some(Cow::Owned(self.completed_at.to_string())),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// Registers the `NextCommandId` resource and every `CommandRequested` /
/// `CommandExecuted` message type. Keeps `main.rs` free of per-variant
/// `add_message` noise as later phases add handlers.
pub struct CommandEventsPlugin;

impl Plugin for CommandEventsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NextCommandId>();
        // Per-variant request messages — all registered up front so handlers
        // added in later phases only need to write a new system.
        app.add_message::<MoveRequested>();
        app.add_message::<MoveToCoordinatesRequested>();
        app.add_message::<SurveyRequested>();
        app.add_message::<ColonizeRequested>();
        app.add_message::<ScoutRequested>();
        app.add_message::<LoadDeliverableRequested>();
        app.add_message::<DeployDeliverableRequested>();
        app.add_message::<CoreDeployRequested>();
        app.add_message::<TransferToStructureRequested>();
        app.add_message::<LoadFromScrapyardRequested>();
        app.add_message::<AttackRequested>();
        // Single tagged "command finished" message.
        app.add_message::<CommandExecuted>();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::message::Messages;

    #[test]
    fn command_id_allocator_is_monotonic() {
        let mut next = NextCommandId::default();
        let a = next.allocate();
        let b = next.allocate();
        let c = next.allocate();
        assert_eq!(a.0, 1);
        assert_eq!(b.0, 2);
        assert_eq!(c.0, 3);
        assert!(a < b && b < c);
    }

    #[test]
    fn command_id_allocator_starts_at_one_not_zero() {
        // CommandId(0) is reserved as ZERO sentinel.
        let mut next = NextCommandId::default();
        assert_ne!(next.allocate(), CommandId::ZERO);
    }

    /// §6 open-question guard: confirm the `MessageReader` iteration order
    /// matches the `MessageWriter::write` order so downstream handlers can
    /// rely on FIFO delivery. Locking this assumption here keeps future
    /// Bevy upgrades honest.
    #[test]
    fn test_message_reader_preserves_emit_order() {
        let mut app = App::new();
        app.add_plugins(CommandEventsPlugin);

        let dummy_ship = Entity::from_raw_u32(1).unwrap();
        let dummy_target = Entity::from_raw_u32(2).unwrap();

        // Write 100 distinct MoveRequested messages in ascending id order.
        {
            let mut messages = app.world_mut().resource_mut::<Messages<MoveRequested>>();
            for i in 1..=100u64 {
                messages.write(MoveRequested {
                    command_id: CommandId(i),
                    ship: dummy_ship,
                    target: dummy_target,
                    issued_at: i as i64,
                });
            }
        }

        // Collect ids in `read()` order and verify strict ascending.
        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        let ids: Vec<u64> = cursor.read(messages).map(|m| m.command_id.0).collect();
        assert_eq!(ids.len(), 100);
        for (expected, got) in (1..=100u64).zip(ids.iter().copied()) {
            assert_eq!(expected, got, "MessageReader must preserve FIFO emit order");
        }
    }

    #[test]
    fn plugin_registers_next_command_id_resource() {
        let mut app = App::new();
        app.add_plugins(CommandEventsPlugin);
        assert!(app.world().get_resource::<NextCommandId>().is_some());
    }
}
