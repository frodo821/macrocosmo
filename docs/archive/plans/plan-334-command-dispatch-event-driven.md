# Implementation Plan: Issue #334 — Command dispatch refactor (event-driven split, Option C)

_Prepared 2026-04-15 by Plan agent. Depends on #332 (merged) for the `CommandExecuted` → gamestate scope-closure integration (Phase 4). #296 (merged) provides the current `PendingCoreDeploys` / `resolve_core_deploys` path that this refactor folds back into the event pipeline. Blocker for #268 (Courier opportunistic relay — needs dedup plumbing) and #302/#321 (diplomacy v2 setter family — lands on the same Phase 4 event surface)._

---

## 0. TL;DR

`process_deliverable_commands` already hits Bevy's 16-arg SystemParam cap even after #296 merged two Core-related queries into a tuple query. The next generation of commands (Port combat engage #219, defensive platforms #220, courier relay #268, diplomacy v2 setter #321) will double or triple the variant count within a milestone, and the current shape (**one fat dispatcher that both validates AND mutates per variant**) cannot absorb them.

This plan introduces an **event-driven split** (Option C from the issue):

- **Single lightweight dispatcher** (`process_queued_commands` replacing the mutating loop in `process_deliverable_commands` / `process_command_queue`) that only validates, dedups, and emits typed `CommandRequested` messages.
- **One handler system per command kind**, each holding only the queries it actually needs. Handlers read a single `MessageReader<CommandRequested::X>` (or a tagged enum — see §2.1 trade-off) and are ordered `.after(dispatcher)` for same-tick execution.
- **Post-handler `CommandExecuted` message** emitted by each handler. Consumed by `CommandLog`, `GameEvent`, the #332 gamestate hook bridge, and (future) the `on_command_completed` Lua hook via the queue-only pattern from `feedback_rust_no_lua_callback.md`.
- **Migration is phased** (4 phases, ≈15 commits total, 3 landable PRs). No semantic changes to command behaviour at any phase boundary.

The refactor is a **behaviour-preserving mechanical reshape**. Every variant's validation logic moves verbatim; what changes is where it lives (dispatcher, handler) and how results flow (inline state write → message → deferred handler). Tests are the contract.

---

## §1 現状棚卸し

### 1.1 `process_deliverable_commands` SystemParam (16 args, at cap)

Source: `macrocosmo/src/ship/deliverable_ops.rs:44-78`.

