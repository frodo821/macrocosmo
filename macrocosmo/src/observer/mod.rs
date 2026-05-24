//! Observer mode (#214) — run the game without a Player entity.
//!
//! Observer mode is activated via `--no-player` on the command line. It:
//!
//! * spawns one full NPC Empire per registered Faction (no `PlayerEmpire`);
//! * gates player-specific systems and command-issuance UI;
//! * provides a top-bar faction selector synced with the AI Debug UI Governor tab;
//! * exits automatically on `--time-horizon`, all-empires-eliminated, or Esc.
//!
//! Reproducibility helpers (also available outside observer mode):
//!
//! * `--seed N` — deterministic galaxy generation seed
//! * `--speed S` — initial game speed (hexadies per real second)
//!
//! See `cli.rs` for the CLI parser.
//!
//! ## Mode kinds (#490)
//!
//! [`ObserverModeKind`] is the 3-variant enum that branches the
//! "what does the viewer see?" contract:
//!
//! * [`ObserverModeKind::Disabled`] — normal single-player. Renders from
//!   the `PlayerEmpire` perspective using its `KnowledgeStore`.
//! * [`ObserverModeKind::EmpireView`] — observer mode (`--no-player` /
//!   `--observer`). Renders from the empire selected in [`ObserverView`]
//!   using THAT empire's `KnowledgeStore` (light-coherent, #499).
//! * [`ObserverModeKind::Omniscient`] — god view (#490). Bypasses every
//!   `KnowledgeStore` and renders realtime ECS ground truth. Dev-only;
//!   toggled via `ui.toggle_omniscient` (default F9).

pub mod cli;
mod exit;

pub use cli::CliArgs;
pub use exit::{check_all_empires_eliminated, check_time_horizon, esc_to_exit};

use bevy::prelude::*;

use crate::time_system::GameSpeed;

/// #490: The three observer-mode kinds. See module docs for semantics.
///
/// The `viewing` target for `EmpireView` lives on [`ObserverView`]
/// (kept separate so the top-bar selector and the AI debug F10 panel
/// can mutate / mirror it without churning this enum).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Reflect)]
pub enum ObserverModeKind {
    /// Normal single-player. PlayerEmpire view, PlayerEmpire knowledge.
    #[default]
    Disabled,
    /// Observer mode (`--no-player` / `--observer`). Views another
    /// empire through its `KnowledgeStore` (light-coherent).
    EmpireView,
    /// God view (#490). Bypasses every `KnowledgeStore` and renders
    /// realtime ECS ground truth. Dev-only toggle.
    Omniscient,
}

/// #490: Subset of [`ObserverModeKind`] that can be saved as the
/// "previous mode" for Omniscient toggle restoration. `Omniscient`
/// itself is intentionally excluded — restoring `Omniscient` from
/// `Omniscient` is a no-op semantic trap (= double F9 same frame
/// converting `Omniscient → Omniscient → Omniscient` indefinitely, or
/// save/restore corruption where a previously-stuck-Omniscient state
/// loses its escape hatch).
///
/// The type-system invariant guards against future maintainers
/// accidentally storing `Omniscient` into [`ObserverMode::previous_kind`]:
/// the `from_observer_kind` constructor returns `None` for that case.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum NonOmniscientKind {
    /// Restores to normal single-player on Omniscient toggle-off.
    #[default]
    Disabled,
    /// Restores to observer-mode empire view on Omniscient toggle-off.
    EmpireView,
}

impl NonOmniscientKind {
    /// Lift the newtype into the full enum (= the value to write into
    /// `ObserverMode.kind` on toggle-off).
    pub fn to_observer_kind(self) -> ObserverModeKind {
        match self {
            Self::Disabled => ObserverModeKind::Disabled,
            Self::EmpireView => ObserverModeKind::EmpireView,
        }
    }

