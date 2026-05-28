# Lua UI DSL Design

## Purpose

This note sketches a Lua-defined UI extension model for Macrocosmo, updated
against the current ESC / Lua / command code.

The goal is not to let Lua draw egui directly. The goal is to let scripts
declare view data and actions while Rust keeps ownership of rendering,
validation, scheduling, and mutation.

There are two primary motivations:

1. Separate game logic from UI completely enough that simulation/domain modules
   do not depend on UI rendering, layout, input widgets, or panel-specific
   presentation state.
2. Reduce iteration cost. Today small UI changes often require rebuilding the
   whole `macrocosmo` crate. Moving panel composition, labels, grouping,
   visibility rules, and simple action wiring into Lua should let most UI
   iteration happen by reloading scripts instead of recompiling Rust.

The first target is the Empire Situation Center (ESC) tab API that was
anticipated in `docs/archive/plans/plan-326-esc.md` and now has a Rust-side
framework in `macrocosmo/src/ui/situation_center`.

The desired boundary is therefore not just "Lua can add a tab". It is "Rust
provides stable UI primitives and domain command/view APIs; Lua owns enough of
the concrete UI composition that routine product/UI changes do not touch Rust".

## Current Code Reality

The design must fit these existing boundaries:

- ESC already has `SituationTab`, `OngoingTab`, `SituationTabRegistry`, and the
  `AppSituationExt` registration helpers.
- Built-in ESC tabs are registered by `SituationCenterPlugin`:
  Notifications, Construction, Ship Ops, Diplomatic Standing, Resource Trends.
- `LuaOngoingTabAdapter` exists as a placeholder. It currently registers
  through the normal ongoing-tab path and returns an empty `Vec<Event>`.
- ESC event rendering is a fixed `Event` tree. `EventKind` is still a closed
  enum and `Event` has no actions field yet.
- `TabId` and `TabMeta.display_name` are `&'static str`. Lua-defined tabs need
  owned strings or an interning/registry-storage strategy before real dynamic
  registration is possible.
- Lua `ConditionCtx` currently exposes `empire`, `system`, `planet`, and
  `ship` scope handles. `ConditionScope` has the same four scopes plus `Any`;
  it does not yet have `Colony`, `Fleet`, or `BuildOrder`.
- Lua `gs:request_command(kind, args)` already exists for event callbacks in
  `scripting::gamestate_scope`, but it is a read-write scoped API and its
  command semantics are still encoded in a local `match`, not an application
  command registry.
- Lua gamestate views are sealed snapshots. A UI `collect` callback should use
  that read-only style, not the read-write `gamestate_scope` mutation surface.

## Core Boundary

The UI DSL has three layers:

```text
Lua scripts
  -> view descriptors + action descriptors
  -> Rust registries and renderers
  -> simulation command handlers
```

Lua may:

- register panels/tabs;
- collect read-only view data from a gamestate snapshot;
- attach `Condition` trees to visibility/enabled state;
- declare actions as command descriptors.

Lua must not:

- receive `&mut World`;
- call egui APIs directly;
- mutate ECS during UI rendering;
- define ad hoc per-widget mutation semantics.

Rust owns:

- descriptor parsing;
- `Condition` evaluation;
- command schema parsing or adapter parsing;
- command validation;
- command application in the simulation schedule;
- default rendering for descriptor trees;
- a small, stable widget/rendering vocabulary.

Lua should own:

- panel/tab composition;
- display labels and grouping;
- simple layout descriptors within the supported vocabulary;
- visibility/enabled conditions;
- action descriptors that route to registered commands;
- read-only aggregation and shaping of view data from gamestate snapshots.

The UI DSL should be implemented as an ESC extension first, but the design
should keep the broader compile-boundary goal visible. Avoid hardcoding ESC-only
concepts into the parser/runtime if the same primitive could serve later panels.
The large `ui/mod.rs` split can happen around this boundary once the primitive
set is credible.

## Target Architecture

The practical target is:

```text
Lua define_situation_tab tables
  -> LuaSituationTabDefinition registry
  -> LuaOngoingTabAdapter implements OngoingTab
  -> collect(read-only gamestate snapshot) returns Lua event tree
  -> Rust parses into EscEvent + optional UiActionDescriptor
  -> egui renderer displays default tree + action controls
  -> action click enqueues AppCommandRequest
  -> simulation-side command handler validates/applies
```

For phase 1, keep the tab body constrained to the existing ESC tree renderer so
the first cut is small. That is a stepping stone, not the final DSL shape. The
next phase should introduce a limited descriptor vocabulary for common UI
composition so changes like "split this panel into two groups", "add a list
column", or "move this action under this row" can happen in Lua without
rebuilding Rust.

The intended longer-term shape is:

```text
Rust domain/simulation
  -> stable read-only view APIs / snapshots
  -> stable command registry
  -> stable UI primitive renderer
Lua UI scripts
  -> concrete screens/panels/tabs/layout/action wiring
```

That keeps game logic and UI composition separate while still letting Rust own
the few pieces that must remain type-safe and scheduled.

## Sharpened V1 Contract

The design is intentionally broader than the first implementation. To keep the
runtime small and testable, v1 should define three separate artifacts:

- `UiFragmentDefinition`: loaded from Lua, owns static metadata, declared
  context, declared needs, and a Lua render callback reference.
- `MountedFragment`: owned by a host slot, owns instance id, concrete context,
  local state, refresh policy, dirty flags, and a cached descriptor tree.
- `UiNode`: data-only descriptor tree rendered by Rust without re-entering Lua.

