//! #390-T3: BRP input injection commands for automated UI testing.
//!
//! Adds three JSON-RPC methods to the Bevy Remote Protocol:
//! - `macrocosmo/click` — inject a mouse click at screen coordinates
//! - `macrocosmo/key_press` — inject a keyboard key press
//! - `macrocosmo/hover` — move the cursor without clicking
//!
//! All gated behind `#[cfg(feature = "remote")]`.

use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, error_codes};
use serde::Deserialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Pending release resource — releases injected inputs on the next frame
// ---------------------------------------------------------------------------

/// Collects inputs that were pressed by BRP handlers and need to be released
/// on the following frame so that Bevy's `just_pressed` detection works.
#[derive(Resource, Default)]
pub struct PendingInputReleases {
    pub mouse_buttons: Vec<MouseButton>,
    pub keys: Vec<KeyCode>,
}

/// System that drains [`PendingInputReleases`] and releases the stored inputs.
/// Runs every frame in `PreUpdate` (before game logic reads input).
pub fn release_pending_inputs(
    mut pending: ResMut<PendingInputReleases>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
) {
    for btn in pending.mouse_buttons.drain(..) {
        mouse.release(btn);
    }
    for key in pending.keys.drain(..) {
        keys.release(key);
    }
}

// ---------------------------------------------------------------------------
// macrocosmo/click
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ClickParams {
    x: f32,
    y: f32,
    #[serde(default = "default_button")]
    button: String,
}

fn default_button() -> String {
    "left".into()
}

fn parse_mouse_button(s: &str) -> Result<MouseButton, BrpError> {
    match s.to_lowercase().as_str() {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        other => Err(BrpError {
            code: error_codes::INVALID_PARAMS,
            message: format!("Unknown mouse button: {other:?}. Expected left, right, or middle."),
            data: None,
        }),
    }
}

/// Handler for `macrocosmo/click`.
///
/// Params: `{ "x": f32, "y": f32, "button": "left" | "right" | "middle" }`
///
/// Sets the cursor position on the primary window, then injects a mouse button
/// press. The release happens on the next frame via [`PendingInputReleases`].
pub fn handle_click(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let ClickParams { x, y, button } = parse_params(params)?;
    let btn = parse_mouse_button(&button)?;

    // Update cursor position on the primary window.
    set_cursor_position(world, x, y)?;

    // Press the mouse button (will be released next frame).
    world.resource_mut::<ButtonInput<MouseButton>>().press(btn);
    world
        .resource_mut::<PendingInputReleases>()
        .mouse_buttons
        .push(btn);

    Ok(Value::Object(serde_json::Map::from_iter([
        ("status".into(), Value::String("ok".into())),
        (
            "x".into(),
            serde_json::Number::from_f64(x as f64).map_or(Value::Null, Value::Number),
        ),
        (
            "y".into(),
            serde_json::Number::from_f64(y as f64).map_or(Value::Null, Value::Number),
        ),
        ("button".into(), Value::String(button)),
    ])))
}

// ---------------------------------------------------------------------------
// macrocosmo/key_press
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct KeyPressParams {
    key: String,
}

fn parse_key_code(s: &str) -> Result<KeyCode, BrpError> {
    // Map common key names to KeyCode variants.
    let code = match s {
        // Letters
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

        // Digits
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

        // Function keys
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

        // Special keys
        "Escape" | "Esc" => KeyCode::Escape,
        "Space" | " " => KeyCode::Space,
        "Enter" | "Return" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Insert" => KeyCode::Insert,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,

        // Arrow keys
        "ArrowUp" | "Up" => KeyCode::ArrowUp,
        "ArrowDown" | "Down" => KeyCode::ArrowDown,
        "ArrowLeft" | "Left" => KeyCode::ArrowLeft,
        "ArrowRight" | "Right" => KeyCode::ArrowRight,

        // Modifiers
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" | "CtrlLeft" => KeyCode::ControlLeft,
        "ControlRight" | "CtrlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        "SuperLeft" | "MetaLeft" | "CmdLeft" => KeyCode::SuperLeft,
        "SuperRight" | "MetaRight" | "CmdRight" => KeyCode::SuperRight,

        // Punctuation / symbols
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

        other => {
            return Err(BrpError {
                code: error_codes::INVALID_PARAMS,
                message: format!("Unknown key: {other:?}"),
                data: None,
            });
        }
    };
    Ok(code)
}

/// Handler for `macrocosmo/key_press`.
///
/// Params: `{ "key": "F3" | "Escape" | "Space" | ... }`
///
/// Injects a key press. The release happens on the next frame via
/// [`PendingInputReleases`].
pub fn handle_key_press(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let KeyPressParams { key } = parse_params(params)?;
    let code = parse_key_code(&key)?;

    world.resource_mut::<ButtonInput<KeyCode>>().press(code);
    world.resource_mut::<PendingInputReleases>().keys.push(code);

    Ok(Value::Object(serde_json::Map::from_iter([(
        "status".into(),
        Value::String("ok".into()),
    )])))
}

// ---------------------------------------------------------------------------
// macrocosmo/hover
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct HoverParams {
    x: f32,
    y: f32,
}

/// Handler for `macrocosmo/hover`.
///
/// Params: `{ "x": f32, "y": f32 }`
///
/// Moves the cursor position on the primary window without clicking.
pub fn handle_hover(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let HoverParams { x, y } = parse_params(params)?;
    set_cursor_position(world, x, y)?;

    Ok(Value::Object(serde_json::Map::from_iter([
        ("status".into(), Value::String("ok".into())),
        (
            "x".into(),
            serde_json::Number::from_f64(x as f64).map_or(Value::Null, Value::Number),
        ),
        (
            "y".into(),
            serde_json::Number::from_f64(y as f64).map_or(Value::Null, Value::Number),
        ),
    ])))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse JSON-RPC params into a typed struct.
fn parse_params<T: for<'de> Deserialize<'de>>(params: Option<Value>) -> Result<T, BrpError> {
    match params {
        Some(value) => serde_json::from_value(value).map_err(|err| BrpError {
            code: error_codes::INVALID_PARAMS,
            message: err.to_string(),
            data: None,
        }),
        None => Err(BrpError {
            code: error_codes::INVALID_PARAMS,
            message: "Params not provided".into(),
            data: None,
        }),
    }
}

/// Set the cursor position on the primary window. Coordinates are in logical
/// pixels (matching `Window::cursor_position()`).
fn set_cursor_position(world: &mut World, x: f32, y: f32) -> Result<(), BrpError> {
    let mut windows = world.query::<&mut Window>();
    let mut window = windows.single_mut(world).map_err(|_| BrpError {
        code: error_codes::INTERNAL_ERROR,
        message: "No primary window found".into(),
        data: None,
    })?;
    window.set_cursor_position(Some(Vec2::new(x, y)));
    Ok(())
}

// ---------------------------------------------------------------------------
// Plugin integration — called from main.rs
// ---------------------------------------------------------------------------

/// Extends the [`RemotePlugin`] with macrocosmo-specific input injection
/// methods. Call this in `main.rs` instead of `RemotePlugin::default()`.
pub fn remote_plugin() -> bevy::remote::RemotePlugin {
    bevy::remote::RemotePlugin::default()
        .with_method("macrocosmo/click", handle_click)
        .with_method("macrocosmo/key_press", handle_key_press)
        .with_method("macrocosmo/hover", handle_hover)
}
