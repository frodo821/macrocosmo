//! #347: In-game keybinding manager.
//!
//! Centralises every player-facing keybind behind a single
//! [`KeybindingRegistry`] resource so individual systems no longer hard-code
//! `KeyCode::*` checks. The registry maps stable `action_id` strings (e.g.
//! `"ui.toggle_situation_center"`) to [`KeyCombo`]s so the player can rebind
//! actions at runtime and persist overrides to disk.
//!
//! ## Components
//!
//! * [`KeyCombo`] — a (key, ctrl/shift/alt/super) tuple.
//! * [`KeybindingRegistry`] — `Resource` storing `action_id → KeyCombo`.
//!   Tracks both the *current* binding (subject to player overrides) and the
//!   default binding so a Reset-to-Defaults action can revert.
//! * [`KeybindingPlugin`] — wires the registry into the Bevy app and seeds
//!   default bindings.
//! * [`config`] — TOML save/load against the user config dir
//!   (`keybindings.toml` under a platform-appropriate path).
//!
//! ## Out of scope (v1, see #347)
//!
//! * Rebinding UI (settings panel sub-section + click-to-capture). Tracked
//!   separately; a follow-up PR consumes this registry.
//! * Chord / sequence bindings (`Ctrl+K → S`).
//! * Context-sensitive bindings.
//! * Lua-defined bindings.
//! * Mouse / gamepad bindings.
//!
//! ## Migration intent
//!
//! Default keymap mirrors the previously-hardcoded behaviour exactly — this
//! module's introduction is a pure refactor at the user level. New keybinds
//! should call [`KeybindingRegistry::register_default`] inside a plugin's
//! `build` (or, for Lua-defined actions later, at script load) instead of
//! reading `KeyCode` directly.

use std::collections::HashMap;

use bevy::prelude::*;

pub mod config;

/// A keybinding combo: a primary key plus modifier flags.
///
/// Stored canonically with `ctrl/shift/alt/super_key` as bools so we don't
/// have to distinguish left/right modifier variants at lookup time
/// (matching player intuition — "Ctrl+S" is "Ctrl+S" regardless of which
/// Ctrl key was used).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct KeyCombo {
    /// The primary key (non-modifier — e.g. `KeyCode::F2`, `KeyCode::Space`).
    /// Serialised as a stable string label via [`crate::input::config`].
    #[serde(with = "self::keycode_serde")]
    pub key: KeyCode,
    #[serde(default, skip_serializing_if = "is_false")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub alt: bool,
    /// "Super" / Cmd / Win key.
    #[serde(default, skip_serializing_if = "is_false")]
    pub super_key: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl KeyCombo {
    /// Construct a plain (no-modifier) combo.
    pub const fn key(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
            super_key: false,
        }
    }

    /// Builder: require Ctrl modifier.
    pub const fn with_ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    /// Builder: require Shift modifier.
    pub const fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    /// Builder: require Alt modifier.
    pub const fn with_alt(mut self) -> Self {
        self.alt = true;
        self
    }

    /// Builder: require Super (Cmd / Win) modifier.
    pub const fn with_super(mut self) -> Self {
        self.super_key = true;
        self
    }

    /// Are this combo's modifier requirements satisfied by the current
    /// `ButtonInput<KeyCode>` snapshot? Treats left and right modifier keys
    /// as interchangeable.
    fn modifiers_match(&self, keys: &ButtonInput<KeyCode>) -> bool {
        let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
        let sup = keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight);
        ctrl == self.ctrl && shift == self.shift && alt == self.alt && sup == self.super_key
    }

    /// Was this combo just pressed this frame? (Edge-triggered — fires once
    /// per press.) Returns false unless the modifier set matches exactly.
    pub fn just_pressed(&self, keys: &ButtonInput<KeyCode>) -> bool {
        keys.just_pressed(self.key) && self.modifiers_match(keys)
    }

    /// Is this combo currently held down? (Level-triggered — true every
    /// frame the key is down with matching modifiers.)
    pub fn pressed(&self, keys: &ButtonInput<KeyCode>) -> bool {
        keys.pressed(self.key) && self.modifiers_match(keys)
    }

    /// Human-readable label (e.g. `"Ctrl+Shift+F2"`). Used by future
    /// rebinding UI; also handy in logs.
    pub fn display(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.super_key {
            parts.push("Super");
        }
        let key = keycode_serde::keycode_label(self.key).unwrap_or("?");
        if parts.is_empty() {
            key.to_string()
        } else {
            format!("{}+{}", parts.join("+"), key)
        }
    }
}

