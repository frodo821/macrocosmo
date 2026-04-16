# Empire Situation Center — design note (epic #326)

Tracking doc for the ESC epic (#326). ESC-1 lands the framework (#344),
ESC-2 wires the Notifications tab + bridge (#345), ESC-3 ships the four
ongoing tabs (#346).

This doc is **not** a complete spec — the issues carry the authoritative
scope. It exists to record the design decisions whose rationale would
otherwise only live in PR review, especially the Lua boundary contract
the traits in `src/ui/situation_center/tab.rs` encode.

## Module layout (ESC-1)

```
macrocosmo/src/ui/situation_center/
├── mod.rs                 pub surface + SituationCenterPlugin
├── types.rs               Event / Notification / *Source / Severity / EventKind / GameTime / BuildOrderId
├── tab.rs                 SituationTab / OngoingTab traits + OngoingTabAdapter + default Event tree renderer
├── state.rs               SituationCenterState resource + TabState
├── registry.rs            SituationTabRegistry resource + AppSituationExt trait
├── notifications_tab.rs   NotificationsTab (SituationTab impl) + EscNotificationQueue stub
├── lua_adapter.rs         LuaOngoingTabAdapter + LuaTabRegistration placeholder
└── panel.rs               toggle_situation_center (F3) + draw_situation_center_system (exclusive)
```

## Trait contract

The public surface ESC-2 / ESC-3 / the Lua API all rely on is:

```rust
pub trait SituationTab: Send + Sync + 'static {
    fn meta(&self) -> TabMeta;
    fn badge(&self, world: &World) -> Option<TabBadge>;
    fn render(&self, ui: &mut egui::Ui, world: &World, state: &mut TabState);
    fn as_any(&self) -> &dyn Any;
}

pub trait OngoingTab: Send + Sync + 'static {
    fn meta(&self) -> TabMeta;
    fn collect(&self, world: &World) -> Vec<Event>;
    fn badge(&self, world: &World) -> Option<TabBadge> { /* default roll-up */ }
}
```

An `OngoingTab` is lifted to a `SituationTab` at registration time via
`OngoingTabAdapter<T>`; the framework supplies the default Event-tree
renderer from `render_event_tree`.

Registration is fluent:

```rust
app.register_situation_tab(NotificationsTab);                 // custom render
app.register_ongoing_situation_tab(ConstructionOverviewTab);  // Event-tree render
```

`TabMeta.order` is the sort key (ascending). Ties break by insertion
order.

## `Event` / `Notification` shape

Both are plain structs (no enum variants) with a uniform tree:

```rust
struct Event {
    id, source, started_at, kind, label,
    progress: Option<f32>, eta: Option<GameTime>,
    children: Vec<Event>,
}
struct Notification {
    id, source, timestamp, severity, message, acked,
    children: Vec<Notification>,
}
```

`children` empty ⇒ leaf, non-empty ⇒ collapsible group. This keeps the
default renderer to a single recursive walk and lets Lua-defined tabs
materialise arbitrarily deep trees without dragging in a trait-object
hierarchy.

`EventSource` and `NotificationSource` are **separate enums** with the
same variants (None / Empire / System / Colony / Ship / Fleet / Faction
/ BuildOrder). They're kept apart so individual tabs / bridges can
tighten invariants on one side (e.g. "Construction Event sources are
Colony or System only") without changing the other. Merging later is
cheap if it turns out the distinction carries no weight.

`EventKind` is a closed v1 enum (`Construction / Combat / Diplomatic /
Survey / Travel / Resource / Other`). Lua-defined open kinds land with
the Lua API issue — see the boundary section below.

## Relationship to existing `NotificationQueue`

The pre-existing `crate::notifications::NotificationQueue` drives the
top-centre banner stack (#151): TTL-based pop-overs, Low/Medium/High
priority with pause-on-High semantics, and a `GameEvent`-whitelist
bridge. It's fundamentally a "transient live feed" queue.

ESC introduces a **separate** `EscNotificationQueue` (stub in ESC-1,
wired in ESC-2) for post-hoc ack-able notifications. The two queues
coexist intentionally:

- Banner queue: live feed at the top of the screen, autodismisses.
- ESC queue: history panel inside the ESC, requires explicit ack.

ESC-2 introduces the Lua wildcard subscriber that fills the ESC queue
from `KnowledgeFact` observations. The banner path is unchanged.

Merging the two queues is out of scope for the ESC epic; if it ever
makes sense, the consolidation lands behind a separate issue with
migration steps for the tests in `notifications.rs`.

## Lua API future-proof boundary

**Design goal**: when the `define_situation_tab` Lua API lands
(separate issue, after ESC-1/2/3), **no change to `SituationTab` /
`OngoingTab` / `SituationTabRegistry` / `AppSituationExt` should be
required.**

Three facts make this safe:

1. **Trait contract is stable.** Both traits take `&World` by shared
   reference for `badge` and `render` / `collect`. A Lua callback runs
   read-only over a snapshotted `gamestate` view (#263 / #289 pattern);
   a Rust callback runs read-only over `&World`. The callback shape is
   compatible with both.

2. **Registry stores `dyn SituationTab`.** Lua-defined tabs register a
   `LuaOngoingTabAdapter` (already shipped in ESC-1 as a placeholder).
   The adapter implements `OngoingTab` and the registration path is
   the existing `app.register_ongoing_situation_tab(adapter)` — same
   API as Rust-defined ongoing tabs.

3. **Event-tree conversion is value-shaped.** A Lua `collect` function
   returns a table (array of event tables). The adapter converts it to
   `Vec<Event>` field-by-field. The `Event` struct is plain data — no
   borrowed fields, no trait-object members. Round-trip is simple:

   ```lua
   define_situation_tab {
       id = "my_tab",
       display_name = "My Tab",
       order = 500,
       collect = function(gamestate)
           return {
               { id = 1, label = "Group A", children = {
                   { id = 2, label = "Entry A1", progress = 0.25 },
               }},
               { id = 3, label = "Leaf B" },
           }
       end,
   }
   ```

   The Lua API issue adds a `mlua::RegistryKey` field to
   `LuaTabRegistration` and fills in `collect` with the conversion +
   timeout + error containment used by `#349 dispatch_knowledge`.
   *No framework refactor*.

### EventKind open-variant extension

The closed v1 `EventKind` enum will likely grow a `Custom(String)`
variant when the Lua API lands. This is a **purely additive** change
to `EventKind`; existing pattern matches with a `_` wildcard continue
to compile, and the default renderer has no dependency on the variant
list.

### Registration timing

Lua-defined tabs are registered after `load_all_scripts` (standard
Lua pipeline hook). The registry is append-only in ESC-1, so a
late-arriving Lua tab simply appears on the right of the tab strip
(unless `order` is set). Un-registration / hot-reload is a later
concern.

## Keybinding

ESC-1 hard-codes F3 in `panel.rs::TOGGLE_KEY`. #347 introduces the
in-game keybinding manager; at that point the constant is replaced by
a lookup against the keybinding registry. The `toggle_situation_center`
system signature stays the same — only the resource it reads changes.

## Out of scope for ESC-1

- `EscNotificationQueue` push / ack / dedupe wiring — ESC-2 (#345).
- `GameEvent → EscNotification` bridge — ESC-2 (#345) via Lua
  `*@observed` wildcard subscriber (#349 pipeline).
- Four ongoing tabs — ESC-3 (#346).
- `define_situation_tab` Lua API — separate issue.
- `KeybindingRegistry` integration — #347.
- `EscNotificationQueue` / banner `NotificationQueue` consolidation —
  never in the epic; open a separate issue if needed.

## Test coverage

Unit tests in the new module cover:

- `types.rs` — tree traversal, cascade ack, severity roll-up, severity
  ordering.
- `tab.rs` — `OngoingTabAdapter` delegates to the inner tab, default
  Event-tree renderer handles empty slice + mixed leaf/group.
- `registry.rs` — register + retrieve, sort-by-order, stable within
  order key, ongoing-tab wrapping.
- `notifications_tab.rs` — empty queue ⇒ no badge, unacked count +
  highest severity roll-up, missing resource tolerated.
- `panel.rs` — F3 toggle flips `open`, badge colour matches severity.
- `mod.rs` — plugin installs all expected resources + the bundled
  Notifications tab, further `register_ongoing_situation_tab` calls
  work end-to-end.
- `lua_adapter.rs` — placeholder `LuaOngoingTabAdapter` registers via
  the standard API (regression guard on the future-proof contract).

Integration guards (existing):

- `all_systems_no_query_conflict` in `tests/smoke.rs` — runs the full
  `UiPlugin` + `SituationCenterPlugin` under `full_test_app` and fails
  fast on Query conflicts (B0001) introduced by the new systems.
