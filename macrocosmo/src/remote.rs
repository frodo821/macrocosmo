//! Custom BRP (Bevy Remote Protocol) commands for the macrocosmo remote testing framework.
//!
//! Provides the following JSON-RPC methods:
//! - `macrocosmo/entity_screen_pos` — project an entity's world position to screen coordinates
//! - `macrocosmo/advance_time` — increment the game clock
//! - `macrocosmo/eval_lua` — evaluate arbitrary Lua code
//! - `macrocosmo/click` — inject a mouse click at screen coordinates
//! - `macrocosmo/key_press` — inject a keyboard key press
//! - `macrocosmo/hover` — move the cursor without clicking
//!
//! All gated behind `#[cfg(feature = "remote")]`.

use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, error_codes};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::components::Position;
use crate::scripting::ScriptEngine;
use crate::time_system::GameClock;
use crate::visualization::GalaxyView;

// ═══════════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════════

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

// ═══════════════════════════════════════════════════════════════════════════
// 1. macrocosmo/entity_screen_pos
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct EntityScreenPosParams {
    entity: u64,
}

#[derive(Debug, Serialize)]
struct EntityScreenPosResponse {
    x: f32,
    y: f32,
    visible: bool,
}

/// Look up an entity's Position, project it to screen coordinates using
/// `GalaxyView.scale` and the camera transform, and report visibility.
pub fn entity_screen_pos_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let EntityScreenPosParams { entity: bits } = parse_params(params)?;
    let entity = Entity::try_from_bits(bits).ok_or_else(|| BrpError {
        code: error_codes::ENTITY_NOT_FOUND,
        message: format!("Invalid entity bits: {bits}"),
        data: None,
    })?;

    let position = *world
        .get::<Position>(entity)
        .ok_or_else(|| BrpError::entity_not_found(entity))?;

    let view_scale = world
        .get_resource::<GalaxyView>()
        .ok_or_else(|| BrpError::internal("GalaxyView resource not found"))?
        .scale;

    // World position in screen-space (before camera offset)
    let world_x = position.x as f32 * view_scale;
    let world_y = position.y as f32 * view_scale;

    // Find the 2D camera transform and viewport size
    let mut cam_translation = Vec3::ZERO;
    let mut half_width = 640.0_f32;
    let mut half_height = 360.0_f32;
    let mut found_camera = false;

    let mut camera_query = world.query_filtered::<(&GlobalTransform, &Camera), With<Camera2d>>();
    for (gt, camera) in camera_query.iter(world) {
        cam_translation = gt.translation();
        found_camera = true;
        if let Some(viewport_size) = camera.physical_viewport_size() {
            half_width = viewport_size.x as f32 / 2.0;
            half_height = viewport_size.y as f32 / 2.0;
        }
        break;
    }

    if !found_camera {
        return Err(BrpError::internal("No Camera2d entity found"));
    }

    // Screen position: world pos minus camera translation, then shift to
    // screen coords where (0,0) is top-left.
    let screen_x = (world_x - cam_translation.x) + half_width;
    let screen_y = half_height - (world_y - cam_translation.y); // flip Y

    let visible = screen_x >= 0.0
        && screen_x <= half_width * 2.0
        && screen_y >= 0.0
        && screen_y <= half_height * 2.0;

    let response = EntityScreenPosResponse {
        x: screen_x,
        y: screen_y,
        visible,
    };
    serde_json::to_value(response).map_err(BrpError::internal)
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. macrocosmo/advance_time
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct AdvanceTimeParams {
    hexadies: i64,
}

#[derive(Debug, Serialize)]
struct AdvanceTimeResponse {
    elapsed: i64,
}

/// Increment `GameClock.elapsed` by the specified number of hexadies.
pub fn advance_time_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let AdvanceTimeParams { hexadies } = parse_params(params)?;

    let mut clock = world.resource_mut::<GameClock>();
    clock.elapsed += hexadies;
    let elapsed = clock.elapsed;

    let response = AdvanceTimeResponse { elapsed };
    serde_json::to_value(response).map_err(BrpError::internal)
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. macrocosmo/eval_lua
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct EvalLuaParams {
    code: String,
}

#[derive(Debug, Serialize)]
struct EvalLuaResponse {
    result: String,
}

/// Evaluate arbitrary Lua code via the ScriptEngine and return the result
/// as a string.
pub fn eval_lua_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let EvalLuaParams { code } = parse_params(params)?;

    let engine = world
        .get_resource::<ScriptEngine>()
        .ok_or_else(|| BrpError::internal("ScriptEngine resource not found"))?;

    let lua = engine.lua();
    let result = lua
        .load(&code)
        .eval::<mlua::Value>()
        .map_err(|e| BrpError::internal(format!("Lua error: {e}")))?;

    let result_str = lua_value_to_string(&result);

    let response = EvalLuaResponse { result: result_str };
    serde_json::to_value(response).map_err(BrpError::internal)
}