    /// Narrow the full enum into the newtype. Returns `None` for
    /// `Omniscient` — the type-system gate that pins the invariant
    /// described in the type docstring.
    pub fn from_observer_kind(kind: ObserverModeKind) -> Option<Self> {
        match kind {
            ObserverModeKind::Disabled => Some(Self::Disabled),
            ObserverModeKind::EmpireView => Some(Self::EmpireView),
            ObserverModeKind::Omniscient => None,
        }
    }
}

/// Global observer-mode resource. `kind == Disabled` in normal play.
///
/// **Migration note (#490):** the previous `enabled: bool` field was
/// replaced by [`Self::kind`]. The classic "is observer active?"
/// `enabled()` accessor was then **deleted** during fold-in to force
/// every call site to surface its intent (`is_empire_view()` —
/// spawn-architecture "no PlayerEmpire" mode; `is_omniscient()` —
/// god-view ground-truth branch). Run-conditions that need the union
/// of both (= `kind != Disabled`) call [`Self::is_any_observer`].
#[derive(Resource, Debug, Clone, Default, Reflect)]
#[reflect(Resource)]
pub struct ObserverMode {
    /// Which observer mode is currently active.
    pub kind: ObserverModeKind,
    /// Optional deterministic seed (copied from `RngSeed` for convenience).
    pub seed: Option<u64>,
    /// Auto-exit hexadies. `None` = manual termination only.
    pub time_horizon: Option<i64>,
    /// Initial `GameSpeed.hexadies_per_second`. Applied at Startup.
    pub initial_speed: Option<f64>,
    /// When `true`, the UI is read-only: context menus and ship panel
    /// commands are suppressed. Set by `--observer`.
    pub read_only: bool,
    /// #490: Track the prior non-omniscient kind so the Omniscient
    /// toggle can restore the previous mode (e.g. `EmpireView` → flick
    /// to `Omniscient` and back returns to `EmpireView`, not
    /// `Disabled`). The [`NonOmniscientKind`] newtype enforces that
    /// `Omniscient` itself can never be stashed here.
    pub previous_kind: Option<NonOmniscientKind>,
}

impl ObserverMode {
    /// True when the active mode is [`ObserverModeKind::EmpireView`].
    ///
    /// Use this for branches tied to the spawn-architecture
    /// "no PlayerEmpire" mode (= the `--no-player` / `--observer` CLI
    /// flags). It is **not** true when Omniscient is toggled on top of
    /// a normal single-player session.
    pub fn is_empire_view(&self) -> bool {
        matches!(self.kind, ObserverModeKind::EmpireView)
    }

    /// True when the active mode is [`ObserverModeKind::Omniscient`].
    /// All `KnowledgeStore` gates collapse to ground-truth realtime
    /// ECS reads when this is set.
    pub fn is_omniscient(&self) -> bool {
        matches!(self.kind, ObserverModeKind::Omniscient)
    }

    /// True when any non-`Disabled` mode is active (= `EmpireView` or
    /// `Omniscient`). Use only for genuine union semantics — most
    /// branches want [`Self::is_empire_view`] or [`Self::is_omniscient`]
    /// explicitly.
    pub fn is_any_observer(&self) -> bool {
        !matches!(self.kind, ObserverModeKind::Disabled)
    }
}

/// Current empire the observer is inspecting. One-way mirrored to
/// `AiDebugUi::governor::GovernorState::faction` so the F10 panel follows
/// the top-bar selector.
#[derive(Resource, Debug, Clone, Default, Reflect)]
#[reflect(Resource)]
pub struct ObserverView {
    /// The `Empire` entity being focused. `None` until the selector has
    /// been initialised from the spawned empire list.
    ///
    /// Despite the historical naming (the field outlived an earlier
    /// design where focus was stored as a `Faction` entity), the
    /// initialiser (`setup::init_observer_view`) queries
    /// `With<Empire>` and the top-bar selector iterates `With<Empire>`,
    /// so consumers can dereference this as an Empire entity.
    pub viewing: Option<Entity>,
}