This split is important because `mlua::Function` is not a plain data value and
must not leak into cached descriptors or generic UI render paths. Hosts can own
mounted Lua-backed fragments on the main UI thread, while renderers consume
validated descriptor data.

V1 should be strict:

- fragment definitions may contain Lua functions only in explicitly parsed
  callback fields such as `render` or `collect`;
- inflated descriptor trees must contain only strings, numbers, booleans,
  arrays, maps, opaque ids/refs, and action descriptors;
- parser errors should include fragment id and field path;
- render/collect errors should be contained to the fragment or tab and surfaced
  as diagnostics, not panics;
- unknown primitives, unknown context keys where a host schema exists, and
  unsupported capabilities should fail closed.

The current shadow Lua in `macrocosmo/scripts/ui/init.lua` is therefore an
authoring probe, not an accepted runtime contract. It intentionally uses
placeholder labels and command strings to expose vocabulary gaps.

## Implementation Risks And Limits

Known implementation constraints:

- **Lua VM affinity.** A Lua-backed fragment cannot naturally satisfy a generic
  `Send + Sync` fragment trait if it stores `LuaFunction`. Either keep
  Lua-backed registries as main-thread/non-send resources, or store only stable
  registry keys in sendable traits and invoke Lua through a host-owned runtime.
- **Error shape.** `inflate` / `collect` must return `Result`, not panic or
  assume well-formed Lua. Rust should render a compact diagnostic node when a
  fragment fails.
- **Descriptor validation.** The helper builders currently return mutable Lua
  tables with `_ui_node` tags. The parser must still validate every field,
  reject functions/userdata in descriptor output, clamp values such as progress,
  and reject recursive/cyclic tables.
- **Action validation.** String-only `command` fields are useful for no-op
  authoring but insufficient for real UI. V1 actions need `{ command, args }`
  payloads and should be parsed against a command registry before they render as
  enabled controls.
- **Context typing.** Labels such as `ships`, `relations`, or
  `resource_history` are not entity handles. Context values need list/view/state
  variants before nontrivial fragments can be mounted safely.
- **Host constraints.** `max_actions` and `allowed_primitives` cannot be checked
  from metadata unless fragments declare their needs up front or the runtime
  performs a cheap validation pass after inflation. Prefer declared `needs` for
  host matching and keep post-inflation validation as a guardrail.
- **Refresh policy.** Running Lua every egui frame would erase most of the
  benefit and create avoidable allocation pressure. V1 should require explicit
  dirtying rules and cache descriptor trees.
- **Hot reload.** Reloading scripts must reconcile definitions and mounted
  instances deliberately. Removed fragments should be dropped or replaced with
  diagnostics; changed state schemas need a simple migration/drop policy.
- **Sandbox boundary.** The current script sandbox disables `dofile`,
  `loadfile`, and C modules, but UI scripts still share the same Lua runtime as
  game definitions. Treat UI runtime APIs as capability based rather than
  assuming the global Lua environment is a security boundary.
- **Tooling.** Without source locations and inspectable registries, fragment
  discovery will be hard to debug. Store script/module path and definition order
  with every fragment.

## Fragment Discovery And Invocation Constraints

UI fragments should not primarily declare "I am a modal" or "I am a tab" as an
intrinsic identity. That couples reusable UI pieces to one caller shape and
pushes the system toward ad hoc mount points.

Prefer a query model:

```text
host UI context
  -> list fragments whose declared conditions match this context
  -> host chooses how matching fragments are mounted/rendered
```

In this model, a fragment declares what context it can operate on and when it is
applicable. The caller/host declares the invocation constraints for the slot it
is filling.

Example Lua-side shape:

```lua
define_ui_fragment {
    id = "colony.build_queue.summary",
    labels = { "colony", "summary", "build_queue" },

    context = {
        requires = { "colony" },
        optional = { "system", "empire" },
    },

    when = function(ctx)
        return ctx.colony:has_flag("has_build_queue")
    end,

    render = function(view)
        return section {
            title = "Build Queue",
            children = {
                list(view.colony.build_queue, function(order)
                    return row {
                        text(order.label),
                        progress(order.progress),
                    }
                end),
            },
        }
    end,
}
```

Then a Rust or Lua host asks for fragments by context and constraints:

```text
fragments.match {
  context = { colony = selected_colony, system = selected_system },
  labels_any = { "summary" },
  labels_all = { "colony" },
  disallow = { "destructive_action" },
}
```

The same fragment might be rendered inside a system panel, a situation-center
detail expansion, or a future modal if the host allows that primitive set. The
host owns the mounting decision; the fragment owns whether it is meaningful for
the supplied context.

Invocation constraints do not necessarily need to be Lua-authored. They may be
Rust-side contracts on the host slot:

```rust
pub struct FragmentQuery {
    pub required_context: ContextShape,
    pub allowed_primitives: PrimitiveSet,
    pub labels_any: Vec<String>,
    pub labels_all: Vec<String>,
    pub forbidden_labels: Vec<String>,
    pub max_actions: Option<usize>,
}
```

This gives the host a way to say "I need compact read-only fragments for a tab
body" or "I can host action-bearing detail fragments" without every fragment
hardcoding a modal/tab/inline role.

Open design constraints:

- Fragment matching must be deterministic. Sort by explicit order, then id.
- A fragment must fail closed if required context is absent.
- Fragment `when` conditions should use the same descriptor-based `Condition`
  machinery where possible; arbitrary Lua predicates are tempting but make
  caching, validation, and tooling harder.