/// Convert an mlua::Value to a human-readable string representation.
fn lua_value_to_string(value: &mlua::Value) -> String {
    match value {
        mlua::Value::Nil => "nil".to_string(),
        mlua::Value::Boolean(b) => b.to_string(),
        mlua::Value::Integer(i) => i.to_string(),
        mlua::Value::Number(n) => n.to_string(),
        mlua::Value::String(s) => match s.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => "<invalid utf8>".to_string(),
        },
        mlua::Value::Table(t) => {
            let mut parts = Vec::new();
            if let Ok(pairs) = t
                .clone()
                .pairs::<mlua::Value, mlua::Value>()
                .collect::<Result<Vec<_>, _>>()
            {
                for (k, v) in pairs {
                    parts.push(format!(
                        "[{}] = {}",
                        lua_value_to_string(&k),
                        lua_value_to_string(&v)
                    ));
                }
            }
            format!("{{ {} }}", parts.join(", "))
        }
        mlua::Value::Function(_) => "<function>".to_string(),
        mlua::Value::UserData(_) => "<userdata>".to_string(),
        mlua::Value::LightUserData(_) => "<lightuserdata>".to_string(),
        mlua::Value::Thread(_) => "<thread>".to_string(),
        mlua::Value::Error(e) => format!("<error: {e}>"),
        _ => "<unknown>".to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. macrocosmo/click
// ═══════════════════════════════════════════════════════════════════════════

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
pub fn click_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
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

// ═══════════════════════════════════════════════════════════════════════════
// 5. macrocosmo/key_press
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct KeyPressParams {
    key: String,
}

fn parse_key_code(s: &str) -> Result<KeyCode, BrpError> {
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
pub fn key_press_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let KeyPressParams { key } = parse_params(params)?;
    let code = parse_key_code(&key)?;

    world.resource_mut::<ButtonInput<KeyCode>>().press(code);
    world.resource_mut::<PendingInputReleases>().keys.push(code);

    Ok(Value::Object(serde_json::Map::from_iter([(
        "status".into(),
        Value::String("ok".into()),
    )])))
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. macrocosmo/hover
// ═══════════════════════════════════════════════════════════════════════════

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
pub fn hover_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
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

// ═══════════════════════════════════════════════════════════════════════════
// 7. macrocosmo/screenshot
// ═══════════════════════════════════════════════════════════════════════════

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use serde_json::json;
use std::io::Cursor;

/// Holds the latest captured screenshot as a base64-encoded PNG string plus
/// dimensions. Written by the entity observer, consumed by the BRP handler.
#[derive(Resource, Default)]
pub struct ScreenshotBuffer {
    pub data: Option<ScreenshotData>,
}

/// Payload returned by the `macrocosmo/screenshot` method.
pub struct ScreenshotData {
    pub base64: String,
    pub width: u32,
    pub height: u32,
}

/// BRP handler for `macrocosmo/screenshot`.
///
/// Returns `{ "base64": "...", "width": u32, "height": u32 }` when a screenshot
/// is available. On the first call (nothing buffered) it requests a capture and
/// returns an error telling the client to retry after one frame.
pub fn screenshot_handler(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    // Try to consume a previously-captured screenshot.
    let has_data = world
        .get_resource::<ScreenshotBuffer>()
        .and_then(|buf| buf.data.as_ref())
        .is_some();

    if has_data {
        let data = world
            .resource_mut::<ScreenshotBuffer>()
            .data
            .take()
            .unwrap();
        return Ok(json!({
            "base64": data.base64,
            "width": data.width,
            "height": data.height,
        }));
    }

    // No screenshot buffered yet — spawn a capture request.
    // The entity observer will encode the result into ScreenshotBuffer.
    world
        .spawn(Screenshot::primary_window())
        .observe(on_screenshot_captured);

    Err(BrpError {
        code: error_codes::INTERNAL_ERROR,
        message: "Screenshot requested — retry after one frame".into(),
        data: None,
    })
}

/// Entity observer callback: encodes the captured image as PNG -> base64 and
/// stores it in [`ScreenshotBuffer`].
fn on_screenshot_captured(trigger: On<ScreenshotCaptured>, mut buffer: ResMut<ScreenshotBuffer>) {
    let captured = &*trigger;
    let image = &captured.image;

    let width = image.width();
    let height = image.height();

    // Convert Bevy Image -> DynamicImage -> RGB8 -> PNG bytes -> base64.
    let dyn_img = match image.clone().try_into_dynamic() {
        Ok(img) => img,
        Err(e) => {
            error!("Screenshot: failed to convert to DynamicImage: {e:?}");
            return;
        }
    };

    let rgb = dyn_img.to_rgb8();
    let mut png_bytes = Cursor::new(Vec::new());
    if let Err(e) = rgb.write_to(&mut png_bytes, image::ImageFormat::Png) {
        error!("Screenshot: failed to encode PNG: {e}");
        return;
    }

    let encoded = BASE64.encode(png_bytes.into_inner());

    buffer.data = Some(ScreenshotData {
        base64: encoded,
        width,
        height,
    });

    info!("Screenshot captured and buffered: {width}x{height}");
}

// ═══════════════════════════════════════════════════════════════════════════
// Plugin integration — called from main.rs
// ═══════════════════════════════════════════════════════════════════════════

/// Builds the [`RemotePlugin`] with all macrocosmo BRP methods registered.
pub fn remote_plugin() -> bevy::remote::RemotePlugin {
    bevy::remote::RemotePlugin::default()
        .with_method("macrocosmo/entity_screen_pos", entity_screen_pos_handler)
        .with_method("macrocosmo/advance_time", advance_time_handler)
        .with_method("macrocosmo/eval_lua", eval_lua_handler)
        .with_method("macrocosmo/click", click_handler)
        .with_method("macrocosmo/key_press", key_press_handler)
        .with_method("macrocosmo/hover", hover_handler)
        .with_method("macrocosmo/screenshot", screenshot_handler)
}
