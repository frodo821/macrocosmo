# Evidence: 2026-05-27 Refactor Boundary Review

## Repository Shape

Workspace crates:

```text
members = ["macrocosmo", "macrocosmo-ai"]
```

Current dependency direction:

```text
macrocosmo -> macrocosmo-ai
macrocosmo-ai -X-> macrocosmo / bevy
```

`macrocosmo-ai` is already engine-agnostic. The immediate coupling problem is inside `macrocosmo`, where simulation, UI, rendering, input, scripting, persistence, and AI adapters share one crate and one module namespace.

## Commands Run

```text
sed -n '1,260p' macrocosmo/src/lib.rs
sed -n '1,220p' macrocosmo/src/main.rs
sed -n '1,160p' macrocosmo/src/time_system/mod.rs
rg "struct .*Plugin|impl Plugin for|add_plugins|add_systems" macrocosmo/src -n | head -220
find macrocosmo/src macrocosmo-ai/src -name '*.rs' -type f -print0 | xargs -0 wc -l | sort -nr | head -25
rg "use crate::(ui|visualization|input|remote|observer)|bevy_egui|egui::|KeyCode|ButtonInput|Window" ... 
rg "use crate::(ship|colony|galaxy|knowledge|scripting|technology|ai|time_system|notifications|choice|faction)|crate::(...)" macrocosmo/src/{ui,visualization,input,observer,remote.rs} -n | head -200
```

Note: one `rg` command used brace paths without `.rs` suffix for file modules and emitted `No such file or directory` for those entries. The useful hits from that run still identified simulation-side imports of input/UI symbols in directory modules such as `time_system`, `scripting`, `setup`, and `player`.

## Main App Composition

`macrocosmo/src/main.rs` currently composes all runtime pieces directly:

```text
DefaultPlugins
KeybindingPlugin
GameTimePlugin
GalaxyPlugin
PlayerPlugin
CommunicationPlugin
VisualizationPlugin
KnowledgePlugin
ShipPlugin
ColonyPlugin
ScriptingPlugin
TechnologyPlugin
EventSystemPlugin
EventsPlugin
SpeciesPlugin
ShipDesignPlugin
DeepSpacePlugin
GameSetupPlugin
NotificationsPlugin
FactionRelationsPlugin
ChoicesPlugin
AiPlugin
CasusBelliPlugin
ObserverPlugin
ReflectRegistrationPlugin
UiPlugin
remote_plugin / RemoteHttpPlugin under feature remote
```

Observation:

- There is no plugin-level boundary between authoritative game simulation and human/tool interaction.
- `VisualizationPlugin`, `KeybindingPlugin`, `ObserverPlugin`, remote/BRP, and `UiPlugin` are composed beside core simulation plugins.
- A future first step can wrap the same plugin list into `SimulationPlugin` and `InteractionsPlugin` without moving every source file at once.

## Mixed Responsibility Evidence

### `time_system`

`macrocosmo/src/time_system/mod.rs` currently installs both:

```rust
advance_game_time
handle_speed_controls
```

`advance_game_time` is simulation-owned. It updates `GameClock` and gates time on pending ship routes.

`handle_speed_controls` is interaction-owned. It reads:

```rust
Res<ButtonInput<KeyCode>>
Option<Res<crate::input::KeybindingRegistry>>
```

Conclusion:

- `GameClock`, `GameSpeed`, and `advance_game_time` belong in simulation.
- speed-control key handling belongs in interactions.
- This is the safest first mixed-module split because it has a clear ownership boundary and small surface area.

### UI and visualization read simulation types directly

Representative interaction-side imports:

```text
ui/outline.rs -> colony, galaxy, knowledge, ship, ship_design
ui/context_menu.rs -> colony, galaxy, knowledge, ship, ship_design, technology, time_system
ui/system_panel/mod.rs -> colony, galaxy, knowledge, scripting, ship, time_system
visualization/stars.rs -> colony, galaxy, knowledge, ship, technology, time_system
visualization/ships.rs -> galaxy, knowledge, ship, time_system
remote.rs -> scripting, time_system, visualization
```

Observation:

- This direction is acceptable for `interactions -> simulation`.
- The risk is reverse dependency: simulation-side code importing UI/input/rendering symbols.

### Simulation-side imports of interaction concepts

Current examples found:

```text
time_system/mod.rs -> ButtonInput<KeyCode>, crate::input::KeybindingRegistry
scripting/esc_notifications.rs -> crate::ui::situation_center
setup/mod.rs -> crate::observer::{in_observer_mode, not_in_observer_mode}
player/mod.rs -> crate::observer::not_in_observer_mode
player/mod.rs -> ButtonInput<KeyCode>
```

Interpretation:

- `time_system` and `player` have direct input/key dependencies that should move to interactions.
- `scripting/esc_notifications.rs` suggests scripting can currently reach UI notification types; this should be inspected before crate extraction.
- `observer` is mixed: observer mode affects simulation setup, while controls/toggles are interaction concerns.

## Large / High-Churn Files

Largest Rust files from scan:

```text
5535 macrocosmo/src/persistence/savebag.rs
3652 macrocosmo/src/ai/command_consumer.rs
3016 macrocosmo/src/ui/mod.rs
2518 macrocosmo/src/scripting/gamestate_scope.rs
2469 macrocosmo/src/knowledge/mod.rs
2469 macrocosmo/src/faction/mod.rs
2288 macrocosmo/src/ui/system_panel/mod.rs
2231 macrocosmo/src/ship_design.rs
2161 macrocosmo/src/knowledge/facts.rs
1872 macrocosmo/src/deep_space/mod.rs
1760 macrocosmo/src/setup/mod.rs
1672 macrocosmo/src/ai/npc_decision.rs
1667 macrocosmo/src/ui/ship_panel.rs
1599 macrocosmo/src/ai/command_outbox.rs
1539 macrocosmo/src/ship/mod.rs
```

Observation:

- The large-file problem exists on both sides: UI files and simulation files are both large.
- Therefore file-size cleanup alone will not solve the rendering/game-loop coupling.
- Plugin and dependency direction boundaries should come first.

## Boundary Inference

Useful dependency direction:

```text
interactions -> simulation -> core
ai ----------> core  (optional later)
simulation -> ai     (adapter/runtime integration)
```

Disallowed direction:

```text
simulation -> interactions
simulation -> ui / visualization / input / remote
core -> simulation / interactions / bevy / mlua / postcard
ai -> simulation / bevy
```

Immediate practical target:

```text
macrocosmo/src/simulation
macrocosmo/src/interactions
```

before adding new crates.

## Evidence Gaps / Follow-Up Checks

Before the first implementation PR, run cleaner targeted scans:

```text
rg "bevy_egui|egui::|crate::ui|crate::visualization|crate::input|KeyCode|ButtonInput|Window" macrocosmo/src
rg "crate::observer" macrocosmo/src
rg "crate::ui::" macrocosmo/src/scripting macrocosmo/src/setup macrocosmo/src/player macrocosmo/src/time_system
```

After creating `simulation/`:

```text
rg "bevy_egui|egui::|crate::interactions|crate::ui|crate::visualization|crate::input|KeyCode|ButtonInput|Window" macrocosmo/src/simulation
```

The second command should become a regression guard.

