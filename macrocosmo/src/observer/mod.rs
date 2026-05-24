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

/// Global observer-mode resource. `kind == Disabled` in normal play.
///
/// **Migration note (#490):** the previous `enabled: bool` field is
/// replaced by [`Self::kind`]. The classic "is observer active?" check
/// is now [`Self::enabled`] (a method). The god-view branch is
/// [`Self::is_omniscient`].
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
    /// `Disabled`).
    pub previous_kind: Option<ObserverModeKind>,
}

impl ObserverMode {
    /// True when observer mode is in any non-`Disabled` state. Mirrors
    /// the pre-#490 `enabled: bool` field's semantics: any branch where
    /// the player perspective should *not* be the canonical
    /// `PlayerEmpire`.
    pub fn enabled(&self) -> bool {
        !matches!(self.kind, ObserverModeKind::Disabled)
    }

    /// True when the active mode is [`ObserverModeKind::EmpireView`].
    pub fn is_empire_view(&self) -> bool {
        matches!(self.kind, ObserverModeKind::EmpireView)
    }

    /// True when the active mode is [`ObserverModeKind::Omniscient`].
    /// All `KnowledgeStore` gates collapse to ground-truth realtime
    /// ECS reads when this is set.
    pub fn is_omniscient(&self) -> bool {
        matches!(self.kind, ObserverModeKind::Omniscient)
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

/// Run-condition: observer mode is active (any non-`Disabled` kind).
pub fn in_observer_mode(o: Res<ObserverMode>) -> bool {
    o.enabled()
}

/// Run-condition: observer mode is not active (normal single-player).
pub fn not_in_observer_mode(o: Res<ObserverMode>) -> bool {
    !o.enabled()
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
            // #490: Omniscient toggle (default F9). Runs in every mode
            // so the dev can flip into god view from a normal play
            // session too.
            .add_systems(Update, toggle_omniscient_mode);
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
pub fn in_observer_read_only(o: Res<ObserverMode>) -> bool {
    o.enabled() && o.read_only
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
            .unwrap_or(ObserverModeKind::Disabled);
        mode.kind = restore;
        info!("Omniscient mode OFF (restored {:?})", restore);
    } else {
        mode.previous_kind = Some(mode.kind);
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
        assert!(!mode.enabled());
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
        assert!(mode.enabled());
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
        assert!(mode.enabled());
        assert!(mode.is_empire_view());
        assert!(!mode.read_only);
    }

    /// #490: `Omniscient` is `enabled()` (same as EmpireView) but
    /// distinguishable via `is_omniscient()`.
    #[test]
    fn observer_mode_omniscient_predicates() {
        let mode = ObserverMode {
            kind: ObserverModeKind::Omniscient,
            ..Default::default()
        };
        assert!(mode.enabled());
        assert!(!mode.is_empire_view());
        assert!(mode.is_omniscient());
    }

    /// #490: Omniscient toggle flow — start Disabled, flip to
    /// Omniscient, flip back to Disabled.
    #[test]
    fn observer_mode_omniscient_toggle_restores_disabled() {
        let mut mode = ObserverMode::default();
        // Flip on.
        mode.previous_kind = Some(mode.kind);
        mode.kind = ObserverModeKind::Omniscient;
        assert!(mode.is_omniscient());
        // Flip off — restore.
        let restore = mode.previous_kind.take().unwrap();
        mode.kind = restore;
        assert_eq!(mode.kind, ObserverModeKind::Disabled);
        assert!(!mode.enabled());
    }

    /// #490: Omniscient toggle preserves EmpireView across a flick.
    #[test]
    fn observer_mode_omniscient_toggle_preserves_empire_view() {
        let mut mode = ObserverMode {
            kind: ObserverModeKind::EmpireView,
            ..Default::default()
        };
        // Flip on.
        mode.previous_kind = Some(mode.kind);
        mode.kind = ObserverModeKind::Omniscient;
        assert!(mode.is_omniscient());
        // Flip off — restore to EmpireView.
        let restore = mode.previous_kind.take().unwrap();
        mode.kind = restore;
        assert_eq!(mode.kind, ObserverModeKind::EmpireView);
        assert!(mode.is_empire_view());
        assert!(mode.enabled());
    }
}
