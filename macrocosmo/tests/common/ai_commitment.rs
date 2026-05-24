//! #468 PR-3 HIGH D fold-in: shared "is the AI committed to this work?"
//! helpers, hoisted out of the per-file `count_outbox_for` copies that
//! grew across `ai_npc_outbox_dedup`, `ai_per_region_npc_smoke`,
//! `mid_agent_member_filter`, and the new PR-3 tests.
//!
//! The dedup contract for #468 PR-1/PR-2/PR-3 is "exactly one in-flight
//! command per (issuer, kind, target_system) at any time during the
//! light-speed courier window." The AI tracks in-flight commands across
//! two stores:
//!
//! 1. **`AiCommandOutbox`** — legacy light-speed outbox for kinds that
//!    haven't migrated yet (today only government / non-ship kinds:
//!    `build_ship`, `fortify_system`, `research_focus`,
//!    `build_structure`, `retreat`, `build_deliverable`). Filtered by
//!    `(kind, target_system)`.
//! 2. **`PendingAiShipCommand`** — per-ship holder for ship-control
//!    kinds (survey/colonize/reposition/blockade as of PR-2;
//!    attack/move_ruler/load/unload/colonize_planet as of PR-3). One
//!    entity per ship × kind, keyed by `(kind, target_system)`.
//!
//! Tests asking "is the AI already trying to do X to system Y?" must
//! check both stores. These helpers wrap that bookkeeping so each test
//! site is a one-liner.

use bevy::prelude::*;

use macrocosmo::ai::command_consumer::PendingAiShipCommand;
use macrocosmo::ai::command_outbox::AiCommandOutbox;
use macrocosmo_ai::{CommandKindId, CommandValue};

/// Count in-flight AI commitments for `(kind, target)`. Pools both the
/// legacy outbox path and the new per-ship holder path so callers don't
/// need to know which storage a given kind uses today.
///
/// Allowed to take `&mut App` because `app.world_mut().query::<...>()`
/// requires mutable access to spin up a new query state. Tests don't
/// mind the mutability — the function only reads.
pub fn count_ai_commitments(app: &mut App, kind: CommandKindId, target_system: Entity) -> usize {
    let outbox_count = {
        let outbox = app.world().resource::<AiCommandOutbox>();
        outbox
            .entries
            .iter()
            .filter(|entry| {
                let cmd = &entry.command;
                if cmd.kind != kind {
                    return false;
                }
                match cmd.params.get("target_system") {
                    Some(CommandValue::System(sys_id)) => target_system.to_bits() == sys_id.0,
                    _ => false,
                }
            })
            .count()
    };

    let ship_command_count = {
        let mut q = app.world_mut().query::<&PendingAiShipCommand>();
        q.iter(app.world())
            .filter(|p| p.kind == kind && p.target_system == target_system)
            .count()
    };

    outbox_count + ship_command_count
}

/// Convenience predicate over [`count_ai_commitments`]. Returns `true`
/// when at least one in-flight command for `(kind, target)` exists in
/// either store.
pub fn has_ai_commitment(app: &mut App, kind: CommandKindId, target_system: Entity) -> bool {
    count_ai_commitments(app, kind, target_system) > 0
}
