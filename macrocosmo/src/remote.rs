//! Custom BRP (Bevy Remote Protocol) commands for the macrocosmo remote testing framework.
//!
//! All code in this module is gated behind `#[cfg(feature = "remote")]`.

#[cfg(feature = "remote")]
pub mod remote_commands {
    use bevy::prelude::*;
    use bevy::remote::{BrpError, BrpResult, error_codes};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    use crate::components::Position;
    use crate::scripting::ScriptEngine;
    use crate::time_system::GameClock;
    use crate::visualization::GalaxyView;

    // ── Method names ──────────────────────────────────────────────────

    pub const ENTITY_SCREEN_POS_METHOD: &str = "macrocosmo/entity_screen_pos";
    pub const ADVANCE_TIME_METHOD: &str = "macrocosmo/advance_time";
    pub const EVAL_LUA_METHOD: &str = "macrocosmo/eval_lua";

    // ── Param / response structs ──────────────────────────────────────

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

    #[derive(Debug, Deserialize)]
    struct AdvanceTimeParams {
        hexadies: i64,
    }

    #[derive(Debug, Serialize)]
    struct AdvanceTimeResponse {
        elapsed: i64,
    }

    #[derive(Debug, Deserialize)]
    struct EvalLuaParams {
        code: String,
    }

    #[derive(Debug, Serialize)]
    struct EvalLuaResponse {
        result: String,
    }

    // ── Helpers ───────────────────────────────────────────────────────

    fn parse_params<T: for<'de> Deserialize<'de>>(value: Option<Value>) -> Result<T, BrpError> {
        match value {
            Some(v) => serde_json::from_value(v).map_err(|e| BrpError {
                code: error_codes::INVALID_PARAMS,
                message: e.to_string(),
                data: None,
            }),
            None => Err(BrpError {
                code: error_codes::INVALID_PARAMS,
                message: String::from("Params not provided"),
                data: None,
            }),
        }
    }

    // ── 1. macrocosmo/entity_screen_pos ──────────────────────────────

    /// Look up an entity's Position, project it to screen coordinates using
    /// `GalaxyView.scale` and the camera transform, and report visibility.
    pub fn process_entity_screen_pos(
        In(params): In<Option<Value>>,
        world: &mut World,
    ) -> BrpResult {
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

        let mut camera_query =
            world.query_filtered::<(&GlobalTransform, &Camera), With<Camera2d>>();
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

    // ── 2. macrocosmo/advance_time ───────────────────────────────────

    /// Increment `GameClock.elapsed` by the specified number of hexadies.
    pub fn process_advance_time(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
        let AdvanceTimeParams { hexadies } = parse_params(params)?;

        let mut clock = world.resource_mut::<GameClock>();
        clock.elapsed += hexadies;
        let elapsed = clock.elapsed;

        let response = AdvanceTimeResponse { elapsed };
        serde_json::to_value(response).map_err(BrpError::internal)
    }

    // ── 3. macrocosmo/eval_lua ───────────────────────────────────────

    /// Evaluate arbitrary Lua code via the ScriptEngine and return the result
    /// as a string.
    pub fn process_eval_lua(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
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
}
