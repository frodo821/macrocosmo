# Codebase Scan Report: 2026-04-26

## Scope

Current repository scan for potential defects and maintenance risks.

Commands run:

- `git status --short`
- `SCCACHE_DISABLE=1 RUSTC_WRAPPER= cargo test --workspace --all-targets`
- `SCCACHE_DISABLE=1 RUSTC_WRAPPER= cargo clippy --workspace --all-targets -- -W clippy::all`
- Targeted source scans with `rg` for panic-prone patterns, unchecked results, casts, TODO/FIXME markers, and AI command outbox paths.

No code changes were made during the scan.

## Summary

The workspace is mostly healthy from a compile perspective, but one integration test currently fails. The highest-risk area is the AI command light-speed delay pipeline: a regression test fails, and adjacent code suggests the outbox delay can undermine the in-flight survey dedup mechanism.

`clippy` completes successfully, but the repository emits a large warning volume. Most warnings are cleanup-level, but the volume makes real regressions harder to spot.

## Findings

### 1. Failing Test: AI Survey Command Light-Speed Delay

Severity: High  
Status: Reproduced

`cargo test --workspace --all-targets` fails in:

```text
macrocosmo/tests/ai_command_lightspeed.rs::survey_command_outbox_holds_until_light_delay_elapses
```

Failure message:

```text
no SurveyRequested fired in the final Update even after waiting 305 hexadies past light delay - outbox is over-gating
```

Relevant files:

- `macrocosmo/tests/ai_command_lightspeed.rs`
- `macrocosmo/src/ai/command_outbox.rs`
- `macrocosmo/src/ai/command_consumer.rs`
- `macrocosmo/src/physics/mod.rs`

The test advances `light_delay + 5` ticks, then checks only the most recent `Update` window for `SurveyRequested`. For a 5 ly distance, `light_delay_hexadies(5.0)` is 300, so the command may have been released during one of the earlier updates in that loop and no longer be visible in the final update's current-message iterator.

That means this test failure may be a test-observation bug rather than proof that the outbox never releases the command.

#### Fix Proposal

Change the test to observe the full post-threshold window instead of only the final update.

Suggested shape:

1. Drain/update the `SurveyRequested` messages before crossing the threshold.
2. Advance one tick at a time.
3. Accumulate whether any `SurveyRequested` appears after `light_delay`.
4. Assert that the accumulated count is non-zero.

Also consider asserting the outbox state directly:

- Before threshold: outbox contains at least one pending `survey_system` command.
- After threshold: the relevant command leaves `AiCommandOutbox`.

This would distinguish "outbox never released" from "event fired but the test missed the update window."

### 2. AI Survey Dedup May Not Cover Commands Still in Outbox

Severity: Medium  
Status: Likely risk from code inspection

The AI survey dedup logic relies on `PendingAssignment`:

- `npc_decision_tick` excludes systems already covered by `PendingAssignment`.
- `handle_survey_system` inserts `PendingAssignment` when the command is consumed.
- With the light-speed outbox, emitted AI commands are delayed before `handle_survey_system` runs.

This creates a gap: while a `survey_system` command is still in `AiCommandOutbox`, the ship and target may still look idle/unassigned to `npc_decision_tick`, so the same survey order can be emitted again on later ticks.

Relevant files:

- `macrocosmo/src/ai/npc_decision.rs`
- `macrocosmo/src/ai/command_outbox.rs`
- `macrocosmo/src/ai/command_consumer.rs`
- `macrocosmo/src/ai/assignments.rs`

#### Fix Proposal

Move or duplicate the assignment marker earlier in the pipeline.

Preferred options:

1. Insert a lightweight "in-flight AI command" marker during `dispatch_ai_pending_commands`.
   - Pros: dedup starts as soon as the command enters the outbox.
   - Cons: `dispatch_ai_pending_commands` currently only has broad command routing context; it would need enough command-param parsing to identify survey ship and target.

2. Extend `npc_decision_tick` to also inspect `AiCommandOutbox`.
   - Pros: avoids new ECS markers.
   - Cons: couples decision logic to outbox internals and requires parsing queued command params every tick.

3. Add a dedicated outbox query/helper such as `pending_survey_assignments_for_faction`.
   - Pros: keeps parsing centralized and lets decision logic stay readable.
   - Cons: still adds a dependency from decision logic to outbox state.

