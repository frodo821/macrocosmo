use std::collections::HashMap;

use bevy::prelude::*;

use crate::faction::{FactionRelations, RelationState};

/// Category that a faction belongs to (e.g. `empire`, `space_creature`,
/// `ancient_defense`). Defined from Lua via `define_faction_type`.
///
/// A `FactionDefinition` may reference one of these via its `faction_type`
/// field; the type supplies defaults for new diplomatic relations and
/// gates the diplomacy UI (`can_diplomacy`).
#[derive(Debug, Clone)]
pub struct FactionTypeDefinition {
    pub id: String,
    /// Whether this faction type can engage in formal diplomacy
    /// (treaties, declarations, etc.). Used by the diplomacy UI to
    /// show/hide controls.
    pub can_diplomacy: bool,
    /// Default standing (-100..=100) for new relationships.
    pub default_standing: f64,
    /// Default RelationState for new relationships.
    pub default_state: RelationState,
    /// Combat strength for hostile faction entities of this type
    /// (used at galaxy generation to scale the `HostileStats.strength`
    /// component, and by combat strength calculations). Default 0.0.
    pub strength: f64,
    /// Evasion stat for hostile faction entities (0..=100).
    pub evasion: f64,
    /// Default current HP for a newly spawned hostile entity of this type.
    pub default_hp: f64,
    /// Default max HP for a newly spawned hostile entity of this type.
    pub default_max_hp: f64,
}

/// Registry of all faction-type definitions loaded from Lua.
#[derive(Resource, Default, Debug)]
pub struct FactionTypeRegistry {
    pub types: HashMap<String, FactionTypeDefinition>,
}

impl FactionTypeRegistry {
    /// Look up a faction type by id. Returns `None` if not registered.
    pub fn get(&self, id: &str) -> Option<&FactionTypeDefinition> {
        self.types.get(id)
    }
}

/// A faction definition loaded from Lua scripts.
#[derive(Debug, Clone)]
pub struct FactionDefinition {
    pub id: String,
    pub name: String,
    /// Optional faction-type id (e.g. `"empire"`, `"space_creature"`).
    /// Resolved against `FactionTypeRegistry` at runtime — not validated
    /// at parse time so that types and factions can be defined in any order.
    pub faction_type: Option<String>,
    /// Whether this faction defines an `on_game_start` callback.
    /// The actual function is looked up from `_faction_definitions` at call time.
    pub has_on_game_start: bool,
}

/// Registry of all faction definitions loaded from Lua.
#[derive(Resource, Default, Debug)]
pub struct FactionRegistry {
    pub factions: HashMap<String, FactionDefinition>,
}

/// Parse faction definitions from the Lua `_faction_definitions` global table.
pub fn parse_faction_definitions(lua: &mlua::Lua) -> Result<Vec<FactionDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_faction_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let has_on_game_start = matches!(
            table
                .get::<mlua::Value>("on_game_start")
                .unwrap_or(mlua::Value::Nil),
            mlua::Value::Function(_)
        );

        // Optional `faction_type` field. Accept either a string id
        // ("empire") or a reference table returned by `define_faction_type`.
        // Backwards-compatible alias `type` is also accepted; if both are
        // present `faction_type` wins.
        let raw_type = table
            .get::<mlua::Value>("faction_type")
            .unwrap_or(mlua::Value::Nil);
        let raw_type = match raw_type {
            mlua::Value::Nil => table.get::<mlua::Value>("type").unwrap_or(mlua::Value::Nil),
            v => v,
        };
        let faction_type = match raw_type {
            mlua::Value::Nil => None,
            v => Some(crate::scripting::extract_ref_id(&v)?),
        };

        result.push(FactionDefinition {
            id,
            name,
            faction_type,
            has_on_game_start,
        });
    }

    Ok(result)
}

