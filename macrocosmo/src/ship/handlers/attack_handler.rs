//! #334 Phase 3 (Commit 2): `AttackRequested` skeleton handler.
//!
//! This handler is a **no-op foundation** for #219 (Port combat engage)
//! and #220 (defensive platform autoresponse). Phase 3 only installs the
//! hook point so subsequent PRs add real combat logic without having to
//! touch the dispatcher registration or plugin wiring.
//!
//! Until those issues land there is no code path that emits
//! [`AttackRequested`] — the dispatcher does not currently expose any
//! user-facing `QueuedCommand::Attack` variant, and no remote-command
//! relay produces one either. The handler is therefore unreachable in
//! production; the unit test below wires a synthetic message writer to
//! demonstrate the intended shape and lock the contract
//! (request → handler → `CommandExecuted::Deferred` tagged
//! [`CommandKind::Attack`]).
//!
//! Why `Deferred`? Real combat resolution (damage application, return
//! fire, shield/armor/hull deltas) lives in `super::super::combat` and
//! takes place on subsequent ticks; the attack-request handler only
//! records intent. When #219 / #220 land they can either:
//! - keep this system as the intent recorder (and emit a downstream
//!   `CombatResolved` payload), or
//! - swap `Deferred` for a synchronous `Ok` / `Rejected` once the
//!   combat resolver is invoked inline.

use bevy::prelude::*;

use crate::time_system::GameClock;

use crate::ship::command_events::{AttackRequested, CommandExecuted, CommandKind, CommandResult};

pub fn handle_attack_requested(
    clock: Res<GameClock>,
    mut reqs: MessageReader<AttackRequested>,
    mut executed: MessageWriter<CommandExecuted>,
) {
    for req in reqs.read() {
        // #334 Phase 3: no-op skeleton. #219 / #220 will replace this
        // with real combat invocation. We emit `Deferred` so downstream
        // log/gamestate bridges see the intent flow without the stub
        // falsely claiming success.
        info!(
            "attack: received AttackRequested (cmd {}, attacker {:?} -> target {:?}) — skeleton, no-op",
            req.command_id.0, req.attacker, req.target
        );
        executed.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::Attack,
            ship: req.attacker,
            result: CommandResult::Deferred,
            completed_at: clock.elapsed,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship::command_events::{CommandEventsPlugin, CommandId};
    use bevy::MinimalPlugins;
    use bevy::ecs::message::Messages;

    #[test]
    fn attack_handler_skeleton_emits_deferred_command_executed() {
        // Wire the plugin + handler + a dummy clock.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(42));
        app.add_plugins(CommandEventsPlugin);
        app.add_systems(Update, handle_attack_requested);

        let attacker = app.world_mut().spawn_empty().id();
        let target = app.world_mut().spawn_empty().id();

        // Write an AttackRequested directly (the dispatcher has no
        // Attack arm yet — that's the #219/#220 entry point).
        {
            let mut msgs = app.world_mut().resource_mut::<Messages<AttackRequested>>();
            msgs.write(AttackRequested {
                command_id: CommandId(7),
                attacker,
                target,
                issued_at: 42,
            });
        }
        app.update();

        // Handler must emit exactly one CommandExecuted(Deferred, Attack).
        let executed = app.world().resource::<Messages<CommandExecuted>>();
        let mut cursor = executed.get_cursor();
        let all: Vec<&CommandExecuted> = cursor.read(executed).collect();
        assert_eq!(all.len(), 1, "handler must consume the AttackRequested");
        assert_eq!(all[0].command_id, CommandId(7));
        assert_eq!(all[0].kind, CommandKind::Attack);
        assert_eq!(all[0].ship, attacker);
        assert!(matches!(all[0].result, CommandResult::Deferred));
        assert_eq!(all[0].completed_at, 42);
    }

    #[test]
    fn attack_handler_no_messages_no_output() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(0));
        app.add_plugins(CommandEventsPlugin);
        app.add_systems(Update, handle_attack_requested);
        app.update();

        let executed = app.world().resource::<Messages<CommandExecuted>>();
        let mut cursor = executed.get_cursor();
        assert_eq!(cursor.read(executed).count(), 0);
    }
}
