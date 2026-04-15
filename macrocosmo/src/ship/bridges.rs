//! #334 Phase 1/4: event-bus subscribers that translate `CommandExecuted`
//! messages into side-effects for downstream consumers (`CommandLog`, the
//! Lua `on("macrocosmo:command_completed", ...)` hook, AI debug telemetry).
//!
//! * **Phase 1** — `bridge_command_executed_to_log`: keeps the per-empire
//!   `CommandLog` UI component in sync with the dispatcher/handler pipeline
//!   by matching entries via `CommandId` and flipping `status` /
//!   `executed_at`.
//! * **Phase 4** — `bridge_command_executed_to_gamestate`: forwards terminal
//!   results (`Ok` / `Rejected`) to the event bus as a
//!   [`crate::event_system::COMMAND_COMPLETED_EVENT`] with a typed
//!   [`CommandCompletedContext`] payload, so Lua scripts get an
//!   `on_command_completed(evt)`-equivalent hook **via the queue** — never
//!   sync-dispatched from inside a handler. See plan §7 Phase 4 and
//!   `memory/feedback_rust_no_lua_callback.md` for the reentrancy rationale.

use bevy::prelude::*;

use crate::communication::{CommandLog, CommandLogStatus};
use crate::event_system::EventSystem;
use crate::player::PlayerEmpire;
use crate::ship::command_events::{CommandCompletedContext, CommandExecuted, CommandResult};
use crate::time_system::GameClock;

/// Reads every `CommandExecuted` this frame and updates the matching
/// `CommandLog` entry on the player empire. Entries are keyed by
/// `CommandId`; unmatched `CommandExecuted` messages are silently ignored
/// (NPC empires don't populate `CommandLog` yet — plan §5).
pub fn bridge_command_executed_to_log(
    mut executed: MessageReader<CommandExecuted>,
    mut log_q: Query<&mut CommandLog, With<PlayerEmpire>>,
) {
    let Ok(mut log) = log_q.single_mut() else {
        // Drain so messages don't pile up across frames when there's no
        // player empire (observer mode, teardown).
        for _ in executed.read() {}
        return;
    };
    for event in executed.read() {
        let Some(entry) = log
            .entries
            .iter_mut()
            .find(|e| e.command_id == Some(event.command_id))
        else {
            // No matching dispatcher-side entry. Either the command was
            // issued for an NPC empire, the entry predates dispatcher
            // tracking, or save/load wiped it. Nothing to update.
            continue;
        };
        entry.executed_at = Some(event.completed_at);
        entry.status = match &event.result {
            CommandResult::Ok => CommandLogStatus::Executed,
            CommandResult::Rejected { reason } => CommandLogStatus::Rejected {
                reason: reason.clone(),
            },
            CommandResult::Deferred => CommandLogStatus::Deferred,
        };
        // Keep the legacy `arrived` flag in sync for bottom-bar rendering
        // (terminal dispositions — Ok / Rejected — flip it true so the UI
        // shows "arrived"; Deferred stays false while the follow-up is
        // pending).
        entry.arrived = !matches!(entry.status, CommandLogStatus::Deferred);
    }
}