| # | param | kind | notes |
|---|---|---|---|
| 1 | `commands: Commands` | deferred writes | spawn deliverable entity |
| 2 | `clock: Res<GameClock>` | read | `clock.elapsed` for event timestamps / ticket submit_at |
| 3 | `balance: Res<GameBalance>` | read | `mass_per_item_slot` |
| 4 | `registry: Res<StructureRegistry>` | read | cargo size lookup, `spawns_as_ship` marker |
| 5 | `events: MessageWriter<GameEvent>` | write | dual-write `ShipBuilt` events |
| 6 | `ships: Query<(Entity, &Ship, &ShipState, &Position, &mut CommandQueue, &mut Cargo, &ShipModifiers)>` | read+write | 7-tuple, main driver |
| 7 | `stockpiles: Query<&mut DeliverableStockpile>` | write | load/unload |
| 8 | `platforms: Query<(&Position, &mut ConstructionPlatform), Without<Ship>>` | write | `TransferToStructure` |
| 9 | `scrapyards: Query<(&Position, &mut Scrapyard), Without<Ship>>` | write | `LoadFromScrapyard` |
| 10 | `structures: Query<(&DeepSpaceStructure, &Position), Without<Ship>>` | read (unused, reserved) | future use; currently `let _ = structures` |
| 11 | `player_q: Query<&StationedAt, Without<Ship>>` | read | vantage snapshot |
| 12 | `player_aboard_q: Query<&AboardShip, With<Player>>` | read | vantage snapshot |
| 13 | `star_systems: Query<(Entity, &Position), (Without<Ship>, With<StarSystem>)>` | read | vantage + Core proximity (tuple-merged in #296) |
| 14 | `existing_cores: Query<&AtSystem, With<CoreShip>>` | read | "already has Core" validation (#296) |
| 15 | `pending_cores: ResMut<PendingCoreDeploys>` | write | Core ticket queue (#296, becomes `MessageWriter<CoreDeployRequested>` post-refactor) |
| 16 | `fact_sys: FactSysParam` | bundle | knowledge dual-writes |

`FactSysParam` is itself a multi-field `SystemParam` bundle (knowledge writers + event id allocator), so the 16 count understates the internal pressure. Adding any new read query — e.g. `FactionOwner` lookups for diplomacy gating, or a relay-ship-state query for #268 — requires either another tuple merge (which hurts readability) or splitting the system (which is this plan).

### 1.2 `QueuedCommand` variants (pub enum)

Source: `macrocosmo/src/ship/mod.rs:117-166`.

| variant | fields | current dispatcher | kind |
|---|---|---|---|
| `MoveTo` | `{ system: Entity }` | `process_command_queue` (`command.rs:263-373`) | movement |
| `MoveToCoordinates` | `{ target: [f64; 3] }` | `process_command_queue` (`command.rs:374-402`) | movement |
| `Survey` | `{ system: Entity }` | `process_command_queue` (`command.rs:486-523`) | action |
| `Colonize` | `{ system, planet: Option<Entity> }` | `process_command_queue` (`command.rs:525-553`) | action |
| `Scout` | `{ target_system, observation_duration, report_mode }` | `process_command_queue` (`command.rs:403-481`) | action |
| `LoadDeliverable` | `{ system, stockpile_index }` | `process_deliverable_commands` (`deliverable_ops.rs:100-179`) | cargo |
| `DeployDeliverable` | `{ position, item_index }` | `process_deliverable_commands` (`deliverable_ops.rs:180-323`) | cargo + Core branch |
| `TransferToStructure` | `{ structure, minerals, energy }` | `process_deliverable_commands` (`deliverable_ops.rs:325-367`) | cargo |
| `LoadFromScrapyard` | `{ structure }` | `process_deliverable_commands` (`deliverable_ops.rs:368-425`) | cargo |

Nine variants today. Near-term additions already on the roadmap:
- `AttackTarget { ship_or_structure }` or similar (#219 / #220)
- `RelayCommand { command_id }` (#268)
- Multiple diplomacy setter commands (#302 / #321 — declare war, offer trade, etc.) in Phase 4

### 1.3 Variant → handler flow map (current, per grep)

From `Grep QueuedCommand::Variant` across `macrocosmo/src/ship/`:

```
MoveTo          → process_command_queue (async routing via routing::spawn_route_task_full)
                  → poll_pending_routes (after ApplyDeferred) finalizes FTL/sublight
                  → (for remote path) process_pending_ship_commands (command.rs:83-143)
MoveToCoordinates → process_command_queue (inline sublight)
Survey          → process_command_queue
                  + process_pending_ship_commands (remote path)
Colonize        → process_command_queue
                  + process_pending_ship_commands (remote path)
Scout           → process_command_queue
LoadDeliverable → process_deliverable_commands
DeployDeliverable → process_deliverable_commands
                    ├─ non-Core: spawn_deliverable_entity (inline)
                    └─ Core branch: push CoreDeployTicket → resolve_core_deploys
TransferToStructure → process_deliverable_commands
LoadFromScrapyard  → process_deliverable_commands
```

Note: `process_command_queue` and `process_deliverable_commands` **both match every variant exhaustively** because `QueuedCommand` is `#[non_exhaustive]`-free — each system has a `_ => {}` fallthrough arm that explicitly lists the other system's variants as no-ops. This enforces ordering invariants at the compiler level but also means every new variant touches two files.

### 1.4 `CommandLog` write sites

Source: `macrocosmo/src/communication/mod.rs:293-418`.

`CommandLog` is a per-empire Resource+Component that holds user-facing `CommandLogEntry { description, sent_at, arrives_at, arrived }`. Current write sites:

1. `send_remote_command` (`mod.rs:397-409`) — **dispatcher-side**, on message send: `arrived = false`.
2. `process_pending_commands` (`mod.rs:420+`) — **handler-side**, on message arrival: flips `arrived = true` via description match (string key, fragile).

The event-driven refactor replaces this brittle string match with a stable `command_id: u64` (newtype) carried through both the `CommandRequested` message and the `CommandExecuted` message. See §4.

### 1.5 `PendingCoreDeploys` intermediate resource (#296)

Source: `macrocosmo/src/ship/core_deliverable.rs:71-77, 110-206`.

Current flow:
1. `process_deliverable_commands` validates the Core-deploy (`deliverable_ops.rs:218-286`), then `pending_cores.tickets.push(ticket)` and removes the cargo item.
2. `resolve_core_deploys` runs `.after(process_deliverable_commands)` (`mod.rs:200-201`), drains the resource, tie-breaks with `GameRng`, spawns via `spawn_core_ship_from_deliverable`.

**This is already half an event pipeline** — a side channel resource passed between systems in the same tick. Converting it to `MessageReader<CoreDeployRequested>` is mechanical; the tie-break logic moves into the handler unchanged. Worth noting that Bevy 0.18 renames `Event` → `Message`, `EventReader` → `MessageReader`, `EventWriter` → `MessageWriter`, `add_event` → `add_message` (see `events.rs:112` and `notifications.rs:586`). This plan uses the Bevy 0.18 terminology throughout.

### 1.6 Lua callback path (from `feedback_rust_no_lua_callback.md`)

Critical invariant confirmed in Phase 4: `CommandExecuted` **must not be delivered synchronously into a Lua callback from a handler system**. Instead, a reader system (`bridge_command_events_to_lua` in Phase 4) enqueues `_pending_script_events` entries for the existing `fire_event` dispatch loop to process on the next tick. This preserves the queue-only reentrancy discipline #332 relies on.

---

## §2 Event-driven design

### 2.1 Message type shape — one enum vs. per-variant types

**Recommendation: per-variant message types** wrapped in a trait-free module, not a single enum.

```rust
// macrocosmo/src/ship/command_events.rs (new module)

/// Stable command identifier — allocated by the dispatcher, stitched into
/// `CommandRequested` and `CommandExecuted` so `CommandLog` and #268 relay
/// dedup can match them without string keys. Monotonic per-game-session.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct CommandId(pub u64);

#[derive(Resource, Default)]
pub struct NextCommandId(pub u64);

#[derive(Message, Debug, Clone)]
pub struct MoveRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub target: Entity,         // target star system
    pub issued_at: i64,
}

#[derive(Message, Debug, Clone)]
pub struct MoveToCoordinatesRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub target: [f64; 3],
    pub issued_at: i64,
}

#[derive(Message, Debug, Clone)]
pub struct SurveyRequested { pub command_id: CommandId, pub ship: Entity, pub target_system: Entity, pub issued_at: i64 }

#[derive(Message, Debug, Clone)]
pub struct ColonizeRequested { pub command_id: CommandId, pub ship: Entity, pub target_system: Entity, pub planet: Option<Entity>, pub issued_at: i64 }

#[derive(Message, Debug, Clone)]
pub struct ScoutRequested { /* … ship, target, duration, report_mode, cmd_id, issued_at */ }

#[derive(Message, Debug, Clone)]
pub struct LoadDeliverableRequested { /* … ship, system, stockpile_index, cmd_id, issued_at */ }

#[derive(Message, Debug, Clone)]
pub struct DeployDeliverableRequested {
    pub command_id: CommandId,
    pub ship: Entity,
    pub position: [f64; 3],
    pub item_index: usize,
    pub issued_at: i64,
}

/// Produced by the `DeployDeliverable` handler's Core-branch instead of
/// `PendingCoreDeploys` tickets. Consumed by `resolve_core_deploys` (or a
/// renamed `handle_core_deploy_requested`).
#[derive(Message, Debug, Clone)]
pub struct CoreDeployRequested {
    pub command_id: CommandId,
    pub deployer: Entity,
    pub target_system: Entity,
    pub deploy_pos: [f64; 3],
    pub faction_owner: Option<Entity>,
    pub owner: crate::ship::Owner,
    pub design_id: String,
    pub submitted_at: i64,
}

#[derive(Message, Debug, Clone)]
pub struct TransferToStructureRequested { /* … */ }

#[derive(Message, Debug, Clone)]
pub struct LoadFromScrapyardRequested { /* … */ }

#[derive(Message, Debug, Clone)]
pub struct AttackRequested { /* #219 / #220 forward-compat skeleton */ pub command_id: CommandId, pub attacker: Entity, pub target: Entity, pub issued_at: i64 }

/// Post-handler notification. `kind` is a small enum (no payload) so
/// subscribers that only care about "command X completed" don't have to
/// match on each variant's arg tuple.
#[derive(Message, Debug, Clone)]
pub struct CommandExecuted {
    pub command_id: CommandId,
    pub kind: CommandKind,
    pub ship: Entity,
    pub result: CommandResult,
    pub completed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Move, MoveToCoordinates, Survey, Colonize, Scout,
    LoadDeliverable, DeployDeliverable, CoreDeploy,
    TransferToStructure, LoadFromScrapyard, Attack,
}

#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Handler completed the semantic mutation successfully.
    Ok,
    /// Handler detected a late condition (race, state change, target
    /// despawn) and rolled back. `reason` is a short log-friendly key.
    Rejected { reason: String },
    /// Handler split the command — e.g. auto-inserted MoveTo prefix for
    /// Survey when not at target. No terminal execution yet; another
    /// `CommandExecuted` (or another `CommandRequested`) will follow.
    Deferred,
}
```

**Why per-variant and not `enum CommandRequested { Move(…), Survey(…), … }`**:

1. Bevy's `MessageReader<T>` is typed — a single-enum design forces every handler to `.read()` the full variant stream and match on its own tag, spending cycles and cache lines on messages it ignores. Per-variant types keep the hot path narrow (one handler, one message stream).
2. Adding a new command = adding one struct + one `app.add_message::<X>()` call. No touching an "all commands" enum — preserves open/closed.
3. `CommandExecuted` stays a single enum-tagged message because subscribers (CommandLog, Lua hook bridge) genuinely want the merged stream — the cost of dispatch there is in the subscriber side and is small.

**Downside**: each variant adds an `app.add_message::<X>()` line. Recommend a `CommandEventsPlugin` in `src/ship/command_events.rs` that registers them all so `main.rs` stays quiet. **Acceptable** given we replace nine fat `match` arms with a clean plugin init.

### 2.2 Dispatcher system signature

```rust
pub fn dispatch_queued_commands(
    clock: Res<GameClock>,
    mut next_id: ResMut<NextCommandId>,
    // ONE mutable ship view — peek head, remove on consume.
    mut ships: Query<(Entity, &Ship, &ShipState, &Position, &mut CommandQueue)>,
    // read-only lookups for validation only — no mutation from dispatcher.
    systems: Query<(Entity, &Position), With<StarSystem>>,
    existing_cores: Query<&AtSystem, With<CoreShip>>,
    design_registry: Res<ShipDesignRegistry>,
    // message writers — one per request variant.
    mut move_req: MessageWriter<MoveRequested>,
    mut move_xy_req: MessageWriter<MoveToCoordinatesRequested>,
    mut survey_req: MessageWriter<SurveyRequested>,
    mut colonize_req: MessageWriter<ColonizeRequested>,
    mut scout_req: MessageWriter<ScoutRequested>,
    mut load_req: MessageWriter<LoadDeliverableRequested>,
    mut deploy_req: MessageWriter<DeployDeliverableRequested>,
    mut transfer_req: MessageWriter<TransferToStructureRequested>,
    mut scrap_req: MessageWriter<LoadFromScrapyardRequested>,
    // CommandLog write on dispatch (see §4).
    mut command_log_q: Query<&mut CommandLog, With<PlayerEmpire>>,
) {
    // ~50 lines of peek-head / validate-preconditions / emit-message /
    // pop-from-queue. NO state mutation beyond CommandQueue pop and
    // CommandLog append. Per-variant validation logic is inlined because
    // the rules are small; if a rule balloons it can extract to a
    // `fn validate_move(..) -> Result<MoveRequested, Reject>` helper.
}
```

Param count: **12** (well below the 16-arg cap). The two read queries are needed for tie-break / pre-validation; all writers are `MessageWriter` which is the same `SystemParam` class as the old `&mut` queries but holds zero per-tick cost when empty.

**Critical validation pushed to dispatcher** (see §3):
- Ship exists
- Ship not in FTL mid-transit for commands that require Docked/Loitering
- Target entities exist in systems query
- `ship.is_immobile()` rejection for MoveTo / MoveToCoordinates
- FTL gate + scout module presence for Scout
- Core deploy "already has Core" early check (current `deliverable_ops.rs:248-258`)

**Handler-only validation** (things the dispatcher cannot see without becoming fat again):
- Cargo fit check (needs `Cargo` + `ShipModifiers` + `DeliverableStockpile`)
- Co-location epsilon check for Deploy / Transfer (needs `ConstructionPlatform` / `Scrapyard` positions)
- Colony ship capability (needs `design_registry.can_colonize`)
- Same-tick tie-break for multi-deploy on one system (moved into `resolve_core_deploys` already — unchanged)

### 2.3 Handler signatures (example: MoveTo)

```rust
// macrocosmo/src/ship/handlers/move_handler.rs (new module)

pub fn handle_move_requested(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut reqs: MessageReader<MoveRequested>,
    empire_params_q: Query<&GlobalParams, With<PlayerEmpire>>,
    balance: Res<GameBalance>,
    empire_knowledge_q: Query<&KnowledgeStore, With<PlayerEmpire>>,
    mut ships: Query<(&Ship, &mut ShipState, &Position, Option<&RulesOfEngagement>)>,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    system_buildings: Query<&SystemBuildings>,
    hostiles_q: Query<(&AtSystem, &FactionOwner), With<Hostile>>,
    relations: Res<FactionRelations>,
    mut pending_count: ResMut<RouteCalculationsPending>,
    design_registry: Res<ShipDesignRegistry>,
    building_registry: Res<BuildingRegistry>,
    regions: Query<&ForbiddenRegion>,
    mut executed: MessageWriter<CommandExecuted>,
) {
    // Same logic as process_command_queue's MoveTo arm (command.rs:263-373),
    // refactored to read from reqs instead of peeking a queue head.
    for req in reqs.read() {
        /* … original body, verbatim … */
        executed.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::Move,
            ship: req.ship,
            result: CommandResult::Ok, // or Deferred if async route is spawned
            completed_at: clock.elapsed,
        });
    }
}
```

Param count: **14** (fits, with slack for future needs). Each handler scope is narrow: only MoveTo needs route planner resources; Deploy needs Cargo+Stockpile+Registry; Survey needs `start_survey_with_bonus` supports. The key win: **no handler ever has to pull in every other handler's params**.

`handle_deploy_deliverable_requested` is the biggest handler (because it handles both structure-spawn AND the Core-branch message emit):

```rust
pub fn handle_deploy_deliverable_requested(
    mut commands: Commands,
    clock: Res<GameClock>,
    balance: Res<GameBalance>,
    registry: Res<StructureRegistry>,
    mut reqs: MessageReader<DeployDeliverableRequested>,
    mut core_out: MessageWriter<CoreDeployRequested>,
    mut executed: MessageWriter<CommandExecuted>,
    mut events: MessageWriter<GameEvent>,
    ships: Query<(&Ship, &ShipState, &Position, &mut Cargo)>,
    existing_cores: Query<&AtSystem, With<CoreShip>>,
    star_systems: Query<(Entity, &Position), (Without<Ship>, With<StarSystem>)>,
    mut fact_sys: FactSysParam,
) {
    /* … logic from deliverable_ops.rs:180-323 verbatim … */
}
```

Param count: **12**. Below cap.

### 2.4 System ordering — same-tick delivery

Bevy 0.18's `Messages<T>` buffer is double-buffered per frame. A `MessageWriter` call in system A and a `MessageReader` call in system B **will see the message in the same frame** if B is ordered `.after(A)` (scheduler guarantees this; the message sits in the current-frame buffer until the next `Messages::update` call at end of frame). This is exactly the ordering we need.

Schedule:

```rust
app.add_systems(Update, (
    dispatch_queued_commands,
    (
        handle_move_requested,
        handle_move_to_coordinates_requested,
        handle_survey_requested,
        handle_colonize_requested,
        handle_scout_requested,
        handle_load_deliverable_requested,
        handle_deploy_deliverable_requested,
        handle_transfer_to_structure_requested,
        handle_load_from_scrapyard_requested,
        handle_attack_requested, // future
    ),
    handle_core_deploy_requested, // .after(handle_deploy_deliverable_requested) specifically
    bridge_command_executed_to_log,       // reads CommandExecuted, updates CommandLog
    bridge_command_executed_to_gamestate, // Phase 4, enqueues _pending_script_events
).chain()
 .after(crate::time_system::advance_game_time));
```

The per-handler tuple is **not** `.chain()`'d internally — they're independent and can run in parallel where Bevy's scheduler allows (no query conflicts between them since each has a different handler's query set). `handle_core_deploy_requested` specifically chains after Deploy because the Core message is emitted inside that handler.

---

## §3 Validation error handling policy (Open question 1)

**Decision: dispatcher-side early validation, handler rollback as exception.**

### 3.1 Dispatcher validation — `warn!` + drop, no message emit

For all preconditions the dispatcher can evaluate from its 12-param view, the rule is:

1. Peek queue head
2. Run validation
3. If fail: `warn!(…)`, pop the command (`queue.commands.remove(0)`), **do not emit** a `CommandRequested`.
4. If pass: emit `CommandRequested`, pop the command.

No `CommandExecuted` is emitted on dispatcher-side fail — the command never had an execution phase. `CommandLog` receives a synthetic `rejected: true` entry (see §4) written inline by the dispatcher; this is the one CommandLog write from the dispatcher.

**Checks performed in dispatcher**:

| check | applies to | today's location |
|---|---|---|
| ship exists | all | implicit in today's queries |
| target system exists | MoveTo, Survey, Colonize, Scout, LoadDeliverable | `command.rs:265, 487, 527, 417`, `deliverable_ops.rs:113` |
| ship Docked/Loitering | MoveTo (loitering OK), Deploy, Transfer, Scrap, Survey, Colonize, Scout | `command.rs:247-253`, `deliverable_ops.rs:182-189` |
| ship immobile rejection | MoveTo, MoveToCoordinates | `command.rs:283-291, 379-401` |
| scout FTL + module gate | Scout | `command.rs:425-441` |
| Core "already has Core" early filter | DeployDeliverable (Core branch) | `deliverable_ops.rs:248-258` |
| owner-is-Empire gate | CoreDeploy (Neutral filtered) | `core_deliverable.rs:174` |

### 3.2 Handler rollback — for races only

The handler may still detect failure after the dispatcher passed validation:

- Another handler despawned the target this tick (`systems.get(target).is_err()`).
- Resource state changed (e.g. cargo item consumed by a parallel command).
- Async condition (route planner declares target unreachable).

In these cases the handler:

1. Performs no state mutation (or rolls back partial state).
2. Emits `CommandExecuted { result: CommandResult::Rejected { reason: "target despawned" }, … }`.
3. `warn!` for developer debugging.

This split means **subscribers always see a terminal signal**: every `CommandRequested` yields exactly one `CommandExecuted` (`Ok`, `Rejected`, or `Deferred → later Ok/Rejected`). Dispatcher-side rejections are visible through `CommandLog` but not `CommandExecuted`.

### 3.3 `Deferred` handling (MoveTo async + auto-inserted prefixes)

Two existing patterns need the `CommandResult::Deferred` disposition:

1. **MoveTo async route** (`command.rs:356-372`): `spawn_route_task_full` starts a task and inserts `PendingRoute`; `poll_pending_routes` finalizes later. The handler emits `CommandExecuted { result: Deferred }` on spawn, and the poll system emits the terminal `CommandExecuted { result: Ok/Rejected }` when the route resolves. `command_id` is threaded through `PendingRoute` so the terminal event keys correctly.

2. **Auto-inserted MoveTo prefix** for Survey/Colonize/Scout when ship not at target (`command.rs:443-462, 494-499, 531-535`): handler emits `CommandExecuted { result: Deferred }`, re-inserts the original command back at the head of the queue, **and** inserts a MoveTo in front. The dispatcher picks those up next tick as fresh `MoveRequested` + `SurveyRequested`. The **new `command_id` for the re-inserted Survey** is the same as the original (so `CommandLog` shows one entry); this is plumbed via a `CommandQueue` field extension (`QueuedCommand` gains a `Option<CommandId>` — see §7 Phase 1 migration detail).

---

## §4 CommandLog 記録箇所 (Open question 2)

**Decision: two-phase record keyed by `CommandId`.**

### 4.1 Schema change

```rust
pub struct CommandLogEntry {
    pub command_id: Option<CommandId>, // None for legacy remote messages
    pub description: String,
    pub sent_at: i64,
    pub arrives_at: i64,    // equals sent_at for local (non-remote) cmds
    pub dispatched_at: Option<i64>,
    pub executed_at: Option<i64>,
    pub status: CommandLogStatus,
}

pub enum CommandLogStatus {
    Pending,      // remote cmd in flight
    Dispatched,   // dispatcher emitted CommandRequested
    Executed,     // handler emitted CommandExecuted::Ok
    Rejected { reason: String },
    Deferred,     // CommandExecuted::Deferred, awaiting follow-up
}
```

### 4.2 Write sites

1. **Remote command send** (existing `send_remote_command`, `mod.rs:397-418`): status = `Pending`, `command_id = None` (or allocated at send — TBD, #268 may need it). No change in Phase 1-3; Phase 4 unifies.
2. **Dispatcher** (new): on successful validation → append entry with `status = Dispatched`, `command_id = Some(id)`, `dispatched_at = clock.elapsed`. On validation fail → append with `status = Rejected { reason }`, `executed_at = clock.elapsed`.
3. **Handler bridge system** `bridge_command_executed_to_log`: reads `MessageReader<CommandExecuted>`, looks up entry by `command_id`, updates `status` + `executed_at`.

Bridge system signature:

```rust
pub fn bridge_command_executed_to_log(
    mut executed: MessageReader<CommandExecuted>,
    mut log_q: Query<&mut CommandLog, With<PlayerEmpire>>,
) {
    let Ok(mut log) = log_q.single_mut() else { return; };
    for event in executed.read() {
        if let Some(entry) = log.entries.iter_mut().find(|e| e.command_id == Some(event.command_id)) {
            entry.executed_at = Some(event.completed_at);
            entry.status = match &event.result {
                CommandResult::Ok => CommandLogStatus::Executed,
                CommandResult::Rejected { reason } => CommandLogStatus::Rejected { reason: reason.clone() },
                CommandResult::Deferred => CommandLogStatus::Deferred,
            };
        }
    }
}
```

**Timing gap risk**: the bridge runs `.after(all handlers)`, so within a single `Update` pass a command's `Dispatched` → `Executed` transition is atomic from the player's perspective (the UI reads `CommandLog` in the `EguiPrimaryContextPass` schedule, which runs after `Update`). No visible intermediate state.

**Remote command bridging** (Phase 4): once a remote `PendingCommand` arrives at the target system, its execution should push a `CommandRequested` through the same pipeline. That integration lives under #302/#321 diplomacy work; this plan only commits to not breaking the existing flow.

---

## §5 Observer mode / faction filter (Open question 3)

**Decision: events are emitted for all factions unfiltered; faction filter happens at consumers.**

### 5.1 Rationale

- Knowledge propagation (#175/#176) is the proper place to hide other factions' commands from the player — not the command pipeline itself.
- NPC-empire commands (#173) run through the same dispatcher/handler path. If we filtered events by faction at emit time, AI tooling and observer-mode replay would go dark.
- The event bus is an engine-internal channel. UIs, log panels, and Lua hooks each decide whether to subscribe to all factions or just the player's empire. This matches how `GameEvent` + `KnowledgeStore` relate today.

### 5.2 Observer mode debug telemetry

`ui/ai_debug/` already consumes cross-faction signals; extending the AI debug stream to subscribe to all `CommandExecuted` messages is a one-system addition (a `MessageReader<CommandExecuted>` in `stream.rs`). Out of scope for this plan but explicitly enabled by the design.

### 5.3 Player-facing filter point

`CommandLog` is attached to `PlayerEmpire`. The bridge system writes only to the player's log; NPC commands trigger `CommandExecuted` but not `CommandLog` entries. Knowledge propagation for NPC commands happens through the existing `FactSysParam` dual-writes (already in the handlers — we're moving code, not changing it).

---

## §6 同 tick 多重 command の順序 (Open question 4)

**Decision: FIFO at dispatcher emit time is sufficient.**

### 6.1 Per-ship FIFO via single dispatcher

`dispatch_queued_commands` is a single system that iterates `ships.iter_mut()` and peeks one command per ship per frame. Each ship's queue is drained sequentially, so the per-ship order matches the pre-refactor behaviour exactly.

### 6.2 Cross-ship order and `MessageReader` guarantees

Bevy's `MessageReader` iterates messages **in the order they were emitted** (confirmed by `bevy_ecs::message` docs: the internal buffer is a `Vec`, not a priority queue; `iter()` and `drain()` are FIFO). So if ship A emits `MoveRequested` before ship B in the same dispatcher iteration, the `handle_move_requested` handler processes A then B.

**Spec verification needed at Phase 1 spike**: add a test `test_message_reader_preserves_emit_order` in `tests/` that writes 100 `MoveRequested` messages with distinct `command_id`s and asserts the reader sees them in order. Lock the assumption before it becomes implicit across 10 handlers.

### 6.3 Same-system tie-break for Core deploys

Unchanged from #296. `handle_core_deploy_requested` reads **all** `CoreDeployRequested` messages for the current frame, groups by `target_system`, runs `GameRng` tie-break, spawns the winner. The grouping happens entirely inside the handler, decoupled from message ordering.

---

## §7 Migration phase plan

### Phase 1 — Event skeleton + MoveTo

**Scope**: define `command_events.rs`, register plugin, migrate `MoveTo` + `MoveToCoordinates` only. All other variants remain in `process_deliverable_commands` / `process_command_queue` unchanged.

**Commits (4 commits, ~500 LoC)**:

1. `[334] add command_events module + CommandId + CommandKind + CommandResult`
   - New file `macrocosmo/src/ship/command_events.rs` (~200 LoC).
   - `NextCommandId` resource + `CommandEventsPlugin` registering all message types (even pre-declared — cheap to list, avoids churn as later phases land).
   - Unit tests for `CommandId` monotonicity.
2. `[334] dispatcher skeleton + MoveRequested / MoveToCoordinatesRequested`
   - New file `macrocosmo/src/ship/dispatcher.rs` (~150 LoC).
   - Dispatcher emits only `MoveRequested` / `MoveToCoordinatesRequested` for those two variants; other variants fall through to the old `process_command_queue` untouched.
   - Wire into `ShipPlugin` ordered before `process_command_queue`.
3. `[334] handle_move_requested + handle_move_to_coordinates_requested`
   - Extract MoveTo logic from `process_command_queue` into `macrocosmo/src/ship/handlers/move_handler.rs`.
   - Remove matching arms from `process_command_queue` (keep the other variants!).
   - Thread `command_id` through `PendingRoute` so `poll_pending_routes` can emit terminal `CommandExecuted`.
4. `[334] CommandLog 2-phase status + bridge_command_executed_to_log`
   - Extend `CommandLogEntry` with optional `command_id` and `status` enum (backwards-compatible default).
   - Bridge system registered in `CommunicationPlugin`.
   - Integration test: dispatch a MoveTo, assert `CommandLog.entries.last().status == Executed` within the same tick, `command_id` matches.

**PR split**: single PR (atomic; touches `command_events` + MoveTo path together). Regression risk concentrated in the MoveTo flow — already the most-tested movement command.

**LoC estimate**: +~700, -~200 (removed dispatching code from `process_command_queue`).

**Acceptance tests** (all must pass):
- `full_test_app()` with no query conflicts.
- `all_systems_no_query_conflict` integration test.
- Existing `tests/movement.rs` + `tests/routing.rs` + remote MoveTo tests (`tests/communication.rs`) unchanged green.
- New test: `test_move_to_dispatcher_emits_request`, `test_move_to_command_id_preserved_through_pending_route`, `test_move_to_immobile_rejected_by_dispatcher`.

### Phase 2 — DeployDeliverable + Core deploy + settlement commands

**Scope**: migrate `DeployDeliverable`, `LoadDeliverable`, `TransferToStructure`, `LoadFromScrapyard`, `Colonize`, `Survey`. Convert `PendingCoreDeploys` resource → `CoreDeployRequested` message; `resolve_core_deploys` becomes `handle_core_deploy_requested` reading a `MessageReader`. **Delete the `PendingCoreDeploys` resource entirely.**

**Commits (5 commits, ~900 LoC)**:

1. `[334] dispatcher: emit all deliverable + settlement variant requests`
2. `[334] handle_deploy_deliverable_requested (with Core branch emitting CoreDeployRequested)`
3. `[334] handle_core_deploy_requested — replace PendingCoreDeploys resource`
   - Remove `PendingCoreDeploys` resource and its `init_resource` in `ShipPlugin`.
   - Renamed `resolve_core_deploys` to `handle_core_deploy_requested`, reads `MessageReader<CoreDeployRequested>` instead of `ResMut<PendingCoreDeploys>`.
   - Same tie-break + GameRng logic, verbatim.
4. `[334] handle_load_deliverable + handle_transfer + handle_scrap + handle_survey + handle_colonize`
5. `[334] shrink process_deliverable_commands to deleted shell + remove dead variant arms from process_command_queue`

**PR split**: can be one large PR, or split 3+2 commits into two PRs (deliverables first, then settlement + core). Recommend **one PR** because Phase 2 has high semantic coherence; reviewer mental load is easier than splitting.

**LoC estimate**: +~900, -~700 (delete `process_deliverable_commands` body and the `match` arms in `process_command_queue`).

**Critical regression tests**:
- `tests/infrastructure_core.rs` — all Core deploy tests (§10 of #296 plan) must pass without `PendingCoreDeploys`.
- `tests/deliverable_ops.rs` (if present — grep confirms this is in unit tests within `deliverable_ops.rs`).
- `tests/settlement.rs` colonize flow.
- Fixture `minimal_game.bin` (committed fixture) — `PendingCoreDeploys` is not persisted (it's a transient per-tick resource), so the save format is **not** affected. Confirm in `persistence/savebag.rs:856-869` and note in PR.

### Phase 3 — Scout + attack scaffolding + dispatcher-only `process_command_queue`

**Scope**: migrate Scout; add `AttackRequested` skeleton for #219/#220 adoption (no handler yet — just message type + dispatcher arm if we have a QueuedCommand::Attack to migrate, otherwise defer). Delete `process_command_queue` (all variants now handled elsewhere); delete `process_deliverable_commands`.

**Commits (3 commits, ~300 LoC)**:

1. `[334] handle_scout_requested + remove Scout from process_command_queue`
2. `[334] delete process_command_queue + process_deliverable_commands entirely`
   - The two systems' mod registrations in `ShipPlugin` collapse to the dispatcher + handler set.
3. `[334] AttackRequested skeleton (no handler) for #219/#220 foundation`

**PR split**: single PR, relatively small.

**LoC**: +~200, -~500.

### Phase 4 — CommandExecuted → gamestate / Lua bridge (depends on #332 merged)

**Scope**: add `bridge_command_executed_to_gamestate` that reads `MessageReader<CommandExecuted>` and enqueues `_pending_script_events` entries for the existing `fire_event` dispatch loop to pick up next tick. This gives Lua scripts an `on_command_completed(cmd)` hook without synchronous callback from the handler system — preserving the queue-only reentrancy invariant.

**Commits (3 commits, ~400 LoC)**:

1. `[334] bridge_command_executed_to_gamestate (queue-only enqueue)`
   - Reads `MessageReader<CommandExecuted>`.
   - Builds an event-like payload keyed by `command_id` + `kind` + `ship`.
   - Enqueues `_pending_script_events` (same path existing fire_event queue uses).
2. `[334] Lua API: ctx.gamestate:request_command(kind, args) → emit CommandRequested`
   - Gamestate scope closure gets a new setter (`create_function_mut` with `&mut World` handle).
   - Rust side: construct the appropriate `CommandRequested` struct and write it via `World::send_message` inside the closure.
   - Opens the door to diplomacy v2 setters (#302/#321) and Lua-side AI (#173).
3. `[334] docs: command lifecycle diagram + Lua hook example script`

**PR split**: single PR, gated on #332 merge (which is already merged as of 2026-04-15). Should land together with the first diplomacy-v2 hook consumer so we can exercise the full path end-to-end.

**LoC**: +~400, -~0.

### Phase summary table

| Phase | Commits | LoC (+/-) | PR count | Prereqs |
|---|---|---|---|---|
| 1 | 4 | +700 / -200 | 1 | none |
| 2 | 5 | +900 / -700 | 1 | Phase 1 merged |
| 3 | 3 | +200 / -500 | 1 | Phase 2 merged |
| 4 | 3 | +400 / 0 | 1 | Phase 3 merged + #332 merged (done) |
| **Total** | **15** | **+2200 / -1400** (net +~800) | **4 PRs** | |

Each phase is **independently landable and shippable**: no intermediate commit leaves the tree in a broken state, because we're adding parallel infrastructure and swapping variants one at a time.

---

## §8 #268 (Courier opportunistic relay) との統合

### 8.1 Command ID dedup is the dispatcher's job

`CommandId` allocated by `NextCommandId` in the dispatcher (§2.1) is the dedup key #268 needs. When a courier ship reaches a system and opportunistically relays pending commands, the relay path must not re-dispatch commands already in flight or already executed.

**Algorithm**:

1. Relay picks up a bundle of `PendingCommand` components to forward (light-speed catch-up).
2. At delivery, before enqueueing a new `CommandRequested`, check `CommandLog` or a new `DispatchedCommandIds: HashSet<CommandId>` resource for prior dispatch.
3. If already dispatched: drop the relay copy with `trace!` log (normal case — light-speed msg + relay both arrived).
4. If new: proceed through the dispatcher normally, allocating a fresh `CommandId` tied to the relay source.

**Data structure**:

```rust
#[derive(Resource, Default)]
pub struct DispatchedCommandIds {
    // cleared at end of frame or bounded by retention policy
    ids: HashSet<CommandId>,
}
```

Alternative: allocate `CommandId` at `send_remote_command` time (dispatcher-side of the remote pipeline), thread it through `PendingCommand`, and the delivery-side dispatcher simply checks its own `CommandLog`. This is cleaner but requires reshuffling remote-command plumbing — **defer to #268 PR itself**, don't prescribe.

### 8.2 Dependency registration

This plan recommends registering **#268 blocked-by #334 Phase 1** (so #268 implementor can assume `CommandId` exists). Mark Phase 2 as the point where `PendingCoreDeploys` is deleted — #268 can then freely use the message bus without worrying about intermediate-resource patterns proliferating.

---

## §9 Diplomacy v2 setter (#302/#321) との統合

### 9.1 Three-layer command pipeline

Phase 4 enables the clean pattern diplomacy v2 needs:

1. **Lua setter**: `ctx.gamestate:declare_war(other_empire, reason)` — a scope closure `create_function_mut` that constructs a Rust-side `CommandRequested::DeclareWar { … }` struct and writes it via `World::send_message`.
2. **Rust dispatcher**: this is not `dispatch_queued_commands` but a diplomacy-specific dispatcher (out of scope for this plan). Alternative: reuse the same dispatcher if `QueuedCommand` gains diplomacy variants. **Recommendation**: keep `QueuedCommand` for ship actions and introduce a parallel `EmpireCommand` enum + dispatcher for empire-level actions, with the **same message-event pattern**. The plumbing is identical; the entity target differs.
3. **Handler**: `handle_declare_war_requested` mutates faction relations, emits `CommandExecuted`.
4. **Subscribers**: UI log, Lua `on_command_completed` hook, knowledge propagation.

The three-layer story (setter → dispatcher → handler, plus post-event subscribers) maps directly to what diplomacy v2 already wants. This plan's deliverables make that pattern **the established one in the codebase**, so #321 doesn't need to invent new machinery.

### 9.2 Setter reentrancy safety

Per `feedback_rust_no_lua_callback.md`: the Lua-side `request_command` setter is a `create_function_mut` that **does not call back into Lua**. It constructs a pure Rust struct and emits a message — no `Function::call`, no stored `RegistryKey` invocation. The handler (normal Bevy system) writes the terminal `CommandExecuted`, which a separate bridge system enqueues into `_pending_script_events` for the next-tick fire_event loop to deliver to `on_command_completed` Lua handlers.

This means the diplomacy Lua hook loop is:

```
Lua event handler
  → ctx.gamestate:declare_war(x)        -- write closure emits CommandRequested
  → Rust dispatcher handler runs       -- mutates state, emits CommandExecuted
  → bridge enqueues _pending_script_events
  → next tick: fire_event dispatches on_command_completed to Lua
  → Lua handler reads ctx.gamestate.empires[x].relations (live via scope closure)
```

No synchronous callback, no reentrancy. Matches #332's scope-closure invariant exactly.

---

## §10 リスク表

| # | risk | likelihood | impact | mitigation |
|---|---|---|---|---|
| R1 | `Messages<T>` same-frame delivery semantics break with scheduler change | low | high | Add `test_message_same_frame_visible` in Phase 1; document Bevy 0.18 version dependency |
| R2 | Dispatcher hits 16-arg cap as #268 + #219 + #220 add validation reads | medium | medium | Dispatcher starts with slack (12 args); plan for `DispatcherValidationParams` SystemParam bundle when cap approached. Note: dispatcher's per-tick cost is dominated by the ship-queue peek, not param count, so splitting the dispatcher by variant group (movement vs cargo vs combat) is the escape hatch if needed |
| R3 | 2-phase CommandLog timing gap visible to UI | low | low | UI reads in `EguiPrimaryContextPass` which runs after `Update`; entries transition atomically from the player's frame |
| R4 | Tests depending on `process_deliverable_commands` direct run order break | medium | medium | Phase 2 keeps the old systems alive for one phase, deletes in Phase 3 once all tests migrate to handler-based assertions. Prefer integration test that asserts `CommandExecuted` result over "system X mutated component Y". |
| R5 | B0001 query conflict: `handle_move_requested` and `handle_transfer_to_structure_requested` both need ship mutable access | medium | medium | Each handler takes `Query<(&Ship, &mut ShipState)>` or `Query<(&Ship, &mut Cargo)>` — different mutable components, no conflict. Run `full_test_app()` at every commit. Detected early by `all_systems_no_query_conflict` integration test. |
| R6 | `CommandId` wraps on a 292M-year game (u64 increments per dispatched command) | neglectable | low | Not a real concern |
| R7 | Saved game + mid-upgrade fixture: `CommandRequested` in-flight messages not in save | high | low | Messages are not saved (they're frame-transient buffers). If a save is taken mid-frame between dispatcher and handler, the command is lost. **This is already the behaviour for `PendingRoute` tasks today.** `tests/fixtures_smoke.rs` `minimal_game.bin` fixture should remain green; confirm in Phase 2 PR. |
| R8 | Lua bridge double-fires `on_command_completed` on `Deferred` → `Ok` pairs | medium | low | Bridge filters `Deferred` results, only enqueues on terminal `Ok`/`Rejected`. Test: `test_deferred_does_not_fire_lua_hook` |
| R9 | Merge conflict with open #328 or other in-flight ship-module work | medium | low | Phase 1 touches `ship/command.rs` + `ship/mod.rs` plugin wiring — coordinate with any open PR touching those files. Run `cargo test` after merge (per `feedback_semantic_merge_conflict.md`). |
| R10 | `CommandLog` description format change breaks existing UI assertions | low | low | Keep `description: String` field intact; add new fields behind `Option<>` so `Default` impls in tests don't break |

---

## §11 Test plan

### 11.1 Pattern: event-emit → handler-run → effect-observed

Canonical integration-test shape for the new pipeline:

```rust
#[test]
fn test_move_to_dispatcher_emits_request_and_handler_consumes() {
    let mut app = full_test_app();
    let ship = spawn_test_ship(&mut app, /* … */);
    let target = spawn_test_system(&mut app, /* … */);
    // enqueue directly (skip UI)
    app.world_mut().entity_mut(ship).get_mut::<CommandQueue>().unwrap()
        .commands.push(QueuedCommand::MoveTo { system: target });
    app.update(); // one tick: dispatcher emits, handler consumes
    // assert CommandRequested was emitted
    let msgs = app.world().resource::<Messages<MoveRequested>>();
    assert_eq!(msgs.len(), 1);
    // assert handler ran (ship state transitioned)
    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(matches!(state, ShipState::Ftl { .. } | ShipState::SubLight { .. }));
    // assert CommandExecuted recorded Ok
    let executed: Vec<_> = app.world().resource::<Messages<CommandExecuted>>().iter_current_update_events().collect();
    assert_eq!(executed.len(), 1);
    assert!(matches!(executed[0].result, CommandResult::Ok | CommandResult::Deferred));
}
```

### 11.2 Regression matrix per phase

| phase | required tests |
|---|---|
| 1 | MoveTo: ftl happy path, ftl→sublight fallback, immobile reject, route async happy, route async fail. MoveToCoordinates: sublight happy, immobile reject |
| 2 | DeployDeliverable: structure spawn, Core branch with valid target, Core deep-space self-destruct, Core already-has-core reject, same-tick tie-break. Load/Transfer/Scrap: existing tests should pass with no code change beyond plumbing. Survey: start+complete flow, not-at-target auto-MoveTo prefix. Colonize: start+complete flow |
| 3 | Scout: not-at-target auto-MoveTo prefix, FTL gate, module gate, observation→report flow. Legacy `process_command_queue` / `process_deliverable_commands` deletion: grep must return zero matches; all `ShipPlugin` tests still green |
| 4 | Lua `on_command_completed` fires once per terminal result; Deferred does not fire; `ctx.gamestate:request_command` setter emits correct message; reentrancy test (command → hook → another command → hook) |

### 11.3 Runtime query-conflict detection

Use `full_test_app()` in a Phase 1 regression test:

```rust
#[test]
fn all_command_handler_systems_run_without_query_conflict() {
    let mut app = full_test_app();
    for _ in 0..5 { app.update(); } // exercise scheduler
}
```

This is the canonical B0001 detector — already used in the codebase (see CLAUDE.md "Query conflicts"). Each phase PR must include this assertion passing.

### 11.4 Save-file fixture guard

`tests/fixtures_smoke.rs` `load_minimal_game_fixture_smoke` — as long as no field in `SavedComponentBag` changes, this stays green. The refactor intentionally avoids touching save format:

- `PendingCoreDeploys` was not in savebag (verified grep on `savebag.rs`).
- `CommandLog`'s new `command_id`/`status` fields should be `#[serde(default)]` on the persistence path (or excluded entirely if CommandLog isn't persisted — confirm in Phase 1).

If Phase 1 PR intends to persist `CommandLog` with the new schema, bump `SAVE_VERSION` and regenerate the fixture per `CLAUDE.md` "Save-file Fixtures" procedure.

---

## §12 Out of scope

- **Exclusive system (`&mut World`) migration**: rejected in the issue itself. Handlers remain normal parallel systems.
- **Handler trait-object / registry (Option D)**: rejected. Per-variant functions wired into `ShipPlugin` is simpler and matches Bevy idioms.
- **Command semantics changes**: movement, deploy, survey, colonize, scout, transfer, scrap, attack behave identically pre- and post-refactor. Every behavioural change this plan introduces (CommandLog 2-phase status, `CommandId`, `CommandExecuted`) is additive / observational.
- **Remote (light-speed) command unification with local dispatch**: Phase 4 opens the door (diplomacy v2 path) but doesn't require migrating `PendingCommand` / `send_remote_command` flow to message buses. That lives under #302/#321.
- **UI refactor** to consume `CommandExecuted` stream: current UI reads `CommandLog` which we preserve.
- **Event-sourcing / command replay**: message buffers are frame-local; full replay requires persistence that's out of scope.

---

## §13 Critical files for implementation

### New files

- `macrocosmo/src/ship/command_events.rs` — message type definitions, `CommandId`, `NextCommandId`, `CommandEventsPlugin`.
- `macrocosmo/src/ship/dispatcher.rs` — `dispatch_queued_commands` system.
- `macrocosmo/src/ship/handlers/mod.rs` — re-export module.
- `macrocosmo/src/ship/handlers/move_handler.rs` — `handle_move_requested`, `handle_move_to_coordinates_requested`.
- `macrocosmo/src/ship/handlers/deploy_handler.rs` — `handle_deploy_deliverable_requested`, `handle_core_deploy_requested`.
- `macrocosmo/src/ship/handlers/cargo_handler.rs` — Load / Transfer / Scrap.
- `macrocosmo/src/ship/handlers/settlement_handler.rs` — Survey / Colonize.
- `macrocosmo/src/ship/handlers/scout_handler.rs` — Scout.
- `macrocosmo/src/ship/bridges.rs` — `bridge_command_executed_to_log`, Phase 4 `bridge_command_executed_to_gamestate`.
- `macrocosmo/tests/command_pipeline.rs` — new integration-test file.

### Modified files

- `macrocosmo/src/ship/mod.rs` — `ShipPlugin::build` schedule overhaul (replace `process_command_queue` / `process_deliverable_commands` / `resolve_core_deploys` with dispatcher + handler wiring). `QueuedCommand` enum possibly extended with an internal `command_id: Option<CommandId>` to track auto-inserted re-queues.
- `macrocosmo/src/ship/command.rs` — `process_command_queue` shrinks across phases, deletion in Phase 3.
- `macrocosmo/src/ship/deliverable_ops.rs` — `process_deliverable_commands` shrinks in Phase 2, deletion in Phase 3.
- `macrocosmo/src/ship/core_deliverable.rs` — delete `PendingCoreDeploys` resource (Phase 2); `resolve_core_deploys` renamed + rewired to `MessageReader` (Phase 2).
- `macrocosmo/src/ship/routing.rs` — thread `command_id` through `PendingRoute`; `poll_pending_routes` emits terminal `CommandExecuted`.
- `macrocosmo/src/ship/command.rs` — `process_pending_ship_commands` remains for remote command arrival; Phase 4 bridges into the local dispatcher instead of inlining state mutation.
- `macrocosmo/src/communication/mod.rs` — `CommandLogEntry` schema extension; `dispatch_pending_colony_commands` and `process_pending_commands` unchanged in Phase 1-3.
- `macrocosmo/src/ship/movement.rs`, `macrocosmo/src/ship/survey.rs`, `macrocosmo/src/ship/settlement.rs`, `macrocosmo/src/ship/scout.rs` — handler delegations. The `start_*` helpers remain and are called from handlers.
- `macrocosmo/src/scripting/lifecycle.rs` — Phase 4: consume `_pending_script_events` path for `on_command_completed` hook, keyed by bridge system.
- `macrocosmo/src/main.rs` — register `CommandEventsPlugin` before `ShipPlugin` (Phase 1).
- `macrocosmo/tests/infrastructure_core.rs` — update Core deploy tests to assert via `MessageReader<CoreDeployRequested>` instead of `PendingCoreDeploys` resource (Phase 2).

### Files that should NOT change

- `macrocosmo/src/ship/fleet.rs` — fleet membership unrelated.
- `macrocosmo/src/ship/combat.rs` — combat remains a separate system for now; `AttackRequested` skeleton is additive, combat resolution logic unchanged.
- `macrocosmo/src/persistence/savebag.rs` — no save-format changes (verified: `PendingCoreDeploys` never persisted; `CommandLog` changes use `serde(default)`).
- `macrocosmo/assets/shaders/territory.wgsl` — visualization unrelated.
- `macrocosmo/scripts/**` — no Lua changes in Phases 1-3; Phase 4 adds `on_command_completed` hook examples but doesn't alter existing scripts.

---

## Appendix A — Decision log summary

| Open question | Decision | §  |
|---|---|---|
| Validation fail location | Dispatcher-side early (`warn!` + drop), handler rollback for races only | §3 |
| CommandLog write site | Two-phase, keyed by `CommandId`: dispatcher writes `Dispatched`, bridge system writes `Executed/Rejected/Deferred` | §4 |
| Observer mode event filter | No filter at emit; subscribers filter by faction/empire | §5 |
| Same-tick command order | FIFO via single dispatcher + `MessageReader` iteration order (verify in Phase 1 test) | §6 |
| One-enum vs. per-variant messages | Per-variant messages; single-enum `CommandExecuted` for post-events | §2.1 |
| `PendingCoreDeploys` fate | Delete in Phase 2, replaced by `CoreDeployRequested` message | §1.5 / §7 |
| `CommandId` allocation point | Dispatcher allocates via `NextCommandId` resource | §2.1 / §8 |

## Appendix B — Phase 1 PR-ready checklist

- [ ] `command_events.rs` + `CommandEventsPlugin` lands, all 10 message types registered (even those with no handler yet).
- [ ] `dispatch_queued_commands` handles only MoveTo + MoveToCoordinates; other variants fall through to old paths (test: dispatcher does nothing for `QueuedCommand::Survey`).
- [ ] `handle_move_requested` replaces the MoveTo arm in `process_command_queue` (the arm is deleted, not just gated).
- [ ] `CommandLog` schema extended; bridge system registered.
- [ ] New test file `tests/command_pipeline.rs` with ≥5 integration tests per §11.1.
- [ ] `all_systems_no_query_conflict` passes.
- [ ] `cargo test` all green (fixtures fixture included).
- [ ] No warnings introduced (run `cargo clippy -- -D warnings`).
- [ ] Commit count ≤ 4, total diff ≤ 1000 LoC.

---

## Appendix C — Phase 4 landed (2026-04-15)

All four phases are now merged. #334 is ready to close.

**Commits**:

* **Commit 1** — `bridge_command_executed_to_gamestate` (queue-only).
  Adds `CommandCompletedContext: EventContext`, registers the
  `COMMAND_COMPLETED_EVENT = "macrocosmo:command_completed"` constant,
  and wires the bridge after every handler / `poll_pending_routes`
  alongside the Phase 1 `bridge_command_executed_to_log`. Deferred
  results are filtered per §10 R8. **345 LoC net.**
* **Commit 2** — `ctx.gamestate:request_command(kind, args)`.
  Adds `apply::ParsedRequest` + `apply::parse_request` +
  `apply::request_command` (no `&Lua`, no Lua call). Exposes the
  setter on `ReadWrite` gamestates only. Supports 9 kinds; rejects
  `core_deploy` / `attack` with a diagnostic until their Rust
  handlers land. 6 unit tests + 4 integration tests. **931 LoC.**
* **Commit 3** — docs & example. Adds §10bis to
  `docs/architecture-decisions.md`, creates
  `scripts/examples/on_command_completed.lua` (docs-only; not
  loaded by `init.lua`), and this Phase 4 landed note.

**Plan deviations**:

* `core_deploy` and `attack` kinds are **not** exposed via the Lua
  setter yet. `core_deploy` is produced inside the deliverable
  pipeline (not standalone), so exposing it would invert the
  dependency; `attack` is still skeleton-only (#219/#220). Both
  raise a `not yet supported` `RuntimeError` so modders get a clean
  diagnostic.
* The hook transport is `EventSystem::fire_event_with_payload` rather
  than direct `_pending_script_events` push. The queue discipline is
  identical (fired_log → next-tick `dispatch_event_handlers`), but
  `fire_event_with_payload` carries a typed
  `EventContext` so the hook observes structured fields
  (`command_id`, `kind`, `ship`, `result`, `reason`, `completed_at`)
  rather than just `event_id` + `target`. This matches the
  `BUILDING_BUILT_EVENT` convention (#281).
* Integration tests run against a handler surrogate rather than the
  real `handle_move_requested` — the bridge + Lua API path is
  orthogonal to the FTL router, and pulling in `handle_move_requested`
  would require a galaxy / empire / design-registry fixture that
  dominates the test.

**Invariant audit**:

* `apply::request_command` signature: `(&mut World, ParsedRequest) -> mlua::Result<u64>`. No `&Lua` parameter.
* Scope-closure body for `request_command` uses `_lua` (underscored unused).
* `bridge_command_executed_to_gamestate` is a pure Bevy reader; no Lua access.
* `spike_mlua_scope` test 4/4 passes (regression guard for scope/UserData/borrow discipline).
* `all_systems_no_query_conflict` passes.
* `load_minimal_game_fixture_smoke` passes (save format untouched).

---

_End of plan._