/// Global RNG seed for galaxy generation. Populated from the CLI whether
/// or not observer mode is enabled so the flag is useful for bug repros.
#[derive(Resource, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Resource)]
pub struct RngSeed(pub Option<u64>);

/// #499 / #490: Resolve the viewing empire entity from a `&World`.
///
/// 3-state contract:
/// * [`ObserverModeKind::Disabled`] → `Some(PlayerEmpire)` (singleton).
/// * [`ObserverModeKind::EmpireView`] → `Some(ObserverView.viewing)` if
///   set, else `None`.
/// * [`ObserverModeKind::Omniscient`] → `None` (god view = no single
///   "viewing empire"; callers fall through to realtime ECS).
///
/// This is the single source of truth for the empire-view contract
/// across `&World`-accessing call sites (e.g. `situation_center`
/// tabs). The Query-based mirror is `ui::mod::resolve_ui_empire_raw`,
/// kept identical in spirit; if the contract changes, update both in
/// lockstep.
pub fn resolve_viewing_empire(world: &World) -> Option<Entity> {
    let mode = world.get_resource::<ObserverMode>();
    let kind = mode.map(|m| m.kind).unwrap_or_default();
    match kind {
        ObserverModeKind::Disabled => {
            let mut q = world.try_query::<(Entity, &crate::player::PlayerEmpire)>()?;
            q.iter(world).next().map(|(e, _)| e)
        }
        ObserverModeKind::EmpireView => world.get_resource::<ObserverView>()?.viewing,
        ObserverModeKind::Omniscient => None,
    }
}

/// Run-condition: observer mode is active in its spawn-architecture
/// sense (= `EmpireView`; the CLI-driven "no PlayerEmpire" mode).
///
/// **Why not `is_any_observer`?** This gates spawn-time setup
/// (player-empire spawn, observer-view init, observer exit / horizon
/// systems). Those are tied to the boot-time empire architecture, not
/// to the runtime god-view toggle: a player who F9-toggles Omniscient
/// mid-game must not suddenly start firing `esc_to_exit` (= app quit)
/// or have `init_observer_view` re-run.
pub fn in_observer_mode(o: Res<ObserverMode>) -> bool {
    o.is_empire_view()
}

/// Run-condition: observer mode is not active in its spawn-architecture
/// sense (= a `PlayerEmpire` exists — `Disabled` or `Omniscient` on top
/// of a player game). See [`in_observer_mode`] for the rationale.
pub fn not_in_observer_mode(o: Res<ObserverMode>) -> bool {
    !o.is_empire_view()
}

/// Bevy plugin that registers observer resources, exit systems, and
/// wiring that must run regardless of whether observer mode is enabled
/// (run-conditions short-circuit inside each system).
pub struct ObserverPlugin;

impl Plugin for ObserverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ObserverMode>()
            .init_resource::<ObserverView>()
            .init_resource::<RngSeed>()
            // #439 Phase 3: gameplay config (initial speed) is a
            // new-game construction step.
            .add_systems(
                OnEnter(crate::game_state::GameState::NewGame),
                apply_initial_speed.run_if(in_observer_mode),
            )
            .add_systems(
                Update,
                (
                    check_time_horizon,
                    check_all_empires_eliminated,
                    esc_to_exit,
                    sync_observer_view_to_governor,
                )
                    .run_if(in_observer_mode),
            )
            // #490: Omniscient toggle (default F9). Runs in every
            // observer-spawn-architecture mode so a dev can flip into
            // god view from either normal play or `--no-player`. The
            // `run_if(in_state(InGame))` keeps the toggle inert during
            // main-menu / loading, matching other UI-action systems.
            .add_systems(
                Update,
                toggle_omniscient_mode.run_if(in_state(crate::game_state::GameState::InGame)),
            );
    }
}