/// Resource: maps stable action ids (e.g. `"ui.toggle_situation_center"`)
/// to the [`KeyCombo`] currently bound to that action.
///
/// Plugins seed defaults via [`Self::register_default`] during their `build`
/// step. Player overrides loaded from `keybindings.toml` later in the
/// startup sequence layer on top via [`Self::set`]; the original default is
/// remembered so a single binding (or the entire map) can be reset.
#[derive(Resource, Debug, Clone, Default)]
pub struct KeybindingRegistry {
    /// Currently active binding for each action.
    bindings: HashMap<String, KeyCombo>,
    /// Hardcoded default binding for each registered action — survives
    /// [`Self::set`] overrides so [`Self::reset_to_defaults`] can revert.
    defaults: HashMap<String, KeyCombo>,
}

impl KeybindingRegistry {
    /// Construct an empty registry. Most callers should use
    /// [`Self::with_engine_defaults`] instead.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a registry pre-populated with every default binding the
    /// game ships with. This is the canonical entry-point for production
    /// code; tests can either use this or build their own minimal registry.
    pub fn with_engine_defaults() -> Self {
        let mut r = Self::new();
        register_engine_defaults(&mut r);
        r
    }

    /// Register a new action with its default binding. If the same
    /// `action_id` is registered twice, the latest registration wins (and
    /// a warning is logged) — this is a programmer error, not a player
    /// configuration issue, so it is louder than [`set`](Self::set).
    pub fn register_default(&mut self, action_id: impl Into<String>, combo: KeyCombo) {
        let id = action_id.into();
        if let Some(prev) = self.defaults.insert(id.clone(), combo) {
            warn!(
                "KeybindingRegistry: action '{}' re-registered (was {}, now {})",
                id,
                prev.display(),
                combo.display()
            );
        }
        // Initialise the active binding from the default unless the player
        // already overrode it via `set`.
        self.bindings.entry(id).or_insert(combo);
    }

    /// Override the binding for an existing action. Unknown action ids are
    /// ignored with a warning — this is the contract for runtime config
    /// loading, where a stale `keybindings.toml` may reference actions that
    /// have since been removed.
    pub fn set(&mut self, action_id: &str, combo: KeyCombo) {
        if !self.defaults.contains_key(action_id) {
            warn!(
                "KeybindingRegistry: ignoring override for unknown action '{}'",
                action_id
            );
            return;
        }
        self.bindings.insert(action_id.to_string(), combo);
    }

    /// Look up the currently-active combo for `action_id`.
    pub fn get(&self, action_id: &str) -> Option<KeyCombo> {
        self.bindings.get(action_id).copied()
    }

    /// Look up the default (un-overridden) combo for `action_id`.
    pub fn default_for(&self, action_id: &str) -> Option<KeyCombo> {
        self.defaults.get(action_id).copied()
    }

    /// Edge-triggered: did the action's bound combo just fire this frame?
    /// Returns `false` for unknown action ids (with no warning — hot path).
    pub fn is_just_pressed(&self, action_id: &str, keys: &ButtonInput<KeyCode>) -> bool {
        self.bindings
            .get(action_id)
            .is_some_and(|c| c.just_pressed(keys))
    }

    /// Level-triggered: is the action's bound combo held down this frame?
    pub fn is_pressed(&self, action_id: &str, keys: &ButtonInput<KeyCode>) -> bool {
        self.bindings
            .get(action_id)
            .is_some_and(|c| c.pressed(keys))
    }

    /// Reset the binding for a single action to its registered default.
    pub fn reset_one(&mut self, action_id: &str) {
        if let Some(default) = self.defaults.get(action_id).copied() {
            self.bindings.insert(action_id.to_string(), default);
        }
    }

    /// Reset every binding to its registered default. Drops any player
    /// overrides.
    pub fn reset_to_defaults(&mut self) {
        self.bindings = self.defaults.clone();
    }

    /// Iterate `(action_id, current_binding)` pairs. Order is unspecified;
    /// rebinding UI should sort for display.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &KeyCombo)> {
        self.bindings.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of registered actions (defaults known).
    pub fn len(&self) -> usize {
        self.defaults.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defaults.is_empty()
    }