- Host constraints should be allowed to reject fragments that use unsupported
  primitives, action kinds, or expensive data requirements.
- Fragment discovery should be inspectable in tests so adding a fragment cannot
  silently change an unrelated panel.

## Host Constraints

The no-op fragment pass showed that "where this fragment is rendered" is a host
contract, not a fragment identity. The DSL needs an explicit host capability
model.

Suggested Rust shape:

```rust
pub enum UiHostKind {
    ChromeTop,
    ChromeBottom,
    SidePanel,
    FloatingWindow,
    BlockingModal,
    Overlay,
    TabBody,
    DetailRegion,
    DebugWindow,
}

pub struct UiHostConstraints {
    pub kind: UiHostKind,
    pub allowed_primitives: PrimitiveSet,
    pub action_policy: ActionPolicy,
    pub state_policy: StatePolicy,
    pub capabilities: HostCapabilitySet,
    pub max_depth: Option<u8>,
    pub max_actions: Option<u8>,
}
```

Host constraints answer questions such as:

- Can this host show action buttons?
- Can this host block game flow?
- Can this host allocate local UI state?
- Can this host use file I/O or developer-only data?
- Can this host render charts or only compact rows?
- Can this host display large scrollable content?

The fragment declares what it needs. The host declares what it allows. Matching
fails closed if either side is incompatible.

```lua
define_ui_fragment {
    id = "core.ui.esc.resource_trends",
    labels = { "esc", "resources", "charts" },
    needs = {
        primitives = { "section", "row", "text", "sparkline" },
        capabilities = { "chart" },
        actions = false,
    },
    context = { requires = { "empire" }, optional = { "resource_history" } },
    render = function(view) ... end,
}
```

For v1, the host set can be small:

- `TabBody`: ESC tab body / future panel tabs.
- `FloatingWindow`: Research, Diplomacy, Ship Designer style windows.
- `SidePanel`: Outline / debug inspector selectors.
- `BlockingModal`: choice dialogs.
- `Chrome`: top/bottom bars and notification pills.

Avoid modelling every egui container at first. Host constraints are about
behavioral safety and fit, not pixel-perfect layout.

## Context Shape

The no-op pass showed that singular entity context is insufficient. Context
must support:

- single entity handles: `empire`, `system`, `planet`, `colony`, `ship`,
  `fleet`, `faction`;
- plural collections: `ships`, `systems`, `colonies`, `relations`;
- view models / histories: `resource_history`, `notification_queue`,
  `event_log`, `research_queue`;
- local host state handles: selected tab, filter, scroll, form state;
- capability-bearing debug state: AI debug snapshots, replay state, console
  state.

Use named context keys, but keep the value space typed on the Rust side:

```rust
pub enum UiContextValue {
    Entity(Entity),
    EntityList(Vec<Entity>),
    String(String),
    StringList(Vec<String>),
    Number(f64),
    Boolean(bool),
    ViewRef(UiViewRef),
    StateRef(UiStateRef),
}
```

Lua should see sealed/read-only view tables. Rust can keep `UiViewRef` and
`UiStateRef` opaque until it expands a fragment.

Fragment context declarations should be narrow:

```lua
context = {
    requires = {
        colony = "entity",
    },
    optional = {
        system = "entity",
        empire = "entity",
        build_queue = "view",
    },
}
```

Open issue: whether context type tags should be strings in Lua, or inferred
from a Rust-side schema keyed by context name.

## Primitive Vocabulary

The initial `section/row/text/progress/action` vocabulary is enough for no-op
authoring but not enough for migration. Add primitives only when a concrete
panel needs them.

For layout, start with concrete egui-shaped primitives instead of a full CSS
flexbox/grid abstraction. Macrocosmo's current UI is mostly immediate-mode
vertical stacks, horizontal rows, two-column key/value grids, tab strips, and
scroll areas. A broad flexbox model would add design surface before there is a
renderer or enough migrated panels to justify it.

Recommended approach:

- Use `vstack`, `hstack`, and `grid` as the first layout primitives.
- Keep sizing hints small: `grow`, `min_width`, `max_width`, `align`, `gap`.
- Add `scroll` once a migrated panel needs overflow behavior.
- Defer flexbox-style wrapping, grow/shrink weights, and named grid areas until
  a real panel cannot be expressed cleanly.

Recommended primitive tiers:

### Tier 0: Structural

- `section { title, children }`
- `vstack { children, gap?, align? }`
- `hstack { children, gap?, align? }`
- `grid { columns, children, striped? }`
- `row { children }`
- `column { children }`
- `group { label, children }`
- `separator {}`
- `spacer { size }`

`row`/`column` are semantic aliases only if they carry different defaults from
`hstack`/`vstack`; otherwise prefer `hstack`/`vstack` in Lua examples.

Do not expose arbitrary CSS-like layout in v1:

```lua
-- preferred v1
ui.vstack {
    gap = "sm",
    children = {
        ui.hstack { children = { ui.text("Minerals"), ui.text("+12") } },
        ui.grid {
            columns = 2,
            children = {
                ui.text("Energy"), ui.text("+8"),
                ui.text("Food"), ui.text("-2"),
            },
        },
    },
}
```

If this becomes too restrictive, add a `layout = { ... }` hint object to these
same primitives before introducing a separate general flex container.

### Tier 1: Read-only Data

- `text(value)`
- `label { text, tone }`
- `badge { text, severity }`
- `progress { value, label }`
- `kv { key, value }`
- `table { columns, rows }`
- `tree { rows }`
- `tooltip { child, text }`