Recommended path: option 3. Keep the helper in `command_outbox.rs`, return `(ship, target_system)` pairs for pending `survey_system` commands, and merge those into `pending_assigned_ships` / `pending_survey_targets` in `npc_decision_tick`.

Add a regression test that advances several ticks below the light-speed threshold and asserts the outbox contains only one pending survey command for the target.

### 3. Warning Volume Is High

Severity: Low  
Status: Reproduced

`cargo clippy --workspace --all-targets -- -W clippy::all` completes, but emits a large number of warnings. Common categories:

- Unused imports and variables.
- Deprecated egui APIs such as `screen_rect`, `close_menu`, and `Frame::rounding`.
- `unused_must_use` in tests and helper systems.
- High `type_complexity` / `too_many_arguments` around Bevy systems.
- Cleanup suggestions such as collapsible `if`, `manual_clamp`, `get_first`, and redundant borrows.

Most of these are not immediate bugs, but the volume reduces the signal value of warnings and makes future regressions harder to notice.

#### Fix Proposal

Do not try to fix all warnings at once. Use a staged cleanup:

1. Fix `unused_must_use` first, especially `System::run` and egui `Context::run` calls in tests.
2. Replace deprecated egui APIs in UI modules.
3. Remove unused imports/variables in production modules.
4. Leave structural Clippy warnings (`too_many_arguments`, `type_complexity`) for targeted refactors only.

Consider adding a narrower CI job first:

```text
cargo clippy -p macrocosmo-ai --lib -- -D warnings
cargo clippy -p macrocosmo --lib -- -D warnings
```

Then expand once production warnings are under control.

## Recommended Next Steps

1. Fix or rewrite `survey_command_outbox_holds_until_light_delay_elapses` so it observes the whole post-threshold window.
2. Add an outbox-aware dedup regression test for pending survey commands below the light-speed threshold.
3. Implement outbox-aware survey dedup in `npc_decision_tick` or a helper owned by `command_outbox.rs`.
4. Start warning cleanup with `unused_must_use` and deprecated egui APIs.

## Addendum: PlayerEmpire-Limited Logic Scan

Additional scan request: trace the existing codebase for logic that should apply to any `Empire`, but is still limited to `PlayerEmpire` or `Player`.

Searches covered `PlayerEmpire`, `With<PlayerEmpire>`, `With<Player>`, `EmpireViewerSystem`, `KnowledgeStore`, `SystemVisibilityMap`, `single()`/`single_mut()` player lookups, and major gameplay systems around survey, knowledge, faction relations, research, and command dispatch.

### A1. FTL Survey Completion Uses Player Position and Player GlobalParams

Severity: High  
Status: Likely bug

`process_surveys` is partly generalized: after an FTL survey finishes, auto-return routing resolves the ship owner's empire via `Empire -> EmpireRuler -> Ruler`. However, the earlier decision that chooses "send survey result by light" vs "carry it home by FTL" still uses:

- `Query<&StationedAt, With<Player>>`
- `Query<&GlobalParams, With<PlayerEmpire>>`
- `player_system_pos`
- the first player empire's `ftl_speed_multiplier`

Relevant files:

- `macrocosmo/src/ship/survey.rs`

Why this matters:

- In observer mode or NPC-only contexts, `player_system_pos` is absent, so `use_light_speed` falls back to `false`.
- NPC FTL survey ships may always carry results home even when light-speed propagation to their own ruler/home would be faster.
- NPC survey behavior can use the player's FTL speed multiplier instead of the owning empire's multiplier.

#### Fix Proposal

Resolve the reference system and FTL multiplier per ship owner:

1. For `Owner::Empire(e)`, resolve `e -> EmpireRuler -> Ruler.StationedAt`.
2. Read `GlobalParams` for that owner empire, not `PlayerEmpire`.
3. Use that owner reference position for `distance_ly_arr`.
4. Keep the current `Player` fallback only for legacy tests/saves without the empire-ruler chain.

Suggested regression tests:

- NPC FTL survey in observer/no-player setup chooses light propagation when owner home is close enough.
- NPC FTL survey uses its own `GlobalParams.ftl_speed_multiplier`, not the player's.

### A2. Sensor Buoy Detection Requires a Player Entity but Writes to All Empires

Severity: High  
Status: Likely bug / incomplete generalization

