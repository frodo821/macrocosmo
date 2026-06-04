# No-op Existing UI Fragment Pass

This pass mirrors existing Rust UI surfaces as no-op Lua UI fragments in
`macrocosmo/scripts/ui/init.lua`. Rust does not render these fragments yet.

## Surfaces Covered

- Frame chrome: top bar, bottom bar, notification pills.
- Navigation: outline, ship command context menu.
- System/colony: system summary, planet list, planet detail, colony overview,
  colony population management.
- Ships: multi-selection, selected ship detail, refit.
- Major windows: research, ship designer, diplomacy, Lua console, choice dialog.
- ESC tabs: notifications, construction, ship ops, diplomacy, resource trends.
- AI debug tabs: inspector, plots, stream, governor, replay.

## Gaps Exposed

- Context is not only singular entities. Some hosts need plural or collection
  contexts such as `ships`, `systems`, `colonies`, `relations`, and
  `resource_history`.
- Context keys also need type information. A fragment that requires `ships`
  should not match a host that can only supply one `ship`, even if both are
  ultimately backed by entity ids.
- Host constraints need to be explicit. Current UI surfaces differ between
  low-height chrome, side panels, floating windows, blocking modals, overlays,
  developer-only windows, and tab bodies.
- Fragment labels are descriptive, not authoritative safety controls. Debug,
  file I/O, blocking modal behavior, local state, and mutation/navigation powers
  should be expressed as required host capabilities.
- Primitive vocabulary is too small for real migration. Existing UI needs at
  least tree/list rows, tab strips, filters, tables, tooltips, badges,
  split panes, editable forms, text input, numeric steppers, charts/sparklines,
  and action groups.
- Actions need richer metadata than `{ label, command }`: disabled reasons,
  confirmation, danger styling, ownership/observer gating, preview text,
  command cost, and stale-state handling.
- Some panels depend on local UI scratch state: selected branch/tab, form
  editing state, scroll/filter state, replay loaded-file state, and console
  input/history.
- Debug/file-IO surfaces should probably require host capabilities rather than
  general-purpose fragments.
- ESC should be a fragment host, not the only fragment API. Built-in tabs can
  query fragment labels such as `construction`, `ship_ops`, `diplomacy`, and
  `resources`.
- The shadow definitions currently use placeholder values and string-only
  commands. Real migration needs read-only view adapters and typed command args
  before these fragments can render real game state.

## Implementation Notes From This Pass

- Start with read-only fragments. Action-heavy surfaces multiply the command
  registry, stale-state, disabled-reason, and confirmation problems.
- Keep the first host narrow. ESC tab bodies are the best first target because
  they already render tree-shaped data and can tolerate diagnostic entries when
  Lua collection fails.
- Do not make every existing window a Lua fragment immediately. Research, ship
  designer, diplomacy, console, and AI replay need local state or specialized
  primitives that would bloat v1.
- Require fragments to declare `needs` once matching is real. Inferring
  primitive and capability usage only by running `render` makes host matching
  expensive and makes errors appear too late.
- Add source metadata to every definition. The no-op pass is already large
  enough that duplicate ids or bad context declarations will be painful without
  file/order diagnostics.

## Practical First Targets

The lowest-dependency migrations still look like:

1. ESC read-only ongoing fragments.
2. Bottom bar or notification pills, if host constraints for chrome are added.
3. Outline read-only rows, before selection/actions.

Avoid starting with ship designer, ship detail, diplomacy, AI replay, or Lua
console. They need editable local state, custom interactions, or debug/file
capabilities before the DSL has enough shape.