### Tier 2: Selection And Filtering

- `tabs { selected, tabs }`
- `selectable { id, selected, label }`
- `filter_bar { value_ref, options }`
- `sort_header { key, label }`

### Tier 3: Actions

- `button { label, command, args, enabled }`
- `action { label, command, args, enabled, confirm, danger }`
- `action_group { actions }`
- `menu { label, children }`

`button` is the general clickable control primitive. `action` is a semantic
command descriptor with richer validation/confirmation metadata. A simple
button may compile into an action internally, but the DSL should keep the names
separate so layout authors can express "clickable UI control" without implying
danger/confirmation/preview semantics.

For v1, button labels should be plain text:

```lua
ui.button {
    label = "Open",
    command = "ui.open_panel",
    args = { panel = "research" },
}
```

Whether `label` may be an arbitrary UI node is a separate design question.
Allowing node labels enables icon+text or rich labels, but it also complicates
measurement, accessibility, compact host constraints, and command introspection.
Do not allow arbitrary label nodes until a concrete migrated panel needs them.

### Tier 4: Local Input / Forms

- `text_input { state, placeholder }`
- `number_stepper { state, min, max, step }`
- `checkbox { state, label }`
- `select { state, options }`

### Tier 5: Specialized

- `sparkline { series }`
- `chart { series, axes }`
- `code_log { lines }`
- `file_picker { state, mode }`

Tier 4 and 5 should be gated by host capabilities. They require local state,
debug/file access, or custom rendering and should not become universally
available.

## Actions

Action descriptors need more than `label + command`.

Suggested shape:

```rust
pub struct UiActionDescriptor {
    pub id: Option<String>,
    pub label: String,
    pub command: String,
    pub args: CommandArgs,
    pub visible: Option<Condition>,
    pub enabled: Option<Condition>,
    pub disabled_reason: Option<String>,
    pub confirm: Option<ConfirmDescriptor>,
    pub danger: bool,
    pub preview: Option<ActionPreviewDescriptor>,
}
```

Action validation has three layers:

1. Fragment-time: action descriptor is well-formed and command id exists.
2. Render-time: visible/enabled conditions are evaluated against current view
   context.
3. Click-time: command payload parses and validates against current world state.

The host decides whether actions are allowed at all. For example:

- `ChromeTop`: compact actions allowed, no destructive confirmation modals.
- `TabBody`: actions allowed if the tab host opts in.
- `BlockingModal`: exactly-one or explicit submit/cancel actions.
- `DebugWindow`: developer-only commands allowed behind debug capability.

Observer/read-only mode should be handled as a command validation concern plus a
host action policy. Do not scatter observer checks through Lua fragments.

## Local UI State

Several existing panels depend on local state:

- selected research branch/tab;
- colony detail active tab;
- ship designer edit form;
- filters and severity floors;
- scroll/collapse state;
- console input/history;
- AI replay loaded file and cursor.

The DSL needs state handles, but fragments should not directly own arbitrary
mutable Rust state.

Suggested model:

```lua
state = {
    active_tab = state.enum { default = "overview" },
    filter = state.string { default = "" },
    show_completed = state.bool { default = false },
}
```

Rust stores state by:

```text
host id + fragment id + state key
```

State policy controls whether a host can allocate or persist state:

- `Ephemeral`: reset when host closes.
- `Session`: survives tab/window switches during the run.
- `Persistent`: eligible for save/config persistence.

Start with `Ephemeral` and `Session`; defer `Persistent` until a concrete UI
needs it.

### Hydration Model

Conceptually, fragment state resembles React hydration: the fragment definition
is declarative, and runtime state must be re-associated with the same logical UI
node across frames. The implementation should borrow only the necessary part:
host-slot-level keyed reconciliation. It should not become a general React-like
component tree.

Prefer host-owned fragment instances over a global keyed state store. The host
is the debuggable owner of "what UI is mounted here", and each mounted fragment
instance carries its own context, state, and lifecycle.

```text
UiHost
  slots
    main
      FragmentInstance
        instance_id
        fragment_id
        context
        state
        lifecycle policy
    details
      FragmentInstance
        ...
```

This moves the heavy/debuggable part to the host:

- The host decides which fragment instances exist.
- The host owns each instance's context.
- The host owns each instance's state bucket.
- The host chooses when an instance is retained, replaced, or dropped.
- The fragment receives only its own instance-local context/state handles.

This is easier to debug than a global state key because the runtime can inspect
a host and list exactly which fragments are mounted in each slot.

Important performance constraint: Lua fragment `render` / `inflate` must not run
for every mounted fragment every frame. Even though egui is immediate-mode, the
Lua DSL layer should not be.

Each mounted fragment should cache its last descriptor tree:

```rust
pub struct MountedFragment {
    pub instance_id: FragmentInstanceId,
    pub fragment_id: String,
    pub context: UiFragmentContext,
    pub state: UiFragmentState,
    pub lifecycle: FragmentLifecycle,
    pub cached_descriptor: Option<UiNode>,
    pub dirty: FragmentDirtyFlags,
    pub refresh_policy: FragmentRefreshPolicy,
}
```

Runtime split:

```text
dirty mounted fragment
  -> run Lua render/inflate
  -> cache descriptor tree
clean mounted fragment
  -> reuse cached descriptor tree
Rust egui renderer
  -> draws cached descriptor tree every frame
```

This keeps high-frequency egui drawing in Rust while limiting Lua execution and
descriptor allocation to meaningful invalidation points.