`sensor_buoy_detect_system` writes detected ship snapshots into every `Empire`'s `KnowledgeStore`, but it first requires:

- `Query<&StationedAt, With<Player>>`
- a player system position

If no `Player` exists, the system returns before updating any empire. This conflicts with the current behavior/comment that all empires receive buoy observations until ownership scoping is implemented.

Relevant files:

- `macrocosmo/src/deep_space/mod.rs`

There is also a per-empire timing issue: `observed_at` is computed once from buoy-to-player distance, then reused for all empire stores. If all empires receive the observation, each empire should compute its own delay from that empire's viewer/reference position, or the model should explicitly use a shared global observer.

#### Fix Proposal

Replace `Player` reference lookup with per-empire viewer lookup:

1. Query `(Entity, &mut KnowledgeStore, &EmpireViewerSystem)` or use `FactionVantageQueries`.
2. For each empire, compute `delay = light_delay(distance(buoy_pos, empire_viewer_pos))`.
3. Write snapshots only to that empire's store with its own `observed_at`.
4. Keep ownership scoping as a later filter, but do not require `Player`.

Suggested regression tests:

- In observer mode with two NPC empires and no `Player`, sensor buoy detection still updates empire knowledge.
- Different empire viewer positions produce different `observed_at` values.

### A3. Relay Knowledge Propagation Requires a Player Entity but Writes to All Empires

Severity: High  
Status: Likely bug / incomplete generalization

`relay_knowledge_propagate_system` also writes snapshots to all `Empire` knowledge stores, but it gates the entire system on:

- `Query<&StationedAt, With<Player>>`
- `player_pos`

It then checks whether the player is inside the partner relay range before collecting observations. In observer/no-player mode, no empire receives relay knowledge.

Relevant files:

- `macrocosmo/src/deep_space/mod.rs`

There is a second generalization issue in the same function: system snapshots build `hostile_map` using only the first empire in `empire_entities`, then write that same snapshot to all empires. Hostility visibility should be per receiving empire because `FactionRelations` are directional.

#### Fix Proposal

Generalize relay propagation around receiving empires:

1. Iterate receiving empires with `EmpireViewerSystem` or `FactionVantageQueries`.
2. Check partner relay coverage against each receiver's viewer position.
3. Build `hostile_map` per receiver using `FactionRelations::get_or_default(receiver, hostile_owner)`.
4. Write per-receiver `SystemKnowledge` snapshots instead of sharing one global snapshot.

Suggested regression tests:

- Relay knowledge works in observer mode without a `Player`.
- Two empires with different relations to the same hostile owner receive different `has_hostile` / hostile-strength snapshots.

### A4. Research Production Is Global, Player-Capital-Based, and Distributed to All Empires

Severity: High  
Status: Known TODO, but important Empire-general bug

`emit_research` computes light delay from each colony to the player's stationed system. `PendingResearch` carries no owner. `receive_research` then adds every arrived research packet to every `Empire`'s `ResearchPool`.

Relevant files:

- `macrocosmo/src/technology/research.rs`

The code already has a TODO noting that `PendingResearch` should carry an owner empire. The current behavior means one empire's colony research can benefit all empires, and NPC/player research timing is anchored to the player's location.

#### Fix Proposal

Make research packets owner-scoped:

1. Add `owner: Entity` to `PendingResearch`.
2. Resolve colony owner from `FactionOwner` on the colony.
3. Resolve that owner's capital/reference system, preferably `HomeSystem` or `EmpireRuler`.
4. In `receive_research`, add points only to the matching owner's `ResearchPool`.

Suggested regression tests:

- Two empires with one research colony each accrue independent research pools.
- Removing `Player` does not stop NPC research accrual.
- A remote colony's delay is computed to its owner capital/reference, not the player's.

### A5. CommandLog Is PlayerEmpire-Only

Severity: Low to Medium  
Status: Probably intentional UI limitation, but inconsistent with component placement

Several command logging paths are `PlayerEmpire`-only:

- `dispatch_ship_commands` appends dispatch log entries only to the single `PlayerEmpire`.
- `bridge_command_executed_to_log` finalizes only the single `PlayerEmpire` log.
- colony command dispatch uses `PlayerEmpire` for command log and player-click retry behavior.

Relevant files:

- `macrocosmo/src/ship/dispatcher.rs`
- `macrocosmo/src/ship/bridges.rs`
- `macrocosmo/src/communication/mod.rs`