/// Startup system that applies `ObserverMode.initial_speed` to
/// `GameSpeed`. Gated on `in_observer_mode` at registration.
pub fn apply_initial_speed(mode: Res<ObserverMode>, mut speed: ResMut<GameSpeed>) {
    if let Some(s) = mode.initial_speed {
        speed.hexadies_per_second = s;
        if s > 0.0 {
            speed.previous_speed = s;
        }
        info!("Observer mode: initial speed set to {} hd/s", s);
    }
}

/// Run-condition: observer mode is active AND read-only.
///
/// Read-only is a `--observer`-specific flag set at CLI parse time;
/// today only `EmpireView` can carry it (Omniscient is a runtime
/// toggle), so this collapses to `is_empire_view() && read_only`.
pub fn in_observer_read_only(o: Res<ObserverMode>) -> bool {
    o.is_empire_view() && o.read_only
}

/// One-way mirror from `ObserverView.viewing` (Faction entity) to
/// `AiDebugUi::GovernorState::faction` (`u32` from `to_ai_faction`). This
/// makes the F10 Governor tab follow the top-bar selector.
///
/// The `AiDebugUi` resource is optional so this system can run in
/// headless test apps that don't register `UiPlugin`.
pub fn sync_observer_view_to_governor(
    view: Res<ObserverView>,
    ui: Option<ResMut<crate::ui::ai_debug::AiDebugUi>>,
) {
    let Some(mut ui) = ui else {
        return;
    };
    if let Some(faction_entity) = view.viewing {
        let id = crate::ai::convert::to_ai_faction(faction_entity);
        ui.governor.faction = id.0;
    }
}