```lua
define_ui_fragment {
    id = "core.ui.research",
    state = {
        active_branch = state.enum {
            default = "physics",
            values = { "physics", "industrial", "social", "military" },
        },
        filter = state.string { default = "" },
    },
    render = function(view, state)
        return ui.vstack {
            children = {
                ui.tabs {
                    state = state.active_branch,
                    tabs = { "physics", "industrial", "social", "military" },
                },
                ui.text_input {
                    state = state.filter,
                    placeholder = "Filter techs",
                },
            },
        }
    end,
}
```

Rust-side sketch:

```rust
pub struct FragmentInstanceId(String);

pub struct MountedFragment {
    pub instance_id: FragmentInstanceId,
    pub fragment_id: String,
    pub context: UiFragmentContext,
    pub state: UiFragmentState,
    pub lifecycle: FragmentLifecycle,
}

pub struct UiFragmentState {
    values: HashMap<String, UiStateValue>,
}

pub enum UiStateValue {
    Bool(bool),
    String(String),
    Number(f64),
    Enum(String),
}
```

The host can still use keys internally, but they are local to the host/slot:

```rust
pub struct UiHostState {
    pub host_id: String,
    pub slots: HashMap<String, Vec<MountedFragment>>,
}
```

Hydration then becomes host reconciliation, not global lookup:

```text
host queries matching fragments
  -> host compares desired fragments with mounted instances in that slot
  -> keep matching instances by explicit instance_id
  -> create missing instances with default state
  -> drop stale instances according to lifecycle policy
  -> mark created/recontextualized instances dirty
```

This gives most of the useful hydration behavior without requiring a component
tree diff or allowing fragments to address other UI state. Reconciliation is
limited to host slots and mounted fragment instances; child-node reconciliation
inside an inflated descriptor tree is out of scope for v1.

The difficult part is `fragment_instance_key`. It must be stable and explicit
for repeated fragments:

```lua
ui.fragment {
    id = "core.ui.ship.detail",
    key = ship.id,
    context = { ship = ship.id },
}
```

In the host-owned model, this key becomes the requested `instance_id` inside a
specific host slot. The same `ship.id` can safely appear in another host or slot
because it lives under a different mounted instance list. Implicit positional
keys should still be avoided except for static, non-repeated children because
they are fragile when Lua authors reorder rows.

The fragment should not be able to access arbitrary state paths. It receives:

```lua
render = function(view, state)
    -- state only contains this MountedFragment's declared local state.
end
```

That prevents a fragment from touching another panel's state. Cross-fragment
coordination must go through host context, commands, or explicit host-managed
shared state.

State policy still matters:

- `Ephemeral`: host drops the bucket when the window/modal closes.
- `Session`: host keeps the bucket while the game session lives.
- `Persistent`: store can be serialized by explicit opt-in only.

### Refresh And Dirtying

The hard part is dependency tracking. Do not attempt full automatic tracking at
first. Start with coarse explicit policies:

```rust
pub enum FragmentRefreshPolicy {
    Static,
    OnContextChanged,
    OnStateChanged,
    OnHostRevisionChanged,
    EveryTicks(u32),
    Manual,
}
```

Lua shape:

```lua
refresh = {
    on = { "context_changed", "state_changed", "host_revision_changed" },
    throttle = 5,
}
```

Dirtying rules for v1:

- New mounted instance: dirty.
- Fragment `context` identity changes: dirty.
- Fragment-local state changes: dirty.
- Host view revision changes and policy opts in: dirty.
- Explicit host refresh command: dirty.
- Otherwise: reuse cached descriptor.

Later, the runtime can add more precise dependency tracking:

- view handles with revision counters;
- command result invalidation;
- event-kind invalidation;
- dependency registration while expanding a sealed view table.

Avoid making Lua render functions run every frame as the fallback. If a panel
needs live per-frame values such as a clock or progress bar, prefer Rust-side
dynamic leaf bindings or a narrow `EveryTicks(n)` policy over global per-frame
reinflate.

### Descriptor Cache Boundaries

The cached descriptor tree must be data-only:

- no Lua functions;
- no userdata handles except opaque state/view/action handles validated by Rust;
- no direct ECS references except stable ids or host-owned view refs.

This lets Rust render cached descriptors without re-entering Lua. It also makes
debugging possible: a dev tool can inspect a mounted fragment's context, state,
dirty flags, refresh policy, and cached descriptor.

Open questions:

- Should state declaration live in `define_ui_fragment`, or can hosts inject
  state handles into stateless fragments?
- Should state values be limited to scalar values initially?
- How much reconciliation/garbage collection should each host perform
  automatically when fragment matching changes?
- Do modal/choice flows need transaction state, where state is committed only
  when the player clicks submit?
- Should any shared state exist, or should all cross-fragment coordination go
  through commands and host context?
- How should dynamic leaves be represented for values that must update without
  re-running Lua?

## Host Capabilities

Some UI fragments should require capabilities rather than being available
everywhere:

- `chart`: sparkline/chart drawing.
- `text_input`: user text input.
- `file_io`: replay loading or file picker.
- `developer`: debug-only surfaces.
- `blocking`: can pause/gate flow.
- `selection_mutation`: can change current selected entity.
- `navigation`: can pan/select/jump to entities.

Capabilities should be checked by the host before inflation. This prevents a
general panel from accidentally hosting a debug replay file picker or a blocking
choice fragment.

## Rebuild Boundary

A UI DSL only helps iteration if the Rust surface stays stable. Treat every new
Rust UI primitive as an API addition, not as the normal way to change one panel.