This is probably acceptable if `CommandLog` is strictly a player UI feature. However, `empire_bundle` attaches `CommandLog` to every empire, so the data model suggests per-empire logs are intended eventually.

#### Fix Proposal

Clarify intent:

- If command logs are player UI only, avoid attaching `CommandLog` to every NPC empire or document that NPC logs are intentionally dormant.
- If logs should be per empire, stamp queued commands with issuer/owner and update the corresponding empire's `CommandLog` instead of `PlayerEmpire`.

### A6. Faction Discovery Is Player-Only

Severity: Low  
Status: Probably intentional for current UI, but not Empire-general

`detect_faction_discovery` populates a global `KnownFactions` resource only from the `PlayerEmpire` perspective. It discovers factions through player relations and player ship co-location.

Relevant files:

- `macrocosmo/src/faction/mod.rs`

This is consistent with the current global diplomacy UI, but it is not an Empire-general model. If NPC diplomacy/knowledge should independently discover factions, `KnownFactions` needs to become per empire or move into an empire component.

#### Fix Proposal

No urgent fix unless NPC faction discovery matters for simulation. If needed:

1. Replace global `KnownFactions` with per-empire known faction state.
2. Run relation/co-location discovery for every `Empire`.
3. Filter UI through the active viewing empire.

### A7. UI-Only PlayerEmpire Usage Mostly Looks Intentional

Severity: Informational

Many `PlayerEmpire` hits are UI view selection or player-input paths:

- outline/system panel/ship panel selection and actions
- player choice dialog
- top bar and situation center tabs
- visualization default viewpoint
- player respawn/autopause behavior

These do not need immediate generalization unless observer-mode UI or AI-facing UI is expected to manipulate arbitrary empires. Some visualization modules already have an observer-view fallback; those should be preferred when expanding UI behavior.

## Addendum Recommended Next Steps

1. Fix `process_surveys` to use owner empire reference position and owner `GlobalParams`.
2. Generalize `sensor_buoy_detect_system` and `relay_knowledge_propagate_system` away from `Player`.
3. Owner-scope `PendingResearch`.
4. Decide whether `CommandLog` is player-only UI state or real per-empire state.
5. Defer `KnownFactions` generalization unless NPC independent diplomacy discovery becomes a gameplay requirement.

## Addendum: Immediate Information / Command Propagation Scan

Requested follow-up: check whether information or commands that should travel with light-speed / relay delay are currently delivered immediately.

### B1. Casus Belli War State Bypasses Delayed Diplomacy

Severity: High  
Status: Confirmed delay bypass

The normal diplomacy path has explicit sender-immediate / receiver-delayed semantics:

- `declare_war_with_delay` immediately updates the sender's view, then spawns a `DiplomaticEvent` with `arrives_at`.
- `tick_diplomatic_events` flips the receiver's view only after `arrives_at`.

Relevant files:

- `macrocosmo/src/faction/mod.rs:588`
- `macrocosmo/src/faction/mod.rs:1012`
- `macrocosmo/src/faction/mod.rs:1045`

However, the casus-belli auto-war path directly mutates both relation directions in the same tick:

- `relations.declare_war(attacker, defender)`
- `relations.declare_war(defender, attacker)`
- immediate `GameEventKind::WarDeclared`

Relevant file:

- `macrocosmo/src/casus_belli.rs:283`

`end_war` has the same shape for peace: both directions are set immediately and a `WarEnded` event is emitted immediately.

Relevant file:

- `macrocosmo/src/casus_belli.rs:324`

Impact:

- Surprise-attack semantics implemented by delayed diplomacy do not apply to CB-triggered wars.
- The defender's relation view and ROE can become aware of war immediately even if the factions are far apart.
- Player-facing or AI-facing event streams can learn war/peace state instantly.

#### Fix Proposal

Route CB-driven diplomacy through the same delayed model as explicit diplomacy:

1. For auto-war, update only the attacker's relation immediately and spawn a delayed receiver-side diplomatic event.
2. Compute delay from the attacker/defender capital or current ruler/reference positions, using the same physical model as diplomacy UI.
3. For forced peace / war end, add a delayed built-in diplomatic event type or a CB-specific event that updates each receiver's view only on arrival.
4. Add regression tests covering `FactionRelations` asymmetry before arrival and symmetry after arrival.

