//! #334 Phase 1: event-bus subscribers that translate `CommandExecuted`
//! messages into side-effects for downstream consumers (`CommandLog`,
//! future Lua `on_command_completed` hooks, AI debug telemetry).
//!
//! Phase 1 scope: `bridge_command_executed_to_log` — keeps the per-empire
//! `CommandLog` UI component in sync with the dispatcher/handler pipeline
//! by matching entries via `CommandId` and flipping `status` /
//! `executed_at`. Phase 4 adds `bridge_command_executed_to_gamestate`.

use bevy::prelude::*;

use crate::communication::{CommandLog, CommandLogStatus};
use crate::player::PlayerEmpire;
use crate::ship::command_events::{CommandExecuted, CommandResult};

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
        world
            .spawn((PlayerEmpire, CommandLog { entries }))
            .id()
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
            let mut q = app.world_mut().query_filtered::<&CommandLog, With<PlayerEmpire>>();
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
            let mut q = app.world_mut().query_filtered::<&CommandLog, With<PlayerEmpire>>();
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
            let mut q = app.world_mut().query_filtered::<&CommandLog, With<PlayerEmpire>>();
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
            let mut q = app.world_mut().query_filtered::<&CommandLog, With<PlayerEmpire>>();
            q.single(app.world()).unwrap().clone_entries()
        };
        // The kept entry stays Dispatched.
        assert_eq!(log[0].status, CommandLogStatus::Dispatched);
    }
}