Good Lua-side changes:

- rename labels, headers, and action text;
- regroup existing fields;
- add/remove rows using already-exposed view data;
- change sort/group/filter rules that operate on snapshot data;
- add visibility/enabled conditions;
- wire a button to an existing registered command;
- compose supported primitives such as `section`, `row`, `table`, `tree`,
  `progress`, `badge`, and `action`.

Rust changes should be needed only when:

- the UI needs new domain data not exposed by the view snapshot;
- the UI needs a new validated domain command;
- the existing primitive vocabulary cannot express a genuinely new interaction;
- rendering behavior itself changes.

This distinction is important. A Lua tab API that still requires a Rust field,
enum variant, or custom renderer for every UI tweak will not solve the full
rebuild problem.

## Reuse Existing Condition Infrastructure

Existing Lua conditions are already descriptor-based:

```lua
prerequisites = function(ctx)
    return all(
        ctx.empire:has_tech("industrial_automated_mining"),
        ctx.system:has_building("shipyard")
    )
end
```

`ConditionCtx` does not hold game state. It only builds Lua tables. Rust parses
those tables into `crate::condition::Condition` and evaluates them later.

UI should reuse that model.

Examples:

```lua
visible = function(ctx)
    return ctx.empire:has_flag("can_manage_colonies")
end

enabled = all(
    has_tech("industrial_automated_mining"),
    has_building("shipyard")
)
```

Rust-side descriptor shape:

```rust
pub struct UiConditioned<T> {
    pub visible: Option<Condition>,
    pub enabled: Option<Condition>,
    pub inner: T,
}
```

Parsing can initially reuse the same helper shape as `parse_prerequisites_field`
with the field name made explicit:

```rust
parse_condition_field(table, "visible")
parse_condition_field(table, "enabled")
```

Evaluation should adapt UI selection/view state into the existing
`EvalContext`. Avoid introducing a second UI-only evaluator.

```rust
pub struct UiConditionInputs<'a> {
    pub techs: &'a HashSet<String>,
    pub modifiers: &'a HashSet<String>,
    pub empire: Option<ScopeData<'a>>,
    pub system: Option<ScopeData<'a>>,
    pub planet: Option<ScopeData<'a>>,
    pub ship: Option<ScopeData<'a>>,
}
```

If the existing `ConditionScope` lacks a necessary scope, add it deliberately.
Do not add UI-only string checks inside the UI interpreter.

Likely additions:

- `ConditionScope::Colony`;
- `ConditionScope::Fleet`;
- possibly `ConditionScope::BuildOrder` only if order-level actions need
  condition evaluation.

Do not add these scopes speculatively. ESC phase 1 can start with the current
`empire/system/planet/ship/any` scopes and add `colony` once a concrete Lua tab
needs colony-local flags or buildings.

`ConditionCtx` must be extended together with `condition_parser::parse_scope`
and `EvalContext`; adding only Lua helpers would create conditions that parse
but cannot evaluate correctly.

## Mutation Via Command Descriptors

UI mutation should follow the same pattern as `EffectScope`: Lua callbacks
collect descriptors; they do not directly mutate live state.

Prefer declarative actions first:

```lua
actions = {
    {
        label = "Cancel",
        command = "colony.cancel_build_order",
        args = {
            colony = colony.id,
            order_id = order.id,
        },
        confirm = "Cancel this order?"
    }
}
```

Callback actions are acceptable only if the callback receives a command scope
that can emit descriptors and nothing else:

```lua
actions = {
    {
        label = "Cancel",
        on_click = function(scope)
            scope:command("colony.cancel_build_order", {
                colony = colony.id,
                order_id = order.id,
            })
        end,
    }
}
```

The callback form should be compiled into the same descriptor shape:

```rust
pub struct UiActionDescriptor {
    pub id: Option<String>,
    pub label: String,
    pub visible: Option<Condition>,
    pub enabled: Option<Condition>,
    pub command: String,
    pub args: CommandArgs,
    pub confirm: Option<String>,
}
```

Click flow:

```text
egui button clicked
  -> UiActionDescriptor
  -> AppCommandRegistry lookup
  -> parse typed payload
  -> validate against current World
  -> enqueue/apply during simulation schedule
```

## App Command Registry

To keep the UI DSL interpreter small, the interpreter must not own command
semantics.

Avoid:

```rust
match command_id {
    "colony.cancel_build_order" => { ... }
    "ship.move" => { ... }
    "research.set_focus" => { ... }
}
```

Use a registry:

```rust
pub trait AppCommand: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn schema(&self) -> CommandSchema;
    fn parse(&self, args: CommandArgs) -> Result<Box<dyn AppCommandPayload>, CommandError>;
    fn validate(&self, world: &World, payload: &dyn AppCommandPayload) -> Result<(), CommandError>;
    fn apply(&self, world: &mut World, payload: Box<dyn AppCommandPayload>) -> CommandResult;
}
```

Domain modules own command implementations:

```text
commands/colony.rs
commands/ship.rs
commands/research.rs
commands/diplomacy.rs
```

The UI runtime only sees:

```text
command_id + args -> registry -> queued command result
```

This prevents the Lua UI DSL from growing into a second domain interpreter.
The command catalog may grow, but that is useful growth: it becomes the shared
operation surface for UI, remote API, keybindings, tests, and possibly AI debug
tools.

Do not introduce generic mutation commands such as:

- `set_component_field`;
- `insert_component`;
- `run_system`;
- `eval_lua_on_world`.