/// Parse faction-type definitions from the Lua `_faction_type_definitions`
/// global table. Unknown `default_state` strings produce a `RuntimeError`.
pub fn parse_faction_type_definitions(
    lua: &mlua::Lua,
) -> Result<Vec<FactionTypeDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_faction_type_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let can_diplomacy: bool = table.get::<Option<bool>>("can_diplomacy")?.unwrap_or(false);
        let default_standing: f64 = table.get::<Option<f64>>("default_standing")?.unwrap_or(0.0);
        let default_state_str: String = table
            .get::<Option<String>>("default_state")?
            .unwrap_or_else(|| "neutral".to_string());
        let default_state = RelationState::from_str(&default_state_str)?;

        let strength: f64 = table.get::<Option<f64>>("strength")?.unwrap_or(0.0);
        let evasion: f64 = table.get::<Option<f64>>("evasion")?.unwrap_or(0.0);
        let default_hp: f64 = table.get::<Option<f64>>("default_hp")?.unwrap_or(0.0);
        let default_max_hp: f64 = table
            .get::<Option<f64>>("default_max_hp")?
            .unwrap_or(default_hp);

        result.push(FactionTypeDefinition {
            id,
            can_diplomacy,
            default_standing,
            default_state,
            strength,
            evasion,
            default_hp,
            default_max_hp,
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// #172: define_diplomatic_action — Lua-defined custom diplomatic actions
// ---------------------------------------------------------------------------

/// Lua-defined custom diplomatic action (e.g. "trade_agreement") that
/// coexists with the built-in [`crate::faction::DiplomaticAction`] variants
/// (`DeclareWar`, `ProposePeace`, etc.).
///
/// Prerequisite checks (`requires_diplomacy`, `requires_state`,
/// `min_standing`) gate whether the sending faction may propose the action
/// against a given target. When delivered and accepted, the optional
/// `on_accepted` Lua callback runs with an `EffectScope` whose returned
/// [`crate::effect::DescriptiveEffect`] list is applied as normal tech-style
/// effects (flag sets, global param modifiers, etc.).
#[derive(Debug, Clone)]
pub struct DiplomaticActionDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    /// If `true`, the target faction's `FactionType.can_diplomacy` must be
    /// `true` for the action to be available.
    pub requires_diplomacy: bool,
    /// If set, the *sender's* current view of the target must be in this
    /// state for the action to be available.
    pub requires_state: Option<RelationState>,
    /// If set, the sender's standing toward the target must be `>=` this
    /// value for the action to be available.
    pub min_standing: Option<f64>,
    /// Whether the definition declared an `on_accepted` Lua callback. The
    /// actual function is looked up lazily at call time via
    /// [`lookup_on_accepted`] so we don't need to retain a long-lived
    /// reference to the `Lua` context in the registry.
    pub has_on_accepted: bool,
}

impl DiplomaticActionDefinition {
    /// Evaluate prerequisite checks against the current game state.
    ///
    /// Returns `true` iff:
    /// - when `requires_diplomacy`: the target faction's type is marked
    ///   `can_diplomacy`;
    /// - when `requires_state` is set: the sender's view of the target
    ///   matches that state (an absent relation counts as `Neutral`);
    /// - when `min_standing` is set: the sender's standing toward the
    ///   target is `>=` the threshold.
    pub fn is_available(
        &self,
        from_faction_entity: Entity,
        to_faction_entity: Entity,
        factions: &Query<&crate::player::Faction>,
        relations: &FactionRelations,
        faction_registry: &FactionRegistry,
        type_registry: &FactionTypeRegistry,
    ) -> bool {
        if self.requires_diplomacy
            && !crate::faction::faction_can_diplomacy(
                to_faction_entity,
                factions,
                faction_registry,
                type_registry,
            )
        {
            return false;
        }

        let view = relations.get_or_default(from_faction_entity, to_faction_entity);

        if let Some(state) = self.requires_state
            && view.state != state
        {
            return false;
        }

        if let Some(min) = self.min_standing
            && view.standing < min
        {
            return false;
        }

        true
    }
}

/// Registry of all diplomatic-action definitions loaded from Lua.
#[derive(Resource, Default, Debug)]
pub struct DiplomaticActionRegistry {
    pub actions: HashMap<String, DiplomaticActionDefinition>,
}

impl DiplomaticActionRegistry {
    /// Look up an action by id. Returns `None` if not registered.
    pub fn get(&self, id: &str) -> Option<&DiplomaticActionDefinition> {
        self.actions.get(id)
    }
}

/// Parse diplomatic-action definitions from the Lua
/// `_diplomatic_action_definitions` global table.
pub fn parse_diplomatic_action_definitions(
    lua: &mlua::Lua,
) -> Result<Vec<DiplomaticActionDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_diplomatic_action_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table
            .get::<Option<String>>("name")?
            .unwrap_or_else(|| id.clone());
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();
        let requires_diplomacy: bool = table
            .get::<Option<bool>>("requires_diplomacy")?
            .unwrap_or(false);

        let requires_state = match table.get::<Option<String>>("requires_state")? {
            Some(s) => Some(RelationState::from_str(&s)?),
            None => None,
        };

        let min_standing = table.get::<Option<f64>>("min_standing")?;

        let has_on_accepted = matches!(
            table
                .get::<mlua::Value>("on_accepted")
                .unwrap_or(mlua::Value::Nil),
            mlua::Value::Function(_)
        );

        result.push(DiplomaticActionDefinition {
            id,
            name,
            description,
            requires_diplomacy,
            requires_state,
            min_standing,
            has_on_accepted,
        });
    }

    Ok(result)
}