/// #490: Toggle [`ObserverModeKind::Omniscient`] on/off.
///
/// * If currently `Omniscient` → restore `previous_kind` (or `Disabled`
///   if unset).
/// * Otherwise → save current kind into `previous_kind` and switch to
///   `Omniscient`.
///
/// Bound to `ui.toggle_omniscient` (default `F9`) via the keybinding
/// registry. Falls back to a hardcoded `F9` check when the registry is
/// not present (headless tests with no `KeybindingPlugin`).
pub fn toggle_omniscient_mode(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    mut mode: ResMut<ObserverMode>,
) {
    // `ButtonInput<KeyCode>` is missing in headless test apps that only
    // load `MinimalPlugins`; the wrapper lets the system be a no-op in
    // that case instead of panicking on missing resource.
    let Some(keys) = keys else {
        return;
    };
    let pressed = match keybindings.as_deref() {
        Some(kb) => kb.is_just_pressed(crate::input::actions::UI_TOGGLE_OMNISCIENT, &keys),
        None => keys.just_pressed(KeyCode::F9),
    };
    if !pressed {
        return;
    }
    if mode.is_omniscient() {
        let restore = mode
            .previous_kind
            .take()
            .unwrap_or(NonOmniscientKind::Disabled);
        mode.kind = restore.to_observer_kind();
        info!("Omniscient mode OFF (restored {:?})", mode.kind);
    } else {
        // The newtype refuses `Omniscient` (= `None`). We only reach
        // this branch when the current kind is `Disabled` or
        // `EmpireView`, so the `expect` is a contract-by-construction
        // pin rather than a panic risk in practice.
        mode.previous_kind = NonOmniscientKind::from_observer_kind(mode.kind);
        mode.kind = ObserverModeKind::Omniscient;
        info!("Omniscient mode ON (god view)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_mode_default_is_inactive() {
        let mode = ObserverMode::default();
        assert!(!mode.is_any_observer());
        assert!(!mode.is_empire_view());
        assert!(!mode.is_omniscient());
        assert!(!mode.read_only);
        assert!(mode.seed.is_none());
        assert!(mode.time_horizon.is_none());
        assert!(mode.initial_speed.is_none());
        assert_eq!(mode.kind, ObserverModeKind::Disabled);
    }

    #[test]
    fn observer_mode_read_only_field() {
        let mode = ObserverMode {
            kind: ObserverModeKind::EmpireView,
            read_only: true,
            ..Default::default()
        };
        assert!(mode.is_any_observer());
        assert!(mode.is_empire_view());
        assert!(mode.read_only);
    }

    #[test]
    fn observer_mode_no_player_without_read_only() {
        // --no-player sets kind=EmpireView but read_only=false
        let mode = ObserverMode {
            kind: ObserverModeKind::EmpireView,
            read_only: false,
            ..Default::default()
        };
        assert!(mode.is_any_observer());
        assert!(mode.is_empire_view());
        assert!(!mode.read_only);
    }

    /// #490: `Omniscient` is `is_any_observer()` but
    /// distinguishable via `is_omniscient()`.
    #[test]
    fn observer_mode_omniscient_predicates() {
        let mode = ObserverMode {
            kind: ObserverModeKind::Omniscient,
            ..Default::default()
        };
        assert!(mode.is_any_observer());
        assert!(!mode.is_empire_view());
        assert!(mode.is_omniscient());
    }

    /// #490 fold-in: Omniscient toggle flow with the
    /// [`NonOmniscientKind`] newtype — start Disabled, flip to
    /// Omniscient, flip back to Disabled.
    #[test]
    fn observer_mode_omniscient_toggle_restores_disabled() {
        let mut mode = ObserverMode::default();
        // Flip on.
        mode.previous_kind = NonOmniscientKind::from_observer_kind(mode.kind);
        mode.kind = ObserverModeKind::Omniscient;
        assert!(mode.is_omniscient());
        // Flip off — restore.
        let restore = mode.previous_kind.take().unwrap();
        mode.kind = restore.to_observer_kind();
        assert_eq!(mode.kind, ObserverModeKind::Disabled);
        assert!(!mode.is_any_observer());
    }

    /// #490 fold-in: Omniscient toggle preserves EmpireView across a
    /// flick via the [`NonOmniscientKind`] newtype.
    #[test]
    fn observer_mode_omniscient_toggle_preserves_empire_view() {
        let mut mode = ObserverMode {
            kind: ObserverModeKind::EmpireView,
            ..Default::default()
        };
        // Flip on.
        mode.previous_kind = NonOmniscientKind::from_observer_kind(mode.kind);
        mode.kind = ObserverModeKind::Omniscient;
        assert!(mode.is_omniscient());
        // Flip off — restore to EmpireView.
        let restore = mode.previous_kind.take().unwrap();
        mode.kind = restore.to_observer_kind();
        assert_eq!(mode.kind, ObserverModeKind::EmpireView);
        assert!(mode.is_empire_view());
        assert!(mode.is_any_observer());
    }

    /// #490 fold-in: `NonOmniscientKind::from_observer_kind` rejects
    /// `Omniscient` (= the type-system invariant that prevents an
    /// `Omniscient → Omniscient` restore loop).
    #[test]
    fn non_omniscient_kind_rejects_omniscient() {
        assert_eq!(
            NonOmniscientKind::from_observer_kind(ObserverModeKind::Disabled),
            Some(NonOmniscientKind::Disabled)
        );
        assert_eq!(
            NonOmniscientKind::from_observer_kind(ObserverModeKind::EmpireView),
            Some(NonOmniscientKind::EmpireView)
        );
        assert_eq!(
            NonOmniscientKind::from_observer_kind(ObserverModeKind::Omniscient),
            None,
            "Omniscient must be rejected to prevent Omniscient→Omniscient restore"
        );
    }

    /// #490 fold-in: round-trip narrow then widen preserves the
    /// underlying kind.
    #[test]
    fn non_omniscient_kind_round_trip() {
        for kind in [ObserverModeKind::Disabled, ObserverModeKind::EmpireView] {
            let narrowed = NonOmniscientKind::from_observer_kind(kind).expect("narrows");
            assert_eq!(narrowed.to_observer_kind(), kind);
        }
    }
}