Those commands would bypass domain validation and make save/load, replay, and
multiplayer/debug tooling harder.

### Relationship To Existing `gs:request_command`

The current Lua command bridge is useful prior art, but it is not the final UI
command registry:

- `scripting::gamestate_scope::apply::parse_request` is already Lua-free after
  it extracts primitive args.
- `apply::request_command` already keeps mutation on the Rust side and returns a
  `CommandId`.
- The supported command set is currently hardcoded in a `match`.
- The bridge is exposed only in read-write gamestate scopes; UI `collect` must
  not receive that scope.

Recommended migration:

1. Introduce an `AppCommandRegistry` whose first adapter wraps the existing
   `ParsedRequest` command kinds (`move`, `survey`, etc.).
2. Keep `gs:request_command` working by routing it through the registry once the
   registry exists.
3. Add UI-specific/domain-specific commands such as
   `colony.cancel_build_order` only after the registry is in place.

This avoids building a second command catalog for UI while preserving current
Lua event behavior.

## First Implementation Target: ESC Tabs

Start with ESC because it already uses value-shaped event trees and has a
`LuaOngoingTabAdapter` placeholder.

Initial Lua API:

```lua
define_situation_tab {
    id = "construction",
    display_name = "Construction",
    order = 500,

    collect = function(gs)
        return {
            {
                id = "colony_1_queue",
                label = "Shipyard Queue",
                children = {
                    {
                        id = "order_42",
                        label = "Frigate",
                        progress = 0.25,
                        actions = {
                            {
                                label = "Cancel",
                                command = "colony.cancel_build_order",
                                args = {
                                    colony = "colony_1",
                                    order_id = 42,
                                },
                            },
                        },
                    },
                },
            },
        }
    end,
}
```

The returned event table should map onto the existing `Event` shape first:

```rust
pub struct Event {
    pub id: EventId,
    pub source: EventSource,
    pub started_at: GameTime,
    pub kind: EventKind,
    pub label: String,
    pub progress: Option<f32>,
    pub eta: Option<GameTime>,
    pub children: Vec<Event>,
}
```

Because `EventKind` is currently closed, phase 1 should either:

- parse omitted/unknown Lua `kind` as `EventKind::Other`; or
- add `EventKind::Custom(String)` in the same PR that parses Lua tabs.

Do not require Lua authors to know Rust enum coverage for the first slice.

Phase 1 should support:

- `define_situation_tab`;
- `collect(gamestate) -> Event tree`;
- parsing Lua event trees into current ESC `Event`;
- optional `visible` / `enabled` conditions on events and actions;
- action descriptors only if `Event` gains an actions field in the same slice;
- registry-time validation of command IDs once `AppCommandRegistry` exists;
- click-time validation of command payloads.

Phase 1 should not support:

- arbitrary egui layout;
- custom widget rendering from Lua;
- arbitrary mutation callbacks;
- hot-unregistration;
- persistence of UI script runtime state.

Phase 1 deliberately does not solve all UI rebuild pressure. It proves the
script-registration, snapshot, parser, and command-boundary path with a narrow
surface. The follow-up milestone should generalize from `Event` trees to a small
UI descriptor tree shared by ESC and at least one existing non-ESC panel.

ESC should become a fragment host rather than a one-off Lua-tab mechanism:

- built-in tabs can query fragments with labels such as `construction`,
  `ship_ops`, `diplomacy`, or `resource_trends`;
- a Lua-defined tab can still exist, but it should be one host/query definition
  over fragments, not the only extension shape;
- read-only fragment matching should land before action-bearing fragments.

### Required Rust Shape Changes

Before real Lua registration, update the ESC trait metadata shape:

```rust
pub type TabId = String;

pub struct TabMeta {
    pub id: TabId,
    pub display_name: String,
    pub order: i32,
}
```

or keep static public aliases and store Lua strings in a registry-owned arena.
The owned-string option is simpler and likely good enough; built-in tabs can
return `"construction".into()` with no meaningful cost.

Then add an action-capable event wrapper without disrupting the existing default
renderer:

```rust
pub struct EventAction {
    pub descriptor: UiActionDescriptor,
}

pub struct Event {
    // existing fields...
    pub actions: Vec<EventAction>,
}
```

If that is too broad for the first PR, land read-only Lua tabs first and defer
actions to the command-boundary PR.

## Relationship To `EffectScope`

`EffectScope` is a useful pattern but should not be reused directly as the UI
mutation API.

Reason:

- tech/effect descriptors describe game-rule effects;
- UI action descriptors describe user-requested commands;
- mixing them would blur preview, validation, permission, and scheduling rules.

Instead, create `UiCommandScope` with the same descriptor-collection contract:

```rust
pub struct UiCommandScope {
    commands: Vec<UiActionCommandDescriptor>,
}
```

Lua callback:

```lua
on_click = function(scope)
    scope:command("research.set_focus", { tech = "ftl_theory" })
end
```

The callback result is still a command descriptor, and the command registry is
still responsible for parsing and applying it.

There is one likely cross-over: game-rule effects may need to request a UI
surface. For example, an event choice or lifecycle hook may want to present a
fragment for a specific colony, ship, or diplomatic relation.

That should still be descriptor-based. `EffectScope` should not inflate UI
directly and should not receive egui handles. It may emit a UI presentation
request:

```lua
on_trigger = function(scope, evt)
    scope:show_ui_fragment {
        context = {
            colony = evt.payload.colony,
            system = evt.payload.system,
        },
        labels_all = { "colony", "new_building_choice" },
        host = "modal",
        mode = "blocking_choice",
    }
end
```