/// Look up the `on_accepted` Lua function for the given diplomatic-action id,
/// if any. Returns Ok(None) if the action is not defined or has no callback.
pub fn lookup_on_accepted(
    lua: &mlua::Lua,
    action_id: &str,
) -> Result<Option<mlua::Function>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_diplomatic_action_definitions")?;
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id: String = table.get("id")?;
        if id == action_id {
            let value: mlua::Value = table.get("on_accepted")?;
            if let mlua::Value::Function(f) = value {
                return Ok(Some(f));
            }
            return Ok(None);
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// #302: define_diplomatic_option — Lua-defined diplomatic option framework
// ---------------------------------------------------------------------------

/// A possible response to a diplomatic option (POD — no closures).
///
/// When the receiver picks this response, the `event_id` string is fired
/// through the event system so Lua `on()` handlers can react.
#[derive(Debug, Clone)]
pub struct DiplomaticOptionResponse {
    /// Unique response id within the option (e.g. "accept", "reject").
    pub id: String,
    /// Human-readable label shown in the UI.
    pub label: String,
    /// Event id to fire when this response is chosen.
    pub event_id: String,
}

/// A diplomatic option definition loaded from Lua.
///
/// Unlike [`DiplomaticActionDefinition`], options model a richer interaction:
/// they carry a `kind` (bilateral/unilateral), a list of POD
/// [`DiplomaticOptionResponse`] entries, and a `payload_schema` hint that
/// describes the `HashMap<String,String>` fields carried by the in-flight
/// [`crate::faction::DiplomaticEvent`].
#[derive(Debug, Clone)]
pub struct DiplomaticOptionDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    /// `"bilateral"` (requires receiver response) or `"unilateral"` (fire and
    /// forget).
    pub kind: String,
    /// Ordered list of responses available to the receiver (empty for
    /// unilateral options).
    pub responses: Vec<DiplomaticOptionResponse>,
    /// Optional list of expected payload keys for documentation /
    /// validation purposes. Not enforced at runtime.
    pub payload_schema: Vec<String>,
}

/// Registry of all diplomatic-option definitions loaded from Lua.
#[derive(Resource, Default, Debug)]
pub struct DiplomaticOptionRegistry {
    pub options: HashMap<String, DiplomaticOptionDefinition>,
}

impl DiplomaticOptionRegistry {
    /// Look up an option by id.
    pub fn get(&self, id: &str) -> Option<&DiplomaticOptionDefinition> {
        self.options.get(id)
    }
}