    /// Find every action whose currently-bound combo collides with another
    /// action. Returns groups keyed by [`KeyCombo`]; each group contains
    /// 2+ action ids.
    ///
    /// Pure inspection — does not mutate the registry. The caller decides
    /// whether to surface this to the player. [`detect_and_warn_conflicts`]
    /// runs this against the registry and emits one `warn!` per group.
    pub fn detect_conflicts(&self) -> HashMap<KeyCombo, Vec<String>> {
        let mut by_combo: HashMap<KeyCombo, Vec<String>> = HashMap::new();
        for (action, combo) in &self.bindings {
            by_combo.entry(*combo).or_default().push(action.clone());
        }
        by_combo.retain(|_, v| {
            v.sort();
            v.len() > 1
        });
        by_combo
    }
}

/// Walk the registry, log one warning per binding-collision group. Called
/// from the keybinding plugin's startup system after the user override file
/// has been merged in. Safe to call repeatedly (idempotent — no state).
pub fn detect_and_warn_conflicts(registry: &KeybindingRegistry) {
    for (combo, actions) in registry.detect_conflicts() {
        warn!(
            "KeybindingRegistry: combo '{}' is bound to multiple actions: {}",
            combo.display(),
            actions.join(", ")
        );
    }
}

// ---------------------------------------------------------------------------
// Default keymap
// ---------------------------------------------------------------------------

/// Stable string ids for every action the engine ships with. Centralised
/// here (rather than `&'static str` literals scattered through call sites)
/// so a typo in one consumer is a compile-time error.
pub mod actions {
    // Time controls
    pub const TIME_TOGGLE_PAUSE: &str = "time.toggle_pause";
    pub const TIME_SPEED_UP: &str = "time.speed_up";
    pub const TIME_SPEED_DOWN: &str = "time.speed_down";

    // Camera controls
    pub const CAMERA_PAN_UP: &str = "camera.pan_up";
    pub const CAMERA_PAN_DOWN: &str = "camera.pan_down";
    pub const CAMERA_PAN_LEFT: &str = "camera.pan_left";
    pub const CAMERA_PAN_RIGHT: &str = "camera.pan_right";
    pub const CAMERA_PAN_UP_ALT: &str = "camera.pan_up_alt";
    pub const CAMERA_PAN_DOWN_ALT: &str = "camera.pan_down_alt";
    pub const CAMERA_PAN_LEFT_ALT: &str = "camera.pan_left_alt";
    pub const CAMERA_PAN_RIGHT_ALT: &str = "camera.pan_right_alt";
    pub const CAMERA_RECENTER: &str = "camera.recenter";

    // Selection
    pub const SELECTION_CANCEL: &str = "selection.cancel";

    // UI panel toggles
    pub const UI_TOGGLE_DIPLOMACY: &str = "ui.toggle_diplomacy_panel";
    pub const UI_TOGGLE_SITUATION_CENTER: &str = "ui.toggle_situation_center";
    pub const UI_TOGGLE_CONSOLE: &str = "ui.toggle_console";
    pub const UI_TOGGLE_AI_DEBUG: &str = "ui.toggle_ai_debug";

    // Observer mode
    pub const OBSERVER_EXIT: &str = "observer.exit";

    // Debug
    pub const DEBUG_LOG_PLAYER_INFO: &str = "debug.log_player_info";
}