Rust-side shape:

```rust
pub enum DescriptiveEffect {
    // existing variants...
    PresentUiFragment(UiFragmentPresentationRequest),
}

pub struct UiFragmentPresentationRequest {
    pub context: UiContextDescriptor,
    pub query: FragmentQueryDescriptor,
    pub preferred_host: Option<UiHostKind>,
    pub mode: UiPresentationMode,
}
```

The important boundary is:

```text
EffectScope emits presentation request
  -> simulation/event pipeline records or queues request
  -> UI runtime matches fragments against context + host constraints
  -> UI runtime inflates descriptor tree during UI rendering
```

This keeps game logic from depending on concrete UI layout while still allowing
game rules to ask for player-facing UI at the right time.

Constraints:

- The effect names context and fragment query constraints, not a concrete egui
  widget tree.
- The UI runtime may reject the request if no matching fragment exists or the
  requested host cannot support the fragment primitives.
- Presentation requests should be serializable or reducible to stable ids where
  they can cross save/load or delayed event boundaries.
- Blocking UI requests need explicit scheduling semantics. A "modal choice"
  should pause or gate the relevant game flow through an existing choice/dialog
  pipeline, not by blocking a system while UI is open.
- Non-blocking requests should degrade to notifications or ESC entries if the
  preferred host is unavailable.

## Validation Strategy

Validation should happen at two times.

At script load / tab registration:

- required fields exist;
- tab ids are unique;
- command ids exist, if actions are supported in that slice;
- `visible` / `enabled` fields parse as `Condition`;
- `collect` is a Lua function;
- static tab metadata is valid.

At click / command execution:

- args parse into the command payload;
- referenced entities or domain ids still exist;
- current viewer has permission;
- current simulation state still allows the action;
- the command is applied in the simulation schedule, not during egui draw.

Load-time command-id validation catches script mistakes early. Click-time
payload validation handles stale UI entries and light-delay/state changes.

At `collect` time:

- pass only a sealed/read-only gamestate snapshot;
- convert Lua tables to Rust values immediately;
- reject functions/userdata inside returned event/action trees;
- contain Lua errors to that tab for the frame and render a small diagnostic
  entry instead of panicking the whole UI.

## Testing Plan

Unit tests:

- parse `define_situation_tab` into a registry entry;
- parse `visible` / `enabled` as existing `Condition`;
- reject unknown condition atoms;
- reject unknown command ids when actions are enabled;
- convert Lua event table into the Rust event tree;
- collect commands from `UiCommandScope` callback form.

Integration tests:

- registered Lua tab appears in the ESC registry;
- tab `collect` reads a gamestate snapshot without mutating `World`;
- disabled action renders disabled;
- clicking an action queues an `AppCommandRequest`;
- stale args fail validation without panic;
- command application happens outside egui rendering.

Regression guard:

- no UI DSL module should import domain mutation APIs directly except through the
  command registry boundary;
- no UI DSL collect path should expose `request_command`;
- no simulation-side module should import egui or UI DSL renderer types.

## Open Questions

- Should `AppCommandRequest` be its own queue, or should registry handlers emit
  existing domain messages directly?
- Should the existing `ParsedRequest` bridge become the first
  `AppCommandRegistry` backend, or stay as a legacy adapter until UI actions
  need it?
- What is the minimum widget vocabulary that removes meaningful rebuild churn
  without recreating egui in Lua?
- What fragment labels/context keys are stable enough to become API, and which
  should stay panel-local until proven?
- Should `when` be restricted to descriptor `Condition` values, or can some
  fragment discovery predicates be Lua functions evaluated against read-only
  snapshots?
- How should tests pin host fragment selection so adding a fragment does not
  unexpectedly alter an existing panel?
- Should UI scripts be reloadable in a running dev build, or is restart-without-
  rebuild enough for the first iteration target?
- Which current panel is the best second target after ESC to prove the rebuild
  boundary: system panel, colony detail, diplomacy, or ship panel?
- Which `ConditionScope` additions are truly needed after read-only ESC tabs?
- Should Lua-defined tab ids be namespaced by script/module path to avoid
  collisions?
- Should action descriptors support `dangerous = true` as a standardized
  confirmation style, or only free-form `confirm` text?
- Should command schemas be exposed to Lua/BRP for introspection?
- Should `EventKind::Custom(String)` land with read-only Lua tabs, or should Lua
  v1 use `Other` only?

## Recommended Slice Order

1. Update ESC metadata to support dynamic Lua-owned tab ids/display names.
2. Implement `define_situation_tab` parsing and registration with read-only
   `collect(gamestate) -> Event tree`; no actions yet.
3. Add Lua event-tree parser tests and an integration test proving the tab
   appears in `SituationTabRegistry`.
4. Implement `AppCommandRegistry` by adapting the existing
   `gs:request_command` command kinds.
5. Add `UiActionDescriptor` and `UiCommandScope` without exposing them to
   layout/general UI.
6. Add action descriptors to ESC `Event` and render action buttons in the
   default event-tree renderer.
7. Wire action clicks to `AppCommandRegistry` through a queued
   `AppCommandRequest` applied outside egui rendering.
8. Introduce read-only fragment definitions and deterministic fragment matching
   for one ESC host.
9. Define the first general UI descriptor primitives from concrete panel needs,
   not from a complete egui clone.
10. Migrate one non-ESC panel to the descriptor runtime to prove UI changes can
   happen by editing Lua only.
11. Expand from ESC to other UI surfaces only after the command boundary is
   stable.