/// Parse diplomatic-option definitions from the Lua
/// `_diplomatic_option_definitions` global table.
pub fn parse_diplomatic_option_definitions(
    lua: &mlua::Lua,
) -> Result<Vec<DiplomaticOptionDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_diplomatic_option_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table
            .get::<Option<String>>("name")?
            .unwrap_or_else(|| id.clone());
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();
        let kind: String = table
            .get::<Option<String>>("kind")?
            .unwrap_or_else(|| "bilateral".to_string());

        // Validate kind
        if kind != "bilateral" && kind != "unilateral" {
            return Err(mlua::Error::RuntimeError(format!(
                "define_diplomatic_option '{}': kind must be 'bilateral' or 'unilateral', got '{}'",
                id, kind
            )));
        }

        // Parse responses array
        let mut responses = Vec::new();
        if let Some(resp_table) = table.get::<Option<mlua::Table>>("responses")? {
            for resp_pair in resp_table.pairs::<i64, mlua::Table>() {
                let (_, resp) = resp_pair?;
                let resp_id: String = resp.get("id")?;
                let resp_label: String = resp
                    .get::<Option<String>>("label")?
                    .unwrap_or_else(|| resp_id.clone());
                let resp_event_id: String = resp.get("event_id")?;
                responses.push(DiplomaticOptionResponse {
                    id: resp_id,
                    label: resp_label,
                    event_id: resp_event_id,
                });
            }
        }

        // Parse payload_schema array
        let mut payload_schema = Vec::new();
        if let Some(schema_table) = table.get::<Option<mlua::Table>>("payload_schema")? {
            for schema_pair in schema_table.pairs::<i64, String>() {
                let (_, key) = schema_pair?;
                payload_schema.push(key);
            }
        }

        result.push(DiplomaticOptionDefinition {
            id,
            name,
            description,
            kind,
            responses,
            payload_schema,
        });
    }

    Ok(result)
}