/// Seed the registry with every engine-built-in action's default binding.
///
/// **Defaults must mirror pre-#347 hardcoded behaviour exactly** — the
/// keybinding refactor is intended to be invisible to the player on first
/// run.
fn register_engine_defaults(r: &mut KeybindingRegistry) {
    use actions::*;

    // --- Time controls (was: time_system::handle_speed_controls) ---
    r.register_default(TIME_TOGGLE_PAUSE, KeyCombo::key(KeyCode::Space));
    r.register_default(TIME_SPEED_UP, KeyCombo::key(KeyCode::Equal));
    r.register_default(TIME_SPEED_DOWN, KeyCombo::key(KeyCode::Minus));

    // --- Camera controls (was: visualization::camera::camera_controls) ---
    // WASD as primary, arrow keys as alt — matches existing behaviour where
    // both work simultaneously.
    r.register_default(CAMERA_PAN_UP, KeyCombo::key(KeyCode::KeyW));
    r.register_default(CAMERA_PAN_DOWN, KeyCombo::key(KeyCode::KeyS));
    r.register_default(CAMERA_PAN_LEFT, KeyCombo::key(KeyCode::KeyA));
    r.register_default(CAMERA_PAN_RIGHT, KeyCombo::key(KeyCode::KeyD));
    r.register_default(CAMERA_PAN_UP_ALT, KeyCombo::key(KeyCode::ArrowUp));
    r.register_default(CAMERA_PAN_DOWN_ALT, KeyCombo::key(KeyCode::ArrowDown));
    r.register_default(CAMERA_PAN_LEFT_ALT, KeyCombo::key(KeyCode::ArrowLeft));
    r.register_default(CAMERA_PAN_RIGHT_ALT, KeyCombo::key(KeyCode::ArrowRight));
    r.register_default(CAMERA_RECENTER, KeyCombo::key(KeyCode::Home));

    // --- Selection cancel (was: visualization::click_select_system Esc) ---
    r.register_default(SELECTION_CANCEL, KeyCombo::key(KeyCode::Escape));

    // --- UI panel toggles ---
    r.register_default(UI_TOGGLE_DIPLOMACY, KeyCombo::key(KeyCode::F2));
    r.register_default(UI_TOGGLE_SITUATION_CENTER, KeyCombo::key(KeyCode::F3));
    // Alt+F2 for console (was: ui::toggle_console).
    r.register_default(UI_TOGGLE_CONSOLE, KeyCombo::key(KeyCode::F2).with_alt());
    r.register_default(UI_TOGGLE_AI_DEBUG, KeyCombo::key(KeyCode::F10));

    // --- Observer exit (was: observer::exit::esc_to_exit) ---
    // Same physical key as SELECTION_CANCEL by design — the observer-mode
    // run_if guard keeps the systems mutually exclusive at runtime, so the
    // conflict warning here is benign.
    r.register_default(OBSERVER_EXIT, KeyCombo::key(KeyCode::Escape));

    // --- Debug ---
    r.register_default(DEBUG_LOG_PLAYER_INFO, KeyCombo::key(KeyCode::KeyI));
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// Bevy plugin: inserts a default-populated [`KeybindingRegistry`] and
/// (on startup) attempts to merge in the user's `keybindings.toml`.
/// Conflict detection runs once after the merge and emits warnings.
///
/// Headless tests that don't need persistence can skip this plugin and
/// insert a registry manually with [`KeybindingRegistry::with_engine_defaults`]
/// — see `full_test_app()`.
pub struct KeybindingPlugin;

impl Plugin for KeybindingPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(KeybindingRegistry::with_engine_defaults())
            .add_systems(Startup, load_user_overrides_system);
    }
}

/// Startup system: best-effort load of the user override file. Failures
/// are logged but never fatal — the engine defaults are always present.
fn load_user_overrides_system(mut registry: ResMut<KeybindingRegistry>) {
    match config::load_overrides_into(&mut registry) {
        Ok(config::LoadOutcome::Loaded { path, count }) => {
            info!(
                "Loaded {} keybinding override(s) from {}",
                count,
                path.display()
            );
        }
        Ok(config::LoadOutcome::Missing { path }) => {
            debug!("No keybinding override file at {}", path.display());
        }
        Err(e) => {
            warn!("Failed to load keybinding overrides: {}", e);
        }
    }
    detect_and_warn_conflicts(&registry);
}

// ---------------------------------------------------------------------------
// KeyCode <-> string serde helper
// ---------------------------------------------------------------------------

mod keycode_serde {
    //! Stable string mapping for `bevy::input::KeyCode` so [`KeyCombo`]s
    //! survive round-tripping through TOML. The label set is the same one
    //! used by the BRP `key_press` handler in `remote.rs` — keeping them
    //! aligned means a binding stored in `keybindings.toml` matches the
    //! key name a player would type into a remote test invocation.