/// #334 Phase 4 bridge: reads every terminal `CommandExecuted` this frame
/// and enqueues a [`COMMAND_COMPLETED_EVENT`](crate::event_system::COMMAND_COMPLETED_EVENT)
/// on `EventSystem` so the standard `dispatch_event_handlers` loop picks it
/// up next tick and delivers it to any Lua handler registered via
/// `on("macrocosmo:command_completed", ...)`.
///
/// **Queue-only, never sync-dispatched** (plan §9.2 /
/// `feedback_rust_no_lua_callback.md`). The Rust handler that emits
/// `CommandExecuted` has already released its borrows by the time this
/// bridge runs; the bridge itself never calls into Lua. When the Lua hook
/// eventually runs (next invocation of `dispatch_event_handlers`), it
/// receives a fresh `ctx.gamestate` scope and may call
/// `gs:request_command(...)` to re-enter the command pipeline without
/// reentrancy because the dispatcher/handler path runs in a distinct
/// system and a distinct Bevy message cycle.
///
/// **Deferred filter** (plan §10 R8): `CommandResult::Deferred` messages are
/// skipped so the hook never double-fires for async routes. Only the
/// terminal follow-up emits the event.
pub fn bridge_command_executed_to_gamestate(
    clock: Res<GameClock>,
    mut executed: MessageReader<CommandExecuted>,
    mut event_system: ResMut<EventSystem>,
) {
    for ev in executed.read() {
        // Skip `Deferred` — a terminal `CommandExecuted` will follow.
        let Some(ctx) = CommandCompletedContext::from_executed(ev) else {
            continue;
        };
        event_system.fire_event_with_payload(None, clock.elapsed, Box::new(ctx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::communication::CommandLogEntry;
    use crate::ship::command_events::{CommandEventsPlugin, CommandId, CommandKind};
    use bevy::MinimalPlugins;
    use bevy::ecs::message::Messages;

    fn make_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(CommandEventsPlugin);
        app.add_systems(Update, bridge_command_executed_to_log);
        app
    }

    fn spawn_empire_with_log(world: &mut World, entries: Vec<CommandLogEntry>) -> Entity {
        world.spawn((PlayerEmpire, CommandLog { entries })).id()
    }

    #[test]
    fn ok_terminal_flips_status_and_arrived() {
        let mut app = make_app();
        let cid = CommandId(42);
        spawn_empire_with_log(
            app.world_mut(),
            vec![CommandLogEntry::new_dispatched("MoveTo X".into(), 10, cid)],
        );
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<CommandExecuted>>();
            msgs.write(CommandExecuted {
                command_id: cid,
                kind: CommandKind::Move,
                ship: Entity::from_raw_u32(1).unwrap(),
                result: CommandResult::Ok,
                completed_at: 20,
            });
        }
        app.update();

        let log = {
            let mut q = app
                .world_mut()
                .query_filtered::<&CommandLog, With<PlayerEmpire>>();
            q.single(app.world()).unwrap().clone_entries()
        };
        assert_eq!(log[0].status, CommandLogStatus::Executed);
        assert_eq!(log[0].executed_at, Some(20));
        assert!(log[0].arrived);
    }

    // Little helper because CommandLog itself isn't Clone.
    trait CloneEntries {
        fn clone_entries(&self) -> Vec<CommandLogEntry>;
    }
    impl CloneEntries for CommandLog {
        fn clone_entries(&self) -> Vec<CommandLogEntry> {
            self.entries
                .iter()
                .map(|e| CommandLogEntry {
                    description: e.description.clone(),
                    sent_at: e.sent_at,
                    arrives_at: e.arrives_at,
                    arrived: e.arrived,
                    command_id: e.command_id,
                    status: e.status.clone(),
                    executed_at: e.executed_at,
                })
                .collect()
        }
    }

    #[test]
    fn rejected_terminal_records_reason() {
        let mut app = make_app();
        let cid = CommandId(7);
        spawn_empire_with_log(
            app.world_mut(),
            vec![CommandLogEntry::new_dispatched("MoveTo X".into(), 0, cid)],
        );
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<CommandExecuted>>();
            msgs.write(CommandExecuted {
                command_id: cid,
                kind: CommandKind::Move,
                ship: Entity::from_raw_u32(1).unwrap(),
                result: CommandResult::Rejected {
                    reason: "target despawned".into(),
                },
                completed_at: 5,
            });
        }
        app.update();
        let log = {
            let mut q = app
                .world_mut()
                .query_filtered::<&CommandLog, With<PlayerEmpire>>();
            q.single(app.world()).unwrap().clone_entries()
        };
        assert_eq!(
            log[0].status,
            CommandLogStatus::Rejected {
                reason: "target despawned".into()
            }
        );
        assert!(log[0].arrived);
    }

    #[test]
    fn deferred_leaves_arrived_false() {
        let mut app = make_app();
        let cid = CommandId(3);
        spawn_empire_with_log(
            app.world_mut(),
            vec![CommandLogEntry::new_dispatched("MoveTo X".into(), 0, cid)],
        );
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<CommandExecuted>>();
            msgs.write(CommandExecuted {
                command_id: cid,
                kind: CommandKind::Move,
                ship: Entity::from_raw_u32(1).unwrap(),
                result: CommandResult::Deferred,
                completed_at: 2,
            });
        }
        app.update();
        let log = {
            let mut q = app
                .world_mut()
                .query_filtered::<&CommandLog, With<PlayerEmpire>>();
            q.single(app.world()).unwrap().clone_entries()
        };
        assert_eq!(log[0].status, CommandLogStatus::Deferred);
        assert!(!log[0].arrived);
    }

    #[test]
    fn unmatched_command_id_is_ignored() {
        let mut app = make_app();
        let cid_kept = CommandId(1);
        spawn_empire_with_log(
            app.world_mut(),
            vec![CommandLogEntry::new_dispatched("kept".into(), 0, cid_kept)],
        );
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<CommandExecuted>>();
            // Message with an unmatched id — should be ignored, not panic.
            msgs.write(CommandExecuted {
                command_id: CommandId(999),
                kind: CommandKind::Move,
                ship: Entity::from_raw_u32(1).unwrap(),
                result: CommandResult::Ok,
                completed_at: 5,
            });
        }
        app.update();
        let log = {
            let mut q = app
                .world_mut()
                .query_filtered::<&CommandLog, With<PlayerEmpire>>();
            q.single(app.world()).unwrap().clone_entries()
        };
        // The kept entry stays Dispatched.
        assert_eq!(log[0].status, CommandLogStatus::Dispatched);
    }

    // ------------------------------------------------------------------
    // Phase 4: `bridge_command_executed_to_gamestate` tests.
    // ------------------------------------------------------------------

    fn make_gs_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(CommandEventsPlugin);
        app.insert_resource(EventSystem::default());
        app.insert_resource(GameClock::new(100));
        app.add_systems(Update, bridge_command_executed_to_gamestate);
        app
    }

    #[test]
    fn gamestate_bridge_ok_enqueues_command_completed_event() {
        let mut app = make_gs_app();
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<CommandExecuted>>();
            msgs.write(CommandExecuted {
                command_id: CommandId(17),
                kind: CommandKind::Move,
                ship: Entity::from_raw_u32(3).unwrap(),
                result: CommandResult::Ok,
                completed_at: 100,
            });
        }
        app.update();

        let es = app.world().resource::<EventSystem>();
        assert_eq!(
            es.fired_log.len(),
            1,
            "terminal Ok result should enqueue exactly one event"
        );
        let fired = &es.fired_log[0];
        assert_eq!(fired.event_id, crate::event_system::COMMAND_COMPLETED_EVENT);
        let ctx = fired.payload.as_ref().expect("payload attached");
        // Filter-compatible accessors match what `on(id, filter, fn)` sees.
        assert_eq!(ctx.payload_get("kind").as_deref(), Some("move"));
        assert_eq!(ctx.payload_get("result").as_deref(), Some("ok"));
        assert_eq!(ctx.payload_get("command_id").as_deref(), Some("17"));
        assert_eq!(ctx.payload_get("completed_at").as_deref(), Some("100"));
    }

    #[test]
    fn gamestate_bridge_rejected_carries_reason() {
        let mut app = make_gs_app();
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<CommandExecuted>>();
            msgs.write(CommandExecuted {
                command_id: CommandId(5),
                kind: CommandKind::Survey,
                ship: Entity::from_raw_u32(2).unwrap(),
                result: CommandResult::Rejected {
                    reason: "ship despawned".into(),
                },
                completed_at: 42,
            });
        }
        app.update();

        let es = app.world().resource::<EventSystem>();
        assert_eq!(es.fired_log.len(), 1);
        let ctx = es.fired_log[0].payload.as_ref().unwrap();
        assert_eq!(ctx.payload_get("result").as_deref(), Some("rejected"));
        assert_eq!(ctx.payload_get("reason").as_deref(), Some("ship despawned"));
        assert_eq!(ctx.payload_get("kind").as_deref(), Some("survey"));
    }

    #[test]
    fn gamestate_bridge_deferred_is_skipped() {
        // Plan §10 R8: Deferred must not enqueue — the subsequent terminal
        // CommandExecuted is the one that fires the hook.
        let mut app = make_gs_app();
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<CommandExecuted>>();
            msgs.write(CommandExecuted {
                command_id: CommandId(1),
                kind: CommandKind::Move,
                ship: Entity::from_raw_u32(1).unwrap(),
                result: CommandResult::Deferred,
                completed_at: 10,
            });
        }
        app.update();

        let es = app.world().resource::<EventSystem>();
        assert!(
            es.fired_log.is_empty(),
            "Deferred result must not enqueue a command_completed event"
        );
    }

    #[test]
    fn gamestate_bridge_terminal_payload_shape_for_all_kinds() {
        // Regression: every CommandKind must map to a non-empty `kind_str`
        // so `evt.kind == "..."` in Lua can match. The full table lives on
        // `CommandCompletedContext::kind_str`; this exercises the path end
        // to end for one representative kind per result variant.
        let mut app = make_gs_app();
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<CommandExecuted>>();
            for (i, kind) in [
                CommandKind::Move,
                CommandKind::MoveToCoordinates,
                CommandKind::Survey,
                CommandKind::Colonize,
                CommandKind::Scout,
                CommandKind::LoadDeliverable,
                CommandKind::DeployDeliverable,
                CommandKind::CoreDeploy,
                CommandKind::TransferToStructure,
                CommandKind::LoadFromScrapyard,
                CommandKind::Attack,
            ]
            .into_iter()
            .enumerate()
            {
                msgs.write(CommandExecuted {
                    command_id: CommandId((i + 1) as u64),
                    kind,
                    ship: Entity::from_raw_u32((i + 1) as u32).unwrap(),
                    result: CommandResult::Ok,
                    completed_at: 1,
                });
            }
        }
        app.update();
        let es = app.world().resource::<EventSystem>();
        assert_eq!(es.fired_log.len(), 11, "one event per CommandKind");
        for entry in &es.fired_log {
            let ctx = entry.payload.as_ref().expect("payload attached");
            let kind = ctx.payload_get("kind").unwrap_or_default().to_string();
            assert!(!kind.is_empty(), "kind must be a non-empty string");
        }
    }
}