/// Look up the `on_game_start` Lua function for the given faction id, if any.
/// Returns Ok(None) if the faction is not defined or has no callback.
pub fn lookup_on_game_start(
    lua: &mlua::Lua,
    faction_id: &str,
) -> Result<Option<mlua::Function>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_faction_definitions")?;
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id: String = table.get("id")?;
        if id == faction_id {
            let value: mlua::Value = table.get("on_game_start")?;
            if let mlua::Value::Function(f) = value {
                return Ok(Some(f));
            }
            return Ok(None);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_faction_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction {
                id = "humanity_empire",
                name = "Terran Federation",
            }
            define_faction {
                id = "alien_hive",
                name = "Zyx Collective",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].id, "humanity_empire");
        assert_eq!(defs[0].name, "Terran Federation");
        assert!(!defs[0].has_on_game_start);
        assert_eq!(defs[1].id, "alien_hive");
        assert_eq!(defs[1].name, "Zyx Collective");
        assert!(!defs[1].has_on_game_start);
    }

    #[test]
    fn test_parse_faction_with_on_game_start() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction {
                id = "humanity_empire",
                name = "Terran Federation",
                on_game_start = function(ctx) end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert!(defs[0].has_on_game_start);
    }

    #[test]
    fn test_lookup_on_game_start_returns_function() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction {
                id = "humanity_empire",
                name = "Terran Federation",
                on_game_start = function(ctx) return 42 end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let func = lookup_on_game_start(lua, "humanity_empire").unwrap();
        assert!(func.is_some());
        let result: i64 = func.unwrap().call(()).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_lookup_on_game_start_missing() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"define_faction { id = "humanity_empire", name = "Terran Federation" }"#)
            .exec()
            .unwrap();

        let func = lookup_on_game_start(lua, "humanity_empire").unwrap();
        assert!(func.is_none());

        let func2 = lookup_on_game_start(lua, "nonexistent").unwrap();
        assert!(func2.is_none());
    }

    #[test]
    fn test_define_faction_returns_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: mlua::Table = lua
            .load(r#"return define_faction { id = "test_faction", name = "Test" }"#)
            .eval()
            .unwrap();

        let def_type: String = result.get("_def_type").unwrap();
        assert_eq!(def_type, "faction");
        let id: String = result.get("id").unwrap();
        assert_eq!(id, "test_faction");
    }

    #[test]
    fn test_parse_faction_empty() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 0);
    }

    // --- #170: define_faction_type ---

    #[test]
    fn test_relation_state_from_str() {
        assert_eq!(
            RelationState::from_str("neutral").unwrap(),
            RelationState::Neutral
        );
        assert_eq!(
            RelationState::from_str("Peace").unwrap(),
            RelationState::Peace
        );
        assert_eq!(RelationState::from_str("WAR").unwrap(), RelationState::War);
        assert_eq!(
            RelationState::from_str("alliance").unwrap(),
            RelationState::Alliance
        );
        assert!(RelationState::from_str("bogus").is_err());
    }

    #[test]
    fn test_define_faction_type_returns_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: mlua::Table = lua
            .load(
                r#"return define_faction_type {
                    id = "empire",
                    can_diplomacy = true,
                    default_standing = 0,
                    default_state = "neutral",
                }"#,
            )
            .eval()
            .unwrap();

        let def_type: String = result.get("_def_type").unwrap();
        assert_eq!(def_type, "faction_type");
        let id: String = result.get("id").unwrap();
        assert_eq!(id, "empire");
    }

    #[test]
    fn test_parse_faction_type_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction_type {
                id = "empire",
                can_diplomacy = true,
                default_standing = 0,
                default_state = "neutral",
            }
            define_faction_type {
                id = "space_creature",
                can_diplomacy = false,
                default_standing = -100,
                default_state = "neutral",
            }
            define_faction_type {
                id = "ancient_defense",
                can_diplomacy = false,
                default_standing = -100,
                default_state = "war",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_faction_type_definitions(lua).unwrap();
        assert_eq!(defs.len(), 3);

        let empire = defs.iter().find(|d| d.id == "empire").unwrap();
        assert!(empire.can_diplomacy);
        assert!((empire.default_standing - 0.0).abs() < 1e-9);
        assert_eq!(empire.default_state, RelationState::Neutral);

        let creature = defs.iter().find(|d| d.id == "space_creature").unwrap();
        assert!(!creature.can_diplomacy);
        assert!((creature.default_standing - (-100.0)).abs() < 1e-9);
        assert_eq!(creature.default_state, RelationState::Neutral);

        let ancient = defs.iter().find(|d| d.id == "ancient_defense").unwrap();
        assert!(!ancient.can_diplomacy);
        assert_eq!(ancient.default_state, RelationState::War);
    }

    #[test]
    fn test_parse_faction_type_defaults() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // Only `id` is required; everything else has sensible defaults.
        lua.load(r#"define_faction_type { id = "minimal" }"#)
            .exec()
            .unwrap();

        let defs = parse_faction_type_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "minimal");
        assert!(!defs[0].can_diplomacy);
        assert!((defs[0].default_standing - 0.0).abs() < 1e-9);
        assert_eq!(defs[0].default_state, RelationState::Neutral);
    }

    #[test]
    fn test_parse_faction_type_unknown_state_errors() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"define_faction_type { id = "bad", default_state = "frenemy" }"#)
            .exec()
            .unwrap();

        let res = parse_faction_type_definitions(lua);
        assert!(res.is_err(), "unknown default_state must produce an error");
    }

    #[test]
    fn test_faction_type_registry_lookup() {
        let mut registry = FactionTypeRegistry::default();
        registry.types.insert(
            "empire".to_string(),
            FactionTypeDefinition {
                id: "empire".to_string(),
                can_diplomacy: true,
                default_standing: 0.0,
                default_state: RelationState::Neutral,
                strength: 0.0,
                evasion: 0.0,
                default_hp: 0.0,
                default_max_hp: 0.0,
            },
        );

        assert!(registry.get("empire").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_parse_faction_type_with_hostile_stats() {
        // #293: strength/evasion/default_hp/default_max_hp drive hostile
        // entity spawn at galaxy generation time, replacing hard-coded
        // per-hostile-type constants.
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction_type {
                id = "space_creature",
                strength = 10,
                evasion = 20,
                default_hp = 80,
                default_max_hp = 80,
            }
            define_faction_type {
                id = "ancient_defense",
                strength = 10,
                evasion = 10,
                default_hp = 200,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_faction_type_definitions(lua).unwrap();
        let creature = defs.iter().find(|d| d.id == "space_creature").unwrap();
        assert!((creature.strength - 10.0).abs() < 1e-9);
        assert!((creature.evasion - 20.0).abs() < 1e-9);
        assert!((creature.default_hp - 80.0).abs() < 1e-9);
        assert!((creature.default_max_hp - 80.0).abs() < 1e-9);

        let ancient = defs.iter().find(|d| d.id == "ancient_defense").unwrap();
        assert!((ancient.strength - 10.0).abs() < 1e-9);
        // default_max_hp falls back to default_hp when absent
        assert!((ancient.default_hp - 200.0).abs() < 1e-9);
        assert!((ancient.default_max_hp - 200.0).abs() < 1e-9);
    }

    #[test]
    fn test_parse_faction_type_empty() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let defs = parse_faction_type_definitions(lua).unwrap();
        assert_eq!(defs.len(), 0);
    }

    #[test]
    fn test_define_faction_with_type_string() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction {
                id = "humanity_empire",
                name = "Terran Federation",
                faction_type = "empire",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].faction_type.as_deref(), Some("empire"));
    }

    #[test]
    fn test_define_faction_with_type_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // Reference form: pass the table returned by define_faction_type.
        lua.load(
            r#"
            local empire = define_faction_type {
                id = "empire",
                can_diplomacy = true,
                default_standing = 0,
                default_state = "neutral",
            }
            define_faction {
                id = "humanity_empire",
                name = "Terran Federation",
                faction_type = empire,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].faction_type.as_deref(), Some("empire"));
    }

    #[test]
    fn test_define_faction_without_type() {
        // Backwards-compatible: pre-#170 factions omit faction_type.
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"define_faction { id = "f", name = "F" }"#)
            .exec()
            .unwrap();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert!(defs[0].faction_type.is_none());
    }

    // --- #302: define_diplomatic_option ---

    #[test]
    fn test_parse_diplomatic_option_bilateral() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_diplomatic_option {
                id = "generic_negotiation",
                name = "Negotiate",
                description = "Open a bilateral negotiation.",
                kind = "bilateral",
                responses = {
                    { id = "accept", label = "Accept", event_id = "negotiation_accepted" },
                    { id = "reject", label = "Reject", event_id = "negotiation_rejected" },
                },
                payload_schema = { "terms", "duration" },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_diplomatic_option_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        let opt = &defs[0];
        assert_eq!(opt.id, "generic_negotiation");
        assert_eq!(opt.name, "Negotiate");
        assert_eq!(opt.kind, "bilateral");
        assert_eq!(opt.responses.len(), 2);
        assert_eq!(opt.responses[0].id, "accept");
        assert_eq!(opt.responses[0].event_id, "negotiation_accepted");
        assert_eq!(opt.responses[1].id, "reject");
        assert_eq!(opt.payload_schema, vec!["terms", "duration"]);
    }

    #[test]
    fn test_parse_diplomatic_option_unilateral() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_diplomatic_option {
                id = "break_alliance",
                name = "Break Alliance",
                kind = "unilateral",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_diplomatic_option_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, "unilateral");
        assert!(defs[0].responses.is_empty());
        assert!(defs[0].payload_schema.is_empty());
    }

    #[test]
    fn test_parse_diplomatic_option_invalid_kind() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_diplomatic_option {
                id = "bad",
                kind = "trilateral",
            }
            "#,
        )
        .exec()
        .unwrap();

        let res = parse_diplomatic_option_definitions(lua);
        assert!(res.is_err());
    }

    #[test]
    fn test_parse_diplomatic_option_defaults() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"define_diplomatic_option { id = "minimal" }"#)
            .exec()
            .unwrap();

        let defs = parse_diplomatic_option_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "minimal");
        assert_eq!(defs[0].kind, "bilateral");
        assert!(defs[0].description.is_empty());
    }

    #[test]
    fn test_diplomatic_option_registry_lookup() {
        let mut registry = DiplomaticOptionRegistry::default();
        registry.options.insert(
            "test".to_string(),
            DiplomaticOptionDefinition {
                id: "test".to_string(),
                name: "Test".to_string(),
                description: String::new(),
                kind: "bilateral".to_string(),
                responses: vec![],
                payload_schema: vec![],
            },
        );
        assert!(registry.get("test").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_define_diplomatic_option_returns_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: mlua::Table = lua
            .load(r#"return define_diplomatic_option { id = "test_opt" }"#)
            .eval()
            .unwrap();

        let def_type: String = result.get("_def_type").unwrap();
        assert_eq!(def_type, "diplomatic_option");
        let id: String = result.get("id").unwrap();
        assert_eq!(id, "test_opt");
    }
}