    use bevy::input::keyboard::KeyCode;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(key: &KeyCode, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(keycode_label(*key).ok_or_else(|| {
            serde::ser::Error::custom(format!("KeyCode {:?} has no string label", key))
        })?)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<KeyCode, D::Error> {
        let s = String::deserialize(d)?;
        parse_keycode(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown key label '{}'", s)))
    }

    /// Stable string label for a `KeyCode`. Returns `None` for keys we
    /// don't expose in the public binding surface (numpad, F13+, exotic
    /// internationalisation keys, etc.). New additions here should mirror
    /// `parse_keycode`.
    pub(crate) fn keycode_label(k: KeyCode) -> Option<&'static str> {
        Some(match k {
            // Letters
            KeyCode::KeyA => "A",
            KeyCode::KeyB => "B",
            KeyCode::KeyC => "C",
            KeyCode::KeyD => "D",
            KeyCode::KeyE => "E",
            KeyCode::KeyF => "F",
            KeyCode::KeyG => "G",
            KeyCode::KeyH => "H",
            KeyCode::KeyI => "I",
            KeyCode::KeyJ => "J",
            KeyCode::KeyK => "K",
            KeyCode::KeyL => "L",
            KeyCode::KeyM => "M",
            KeyCode::KeyN => "N",
            KeyCode::KeyO => "O",
            KeyCode::KeyP => "P",
            KeyCode::KeyQ => "Q",
            KeyCode::KeyR => "R",
            KeyCode::KeyS => "S",
            KeyCode::KeyT => "T",
            KeyCode::KeyU => "U",
            KeyCode::KeyV => "V",
            KeyCode::KeyW => "W",
            KeyCode::KeyX => "X",
            KeyCode::KeyY => "Y",
            KeyCode::KeyZ => "Z",

            // Digits
            KeyCode::Digit0 => "0",
            KeyCode::Digit1 => "1",
            KeyCode::Digit2 => "2",
            KeyCode::Digit3 => "3",
            KeyCode::Digit4 => "4",
            KeyCode::Digit5 => "5",
            KeyCode::Digit6 => "6",
            KeyCode::Digit7 => "7",
            KeyCode::Digit8 => "8",
            KeyCode::Digit9 => "9",

            // Function keys
            KeyCode::F1 => "F1",
            KeyCode::F2 => "F2",
            KeyCode::F3 => "F3",
            KeyCode::F4 => "F4",
            KeyCode::F5 => "F5",
            KeyCode::F6 => "F6",
            KeyCode::F7 => "F7",
            KeyCode::F8 => "F8",
            KeyCode::F9 => "F9",
            KeyCode::F10 => "F10",
            KeyCode::F11 => "F11",
            KeyCode::F12 => "F12",

            // Whitespace / control
            KeyCode::Space => "Space",
            KeyCode::Enter => "Enter",
            KeyCode::Tab => "Tab",
            KeyCode::Backspace => "Backspace",
            KeyCode::Escape => "Escape",
            KeyCode::Delete => "Delete",
            KeyCode::Insert => "Insert",
            KeyCode::Home => "Home",
            KeyCode::End => "End",
            KeyCode::PageUp => "PageUp",
            KeyCode::PageDown => "PageDown",

            // Arrows
            KeyCode::ArrowUp => "ArrowUp",
            KeyCode::ArrowDown => "ArrowDown",
            KeyCode::ArrowLeft => "ArrowLeft",
            KeyCode::ArrowRight => "ArrowRight",

            // Punctuation
            KeyCode::Minus => "Minus",
            KeyCode::Equal => "Equal",
            KeyCode::BracketLeft => "BracketLeft",
            KeyCode::BracketRight => "BracketRight",
            KeyCode::Backslash => "Backslash",
            KeyCode::Semicolon => "Semicolon",
            KeyCode::Quote => "Quote",
            KeyCode::Backquote => "Backquote",
            KeyCode::Comma => "Comma",
            KeyCode::Period => "Period",
            KeyCode::Slash => "Slash",

            _ => return None,
        })
    }

    /// Parse a string label produced by `keycode_label`. Accepts a small
    /// set of common aliases (e.g. `"Esc"` for Escape, lowercase letters
    /// for the corresponding letter key).
    pub(crate) fn parse_keycode(s: &str) -> Option<KeyCode> {
        Some(match s {
            // Letters — accept either case for ergonomics.
            "A" | "a" => KeyCode::KeyA,
            "B" | "b" => KeyCode::KeyB,
            "C" | "c" => KeyCode::KeyC,
            "D" | "d" => KeyCode::KeyD,
            "E" | "e" => KeyCode::KeyE,
            "F" | "f" => KeyCode::KeyF,
            "G" | "g" => KeyCode::KeyG,
            "H" | "h" => KeyCode::KeyH,
            "I" | "i" => KeyCode::KeyI,
            "J" | "j" => KeyCode::KeyJ,
            "K" | "k" => KeyCode::KeyK,
            "L" | "l" => KeyCode::KeyL,
            "M" | "m" => KeyCode::KeyM,
            "N" | "n" => KeyCode::KeyN,
            "O" | "o" => KeyCode::KeyO,
            "P" | "p" => KeyCode::KeyP,
            "Q" | "q" => KeyCode::KeyQ,
            "R" | "r" => KeyCode::KeyR,
            "S" | "s" => KeyCode::KeyS,
            "T" | "t" => KeyCode::KeyT,
            "U" | "u" => KeyCode::KeyU,
            "V" | "v" => KeyCode::KeyV,
            "W" | "w" => KeyCode::KeyW,
            "X" | "x" => KeyCode::KeyX,
            "Y" | "y" => KeyCode::KeyY,
            "Z" | "z" => KeyCode::KeyZ,

            "0" => KeyCode::Digit0,
            "1" => KeyCode::Digit1,
            "2" => KeyCode::Digit2,
            "3" => KeyCode::Digit3,
            "4" => KeyCode::Digit4,
            "5" => KeyCode::Digit5,
            "6" => KeyCode::Digit6,
            "7" => KeyCode::Digit7,
            "8" => KeyCode::Digit8,
            "9" => KeyCode::Digit9,

            "F1" => KeyCode::F1,
            "F2" => KeyCode::F2,
            "F3" => KeyCode::F3,
            "F4" => KeyCode::F4,
            "F5" => KeyCode::F5,
            "F6" => KeyCode::F6,
            "F7" => KeyCode::F7,
            "F8" => KeyCode::F8,
            "F9" => KeyCode::F9,
            "F10" => KeyCode::F10,
            "F11" => KeyCode::F11,
            "F12" => KeyCode::F12,

            "Space" | " " => KeyCode::Space,
            "Enter" | "Return" => KeyCode::Enter,
            "Tab" => KeyCode::Tab,
            "Backspace" => KeyCode::Backspace,
            "Escape" | "Esc" => KeyCode::Escape,
            "Delete" | "Del" => KeyCode::Delete,
            "Insert" | "Ins" => KeyCode::Insert,
            "Home" => KeyCode::Home,
            "End" => KeyCode::End,
            "PageUp" | "PgUp" => KeyCode::PageUp,
            "PageDown" | "PgDn" => KeyCode::PageDown,

            "ArrowUp" | "Up" => KeyCode::ArrowUp,
            "ArrowDown" | "Down" => KeyCode::ArrowDown,
            "ArrowLeft" | "Left" => KeyCode::ArrowLeft,
            "ArrowRight" | "Right" => KeyCode::ArrowRight,

            "Minus" | "-" => KeyCode::Minus,
            "Equal" | "=" => KeyCode::Equal,
            "BracketLeft" | "[" => KeyCode::BracketLeft,
            "BracketRight" | "]" => KeyCode::BracketRight,
            "Backslash" | "\\" => KeyCode::Backslash,
            "Semicolon" | ";" => KeyCode::Semicolon,
            "Quote" | "'" => KeyCode::Quote,
            "Backquote" | "`" => KeyCode::Backquote,
            "Comma" | "," => KeyCode::Comma,
            "Period" | "." => KeyCode::Period,
            "Slash" | "/" => KeyCode::Slash,

            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_input() -> ButtonInput<KeyCode> {
        ButtonInput::<KeyCode>::default()
    }

    #[test]
    fn keycombo_just_pressed_no_modifiers() {
        let combo = KeyCombo::key(KeyCode::F2);
        let mut input = fresh_input();
        assert!(!combo.just_pressed(&input));

        input.press(KeyCode::F2);
        assert!(combo.just_pressed(&input));
        assert!(combo.pressed(&input));

        // Subsequent frame: still pressed but not just-pressed.
        input.clear_just_pressed(KeyCode::F2);
        assert!(!combo.just_pressed(&input));
        assert!(combo.pressed(&input));
    }

    #[test]
    fn keycombo_modifier_required() {
        let combo = KeyCombo::key(KeyCode::F2).with_alt();
        let mut input = fresh_input();
        input.press(KeyCode::F2);
        // Alt not held → no fire.
        assert!(!combo.just_pressed(&input));

        // Bevy's `press` is a no-op (no fresh `just_pressed` event) when
        // the key is already in the pressed set, so simulate a real
        // re-press: release first, then press alongside Alt.
        input.release(KeyCode::F2);
        input.clear_just_pressed(KeyCode::F2);
        input.press(KeyCode::AltLeft);
        input.press(KeyCode::F2);
        assert!(combo.just_pressed(&input));
    }

    #[test]
    fn keycombo_modifier_must_match_exactly() {
        // Plain F2 binding must NOT fire when Alt+F2 is pressed — that's
        // a different action (the console toggle).
        let plain = KeyCombo::key(KeyCode::F2);
        let mut input = fresh_input();
        input.press(KeyCode::AltLeft);
        input.press(KeyCode::F2);
        assert!(!plain.just_pressed(&input));
        assert!(!plain.pressed(&input));
    }

    #[test]
    fn keycombo_left_or_right_modifier_accepted() {
        let combo = KeyCombo::key(KeyCode::F2).with_ctrl();
        let mut input = fresh_input();

        input.press(KeyCode::ControlRight);
        input.press(KeyCode::F2);
        assert!(combo.just_pressed(&input));
    }

    #[test]
    fn registry_register_and_lookup() {
        let mut r = KeybindingRegistry::new();
        r.register_default("test.action", KeyCombo::key(KeyCode::F5));
        assert_eq!(r.get("test.action"), Some(KeyCombo::key(KeyCode::F5)));
        assert_eq!(
            r.default_for("test.action"),
            Some(KeyCombo::key(KeyCode::F5))
        );
        assert_eq!(r.get("missing"), None);
    }

    #[test]
    fn registry_set_overrides_binding_but_keeps_default() {
        let mut r = KeybindingRegistry::new();
        r.register_default("a", KeyCombo::key(KeyCode::F1));
        r.set("a", KeyCombo::key(KeyCode::F4));

        assert_eq!(r.get("a"), Some(KeyCombo::key(KeyCode::F4)));
        assert_eq!(r.default_for("a"), Some(KeyCombo::key(KeyCode::F1)));
    }

    #[test]
    fn registry_set_unknown_action_is_ignored() {
        let mut r = KeybindingRegistry::new();
        // No panic, no insertion.
        r.set("never.registered", KeyCombo::key(KeyCode::F1));
        assert_eq!(r.get("never.registered"), None);
    }

    #[test]
    fn registry_reset_one_restores_default() {
        let mut r = KeybindingRegistry::new();
        r.register_default("a", KeyCombo::key(KeyCode::F1));
        r.set("a", KeyCombo::key(KeyCode::F4));
        r.reset_one("a");
        assert_eq!(r.get("a"), Some(KeyCombo::key(KeyCode::F1)));
    }

    #[test]
    fn registry_reset_all_restores_defaults() {
        let mut r = KeybindingRegistry::new();
        r.register_default("a", KeyCombo::key(KeyCode::F1));
        r.register_default("b", KeyCombo::key(KeyCode::F2));
        r.set("a", KeyCombo::key(KeyCode::F4));
        r.set("b", KeyCombo::key(KeyCode::F5));
        r.reset_to_defaults();
        assert_eq!(r.get("a"), Some(KeyCombo::key(KeyCode::F1)));
        assert_eq!(r.get("b"), Some(KeyCombo::key(KeyCode::F2)));
    }

    #[test]
    fn registry_detects_conflicts() {
        let mut r = KeybindingRegistry::new();
        r.register_default("a", KeyCombo::key(KeyCode::F2));
        r.register_default("b", KeyCombo::key(KeyCode::F2));
        r.register_default("c", KeyCombo::key(KeyCode::F3));

        let conflicts = r.detect_conflicts();
        assert_eq!(conflicts.len(), 1, "expected one conflict group");
        let group = conflicts.get(&KeyCombo::key(KeyCode::F2)).unwrap();
        assert_eq!(group, &vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn registry_no_conflict_when_modifiers_differ() {
        // F2 vs Alt+F2 — not a conflict.
        let mut r = KeybindingRegistry::new();
        r.register_default("a", KeyCombo::key(KeyCode::F2));
        r.register_default("b", KeyCombo::key(KeyCode::F2).with_alt());
        assert!(r.detect_conflicts().is_empty());
    }

    #[test]
    fn engine_defaults_have_known_actions() {
        let r = KeybindingRegistry::with_engine_defaults();
        assert_eq!(
            r.get(actions::TIME_TOGGLE_PAUSE),
            Some(KeyCombo::key(KeyCode::Space))
        );
        assert_eq!(
            r.get(actions::UI_TOGGLE_SITUATION_CENTER),
            Some(KeyCombo::key(KeyCode::F3))
        );
        assert_eq!(
            r.get(actions::UI_TOGGLE_CONSOLE),
            Some(KeyCombo::key(KeyCode::F2).with_alt())
        );
    }

    #[test]
    fn engine_defaults_only_known_intentional_collision() {
        // The Escape key is intentionally bound to both
        // SELECTION_CANCEL and OBSERVER_EXIT — they're mutually exclusive
        // at runtime via observer-mode run_if guards. Any *other* default
        // collision is a bug.
        let r = KeybindingRegistry::with_engine_defaults();
        let conflicts = r.detect_conflicts();
        let escape_combo = KeyCombo::key(KeyCode::Escape);
        assert!(
            conflicts.contains_key(&escape_combo),
            "expected Escape collision (selection.cancel / observer.exit)"
        );
        for (combo, actions) in &conflicts {
            assert_eq!(
                combo,
                &escape_combo,
                "unexpected default collision on {}: {:?}",
                combo.display(),
                actions
            );
        }
    }

    #[test]
    fn registry_iter_visits_all_bindings() {
        let mut r = KeybindingRegistry::new();
        r.register_default("a", KeyCombo::key(KeyCode::F1));
        r.register_default("b", KeyCombo::key(KeyCode::F2));
        let count = r.iter().count();
        assert_eq!(count, 2);
        assert_eq!(r.len(), 2);
        assert!(!r.is_empty());
    }

    #[test]
    fn registry_is_pressed_via_action_id() {
        let mut r = KeybindingRegistry::new();
        r.register_default("a", KeyCombo::key(KeyCode::F1));
        let mut input = fresh_input();
        assert!(!r.is_just_pressed("a", &input));
        input.press(KeyCode::F1);
        assert!(r.is_just_pressed("a", &input));
        assert!(r.is_pressed("a", &input));
        assert!(!r.is_just_pressed("missing", &input));
    }

    #[test]
    fn keycombo_display_includes_modifiers() {
        assert_eq!(KeyCombo::key(KeyCode::F2).display(), "F2");
        assert_eq!(KeyCombo::key(KeyCode::F2).with_alt().display(), "Alt+F2");
        assert_eq!(
            KeyCombo::key(KeyCode::KeyS)
                .with_ctrl()
                .with_shift()
                .display(),
            "Ctrl+Shift+S"
        );
    }

    #[test]
    fn plugin_inserts_registry_with_engine_defaults() {
        // Drive the plugin via a real `App` and confirm it (a) installs a
        // registry, (b) seeds engine defaults, and (c) the startup
        // override-load runs without panicking when no override file
        // exists. The startup system is best-effort; missing file is the
        // common case and must not error out.
        let mut app = App::new();
        // Point the override-loader at a guaranteed-missing path so the
        // plugin's startup system takes the `LoadOutcome::Missing` branch
        // deterministically (no env-var leakage from other tests).
        let bogus = std::env::temp_dir().join(format!(
            "macrocosmo-keybind-plugin-test-{}-{}.toml",
            std::process::id(),
            rand::random::<u64>()
        ));
        // SAFETY: serialised against other env-touching tests by the
        // `ENV_LOCK` mutex in the config sub-module's tests; this test
        // uses a unique filename per run so brief env-var visibility to
        // other tests doesn't change their outcome.
        unsafe {
            std::env::set_var(config::PATH_OVERRIDE_ENV, &bogus);
        }
        app.add_plugins(KeybindingPlugin);
        app.update();
        unsafe {
            std::env::remove_var(config::PATH_OVERRIDE_ENV);
        }

        let registry = app
            .world()
            .get_resource::<KeybindingRegistry>()
            .expect("plugin must insert registry");
        assert!(!registry.is_empty(), "engine defaults expected");
        assert_eq!(
            registry.get(actions::UI_TOGGLE_SITUATION_CENTER),
            Some(KeyCombo::key(KeyCode::F3))
        );
    }

    #[test]
    fn keycode_label_round_trip() {
        // Spot-check a few representative keys that the default keymap
        // actually uses.
        for key in [
            KeyCode::F2,
            KeyCode::F10,
            KeyCode::Space,
            KeyCode::Escape,
            KeyCode::Home,
            KeyCode::KeyW,
            KeyCode::ArrowUp,
            KeyCode::Equal,
            KeyCode::Minus,
        ] {
            let label = keycode_serde::keycode_label(key)
                .unwrap_or_else(|| panic!("no label for {:?}", key));
            let parsed = keycode_serde::parse_keycode(label)
                .unwrap_or_else(|| panic!("failed to parse '{}' back", label));
            assert_eq!(parsed, key, "round-trip mismatch for {:?}", key);
        }
    }
}