### B2. Lua/GameState Command Requests Can Inject Ship Commands Immediately

Severity: Medium to High  
Status: Confirmed direct command path; risk depends on how scripts are allowed to call it

The remote colony command pipeline has an explicit delayed transport:

- `send_remote_command` computes distance, `light_delay_hexadies`, and `arrives_at`.
- `process_pending_commands` applies only after `clock.elapsed >= arrives_at`.

Relevant file:

- `macrocosmo/src/communication/mod.rs:501`

By contrast, `GameStateScope::request_command` allocates a `CommandId` and writes typed command messages directly into Bevy message queues:

- `MoveRequested`
- `MoveToCoordinatesRequested`
- `ScoutRequested`
- `LoadDeliverableRequested`
- `DeployDeliverableRequested`
- `TransferToStructureRequested`
- `LoadFromScrapyardRequested`
- `ColonizeRequested`
- `SurveyRequested`

Relevant file:

- `macrocosmo/src/scripting/gamestate_scope.rs:1661`

There is also a delayed scripted fact path (`enqueue_scripted_fact`) that explicitly computes arrival time and records into `PendingFactQueue`, so the scripting layer already has precedent for delayed information propagation.

Relevant file:

- `macrocosmo/src/scripting/gamestate_scope.rs:1832`

Impact:

- If Lua scripts can issue commands for ships or targets that are not co-located with the issuer, those commands skip the light-speed command delay.
- This can bypass `PendingShipCommand`, `PendingCommand`, and courier relay semantics.
- The current function name does not distinguish "local request already at the actuator" from "remote instruction being transmitted".

#### Fix Proposal

Split command APIs by transport semantics:

1. Keep a local-only API for commands that are already at the ship/system and can legitimately write typed request messages.
2. Add or require a remote API that stamps issuer empire, origin position, target position, `sent_at`, and `arrives_at`.
3. Make script-facing `request_command` validate locality; if the issuer is remote, route through the delayed command transport.
4. Add tests where a script attempts to command a ship at a distant system and assert no typed command message is emitted before arrival.

### B3. Player UI Ship Commands Mostly Have Delay, But Direct State Writes Remain for Zero-Delay Cases

Severity: Low to Medium  
Status: Mostly modeled; direct path should be kept constrained

The context-menu ship command path computes command delay from the player's current system to the ship's position, plus remaining ship travel time for non-docked ships.

Relevant file:

- `macrocosmo/src/ui/context_menu.rs:172`

It then either:

- writes a direct new `ShipState` for immediate commands, or
- pushes a `PendingShipCommand` with `arrives_at`.

Relevant file:

- `macrocosmo/src/ui/context_menu.rs:593`

This looks intentional for docked / same-system / zero-delay commands, but the direct `ShipState` assignment is a sharp edge: future call sites must not reuse it for remote commands.

#### Fix Proposal

Make the invariant explicit in code:

1. Gate the direct state write with an assertion or helper named for local/zero-delay commands.
2. Prefer funneling all nonzero-delay commands through `PendingShipCommand`.
3. Add a test for a distant docked ship proving the command is pending, not applied immediately.

### B4. Some Information Events Still Emit Immediate Global GameEvents

Severity: Medium  
Status: Mixed; many domain facts are delayed, but the global event stream can leak information if player-facing

Many ship/colony events correctly pair immediate simulation effects with delayed knowledge facts using `FactSysParam::record_for(...)`. Confirmed examples include:

- settlement / colonization facts
- movement arrival facts
- survey facts
- combat facts
- deliverable load/deploy facts
- building completion facts

Relevant files:

- `macrocosmo/src/ship/settlement.rs`
- `macrocosmo/src/ship/movement.rs`
- `macrocosmo/src/ship/survey.rs`
- `macrocosmo/src/ship/combat.rs`
- `macrocosmo/src/ship/handlers/deliverable_handler.rs`
- `macrocosmo/src/colony/building_queue.rs`

However, several paths emit `GameEvent` immediately without an accompanying delayed `KnowledgeFact` in the same function. Examples found in this scan:

- Core conquest emits `GameEventKind::CoreConquered` immediately.
- Exploration / anomaly side effects emit `SurveyDiscovery` / `AnomalyDiscovered` immediately with default event id.
- Casus belli war/peace emits `WarDeclared` / `WarEnded` immediately.

Relevant files:

- `macrocosmo/src/ship/conquered.rs:80`
- `macrocosmo/src/ship/exploration.rs:100`
- `macrocosmo/src/ship/exploration.rs:227`
- `macrocosmo/src/casus_belli.rs:304`

This may be harmless if `GameEvent` is strictly an internal/audit stream. It is a bug if the event log, notification UI, scripts, or AI consume `GameEvent` as player/empire knowledge.

#### Fix Proposal

Clarify the contract:

1. Treat `GameEvent` as omniscient simulation/audit data only, and never expose it directly as empire knowledge.
2. For player/AI notifications, require `KnowledgeFact` / `PendingFactQueue` or a diplomacy-specific delayed inbox.
3. Add delayed facts for `CoreConquered`, anomaly discoveries, and CB diplomacy events if they are intended to be observed by empires.
4. Add a lint-style test or code review checklist: every user-visible `GameEvent` for remote events must have a delayed observation path.

### B5. Relay / Sensor Knowledge Writes Bypass Per-Empire Arrival Queues

Severity: Medium  
Status: Confirmed model mismatch; also overlaps with PlayerEmpire-limited findings A2 and A3

The relay and sensor-buoy paths update knowledge stores directly once their systems run. They do not enqueue per-empire `PerceivedFact` items with independent `arrives_at` times.

Impact:

- Multiple empires can receive knowledge in the same tick even when their receivers / ruler positions should differ.
- The logic is currently anchored to player position or player relay coverage, then writes into broader empire stores.
- This is a delay-model issue even beyond the `PlayerEmpire` generalization issue.

Relevant files:

- `macrocosmo/src/deep_space/relay.rs`
- `macrocosmo/src/deep_space/sensor_buoy.rs`

#### Fix Proposal

Move relay/sensor observations into the same delayed observation pipeline:

1. Build `FactionVantage` for each receiving empire.
2. Compute arrival per receiver from buoy/relay/source to that empire's reference.
3. Record into per-empire pending knowledge queues when available.
4. Only update each empire's `KnowledgeStore` when that empire's fact arrives.

### B6. Research Uses Delays Internally, But Ownership Gaps Make Arrival Semantics Wrong

Severity: Medium to High  
Status: Confirmed ownership bug; not purely "immediate", but can cause effectively instant/wrong-empire propagation

Research collection and tech propagation do contain delay components:

- `PendingResearch` has `arrives_at`.
- `PendingKnowledgePropagation` has `arrives_at`.

Relevant file:

- `macrocosmo/src/technology/research.rs:61`
- `macrocosmo/src/technology/research.rs:84`

The issue is that `PendingResearch` has no owner, `receive_research` adds arrived points to every empire, and the capital/reference selection is player/global rather than owner-specific.

Relevant file:

- `macrocosmo/src/technology/research.rs:95`
- `macrocosmo/src/technology/research.rs:159`
- `macrocosmo/src/technology/research.rs:260`

This can cause information/economic value produced in one empire to become available to all empires on arrival, and can compute the wrong delay source/target.

#### Fix Proposal

Same as A4, but with an explicit delay invariant:

1. Add owner empire to `PendingResearch` and `PendingKnowledgePropagation`.
2. Compute delay to that owner's capital/reference.
3. Apply arrived research and tech knowledge only to that owner.
4. Test two empires at different distances from their colonies.

### B7. Local Ship Command Queues Are Not Themselves A Delay Bug

Severity: Informational

`CommandQueue` and ship dispatcher paths execute commands from a queue already attached to the ship. That stage appears to be local execution after a command has arrived or after a queued local command is valid.

Relevant files:

- `macrocosmo/src/ship/dispatcher.rs`
- `macrocosmo/src/ship/command_queue.rs`

The risk is upstream: any API that pushes directly into a ship's `CommandQueue` must already have enforced communication delay.

## Immediate Propagation Recommended Next Steps

1. Fix CB war/peace to use delayed diplomacy events instead of direct two-way relation mutation.
2. Split script command APIs into local-only direct dispatch and remote delayed dispatch.
3. Decide whether `GameEvent` is omniscient-only; if not, route user-visible events through delayed knowledge/diplomacy paths.
4. Generalize relay/sensor writes into per-empire pending observation queues.
5. Owner-scope research packets and tech knowledge propagation.

## Verification Notes

The scan was run on 2026-04-26. The working tree was clean before the report file was added.
