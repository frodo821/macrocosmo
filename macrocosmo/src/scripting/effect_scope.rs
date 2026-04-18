use crate::effect::DescriptiveEffect;
use mlua::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Accumulated effects from a Lua callback execution.
#[derive(Default, Clone)]
pub struct EffectAccumulator {
    pub effects: Vec<DescriptiveEffect>,
}

/// Lua UserData passed as `scope` to on_researched, on_chosen, etc.
/// Each method both accumulates the effect AND returns a Lua descriptor table.
#[derive(Clone)]
pub struct EffectScope {
    pub accumulator: Arc<Mutex<EffectAccumulator>>,
}

impl EffectScope {
    pub fn new() -> Self {
        Self {
            accumulator: Arc::new(Mutex::new(EffectAccumulator::default())),
        }
    }

    /// Take all accumulated effects, leaving the accumulator empty.
    pub fn take_effects(&self) -> Vec<DescriptiveEffect> {
        std::mem::take(&mut self.accumulator.lock().unwrap().effects)
    }
}

impl mlua::UserData for EffectScope {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // scope:push_modifier(target, opts)
        // opts: { base_add, multiplier, add, description } (all optional)
        methods.add_method(
            "push_modifier",
            |lua, this, (target, opts): (String, LuaTable)| {
                let base_add: f64 = opts.get("base_add").unwrap_or(0.0);
                let multiplier: f64 = opts.get("multiplier").unwrap_or(0.0);
                let add: f64 = opts.get("add").unwrap_or(0.0);
                let description: Option<String> = opts.get("description").ok();

                let effect = DescriptiveEffect::PushModifier {
                    target: target.clone(),
                    base_add,
                    multiplier,
                    add,
                    description: description.clone(),
                };

                this.accumulator.lock().unwrap().effects.push(effect);

                // Return a descriptor table
                let desc = lua.create_table()?;
                desc.set("_effect_type", "push_modifier")?;
                desc.set("target", target)?;
                desc.set("base_add", base_add)?;
                desc.set("multiplier", multiplier)?;
                desc.set("add", add)?;
                if let Some(d) = description {
                    desc.set("description", d)?;
                }
                Ok(desc)
            },
        );

        // scope:pop_modifier(target)
        methods.add_method("pop_modifier", |lua, this, target: String| {
            let effect = DescriptiveEffect::PopModifier {
                target: target.clone(),
            };
            this.accumulator.lock().unwrap().effects.push(effect);

            let desc = lua.create_table()?;
            desc.set("_effect_type", "pop_modifier")?;
            desc.set("target", target)?;
            Ok(desc)
        });

        // scope:set_flag(name, value, opts?)
        // opts: { description } (optional)
        methods.add_method(
            "set_flag",
            |lua, this, (name, value, opts): (String, bool, Option<LuaTable>)| {
                let description: Option<String> =
                    opts.as_ref().and_then(|t| t.get("description").ok());

                let effect = DescriptiveEffect::SetFlag {
                    name: name.clone(),
                    value,
                    description: description.clone(),
                };

                this.accumulator.lock().unwrap().effects.push(effect);

                let desc = lua.create_table()?;
                desc.set("_effect_type", "set_flag")?;
                desc.set("name", name)?;
                desc.set("value", value)?;
                if let Some(d) = description {
                    desc.set("description", d)?;
                }
                Ok(desc)
            },
        );
    }
}

/// Create a `fire_event` descriptor table (standalone function, not on scope).
/// This returns a descriptor but does NOT queue the event for execution.
pub fn create_fire_event_descriptor(lua: &Lua) -> Result<LuaFunction, LuaError> {
    lua.create_function(|lua, (event_id, payload): (String, Option<LuaTable>)| {
        let desc = lua.create_table()?;
        desc.set("_effect_type", "fire_event")?;
        desc.set("event_id", event_id)?;
        if let Some(p) = payload {
            desc.set("payload", p)?;
        }
        Ok(desc)
    })
}

/// Create a `hide` wrapper function that wraps a descriptor with a label.
pub fn create_hide_function(lua: &Lua) -> Result<LuaFunction, LuaError> {
    lua.create_function(|lua, (label, inner): (String, LuaTable)| {
        let desc = lua.create_table()?;
        desc.set("_effect_type", "hidden")?;
        desc.set("label", label)?;
        desc.set("inner", inner)?;
        Ok(desc)
    })
}

/// Parse a Lua return value into a Vec of DescriptiveEffects.
/// Handles: Nil -> empty, single table -> check if it's a descriptor or a sequence,
/// sequence of descriptor tables.
pub fn parse_effects(value: LuaValue) -> Result<Vec<DescriptiveEffect>, LuaError> {
    match value {
        LuaValue::Nil => Ok(Vec::new()),
        LuaValue::Table(table) => {
            // Check if this is a single descriptor (has _effect_type)
            if let Ok(effect_type) = table.get::<String>("_effect_type") {
                return Ok(vec![parse_single_effect(&table, &effect_type)?]);
            }
            // Otherwise treat as a sequence of descriptors
            let mut effects = Vec::new();
            for pair in table.sequence_values::<LuaTable>() {
                let entry = pair?;
                let effect_type: String = entry.get("_effect_type").map_err(|_| {
                    LuaError::RuntimeError(
                        "Effect descriptor table missing _effect_type field".into(),
                    )
                })?;
                effects.push(parse_single_effect(&entry, &effect_type)?);
            }
            Ok(effects)
        }
        _ => Err(LuaError::RuntimeError(format!(
            "Expected nil or table from effect callback, got {:?}",
            value.type_name()
        ))),
    }
}

fn parse_single_effect(table: &LuaTable, effect_type: &str) -> Result<DescriptiveEffect, LuaError> {
    match effect_type {
        "push_modifier" => {
            let target: String = table.get("target")?;
            let base_add: f64 = table.get("base_add").unwrap_or(0.0);
            let multiplier: f64 = table.get("multiplier").unwrap_or(0.0);
            let add: f64 = table.get("add").unwrap_or(0.0);
            let description: Option<String> = table.get("description").ok();
            Ok(DescriptiveEffect::PushModifier {
                target,
                base_add,
                multiplier,
                add,
                description,
            })
        }
        "pop_modifier" => {
            let target: String = table.get("target")?;
            Ok(DescriptiveEffect::PopModifier { target })
        }
        "set_flag" => {
            let name: String = table.get("name")?;
            let value: bool = table.get("value").unwrap_or(true);
            let description: Option<String> = table.get("description").ok();
            Ok(DescriptiveEffect::SetFlag {
                name,
                value,
                description,
            })
        }
        "fire_event" => {
            let event_id: String = table.get("event_id")?;
            let mut payload = HashMap::new();
            if let Ok(p) = table.get::<LuaTable>("payload") {
                for pair in p.pairs::<String, String>() {
                    let (k, v) = pair?;
                    payload.insert(k, v);
                }
            }
            Ok(DescriptiveEffect::FireEvent { event_id, payload })
        }
        "hidden" => {
            let label: String = table.get("label")?;
            let inner_table: LuaTable = table.get("inner")?;
            let inner_type: String = inner_table.get("_effect_type")?;
            let inner = parse_single_effect(&inner_table, &inner_type)?;
            Ok(DescriptiveEffect::Hidden {
                label,
                inner: Box::new(inner),
            })
        }
        _ => Err(LuaError::RuntimeError(format!(
            "Unknown effect type: {effect_type}"
        ))),
    }
}

/// Collect effects from both the accumulator and the return value, deduplicating.
/// Effects that appear in both (same type + key fields) are included only once.
pub fn collect_effects(
    scope: &EffectScope,
    return_value: LuaValue,
) -> Result<Vec<DescriptiveEffect>, LuaError> {
    let accumulated = scope.take_effects();
    let returned = parse_effects(return_value)?;

    if returned.is_empty() {
        return Ok(accumulated);
    }
    if accumulated.is_empty() {
        return Ok(returned);
    }

    // Both patterns were used. Since scope methods both accumulate AND return,
    // the returned effects are a subset of accumulated effects.
    // Just return the accumulated effects (they're the superset).
    Ok(accumulated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_effect_scope_accumulates_push_modifier() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // Register the scope as a global for testing
        lua.globals().set("scope", scope.clone()).unwrap();

        lua.load(r#"scope:push_modifier("production.minerals", { multiplier = 0.15 })"#)
            .exec()
            .unwrap();

        let effects = scope.take_effects();
        assert_eq!(effects.len(), 1);
        match &effects[0] {
            DescriptiveEffect::PushModifier {
                target, multiplier, ..
            } => {
                assert_eq!(target, "production.minerals");
                assert!((multiplier - 0.15).abs() < 1e-10);
            }
            _ => panic!("Expected PushModifier"),
        }
    }

    #[test]
    fn test_effect_scope_accumulates_multiple() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        lua.globals().set("scope", scope.clone()).unwrap();

        lua.load(
            r#"
            scope:push_modifier("production.minerals", { multiplier = 0.15 })
            scope:set_flag("automated_mining", true)
            scope:pop_modifier("production.energy")
            "#,
        )
        .exec()
        .unwrap();

        let effects = scope.take_effects();
        assert_eq!(effects.len(), 3);
        assert!(matches!(
            &effects[0],
            DescriptiveEffect::PushModifier { .. }
        ));
        assert!(matches!(&effects[1], DescriptiveEffect::SetFlag { .. }));
        assert!(matches!(&effects[2], DescriptiveEffect::PopModifier { .. }));
    }

    #[test]
    fn test_effect_scope_push_modifier_returns_descriptor() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        lua.globals().set("scope", scope.clone()).unwrap();

        let result: LuaTable = lua
            .load(
                r#"return scope:push_modifier("production.minerals", { multiplier = 0.15, description = "Mining bonus" })"#,
            )
            .eval()
            .unwrap();

        let effect_type: String = result.get("_effect_type").unwrap();
        assert_eq!(effect_type, "push_modifier");
        let target: String = result.get("target").unwrap();
        assert_eq!(target, "production.minerals");
        let mult: f64 = result.get("multiplier").unwrap();
        assert!((mult - 0.15).abs() < 1e-10);
        let desc: String = result.get("description").unwrap();
        assert_eq!(desc, "Mining bonus");
    }

    #[test]
    fn test_effect_scope_set_flag_with_description() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        lua.globals().set("scope", scope.clone()).unwrap();

        lua.load(
            r#"scope:set_flag("auto_mining", true, { description = "Enable automated mining" })"#,
        )
        .exec()
        .unwrap();

        let effects = scope.take_effects();
        assert_eq!(effects.len(), 1);
        match &effects[0] {
            DescriptiveEffect::SetFlag {
                name,
                value,
                description,
            } => {
                assert_eq!(name, "auto_mining");
                assert!(*value);
                assert_eq!(description.as_deref(), Some("Enable automated mining"));
            }
            _ => panic!("Expected SetFlag"),
        }
    }

    #[test]
    fn test_parse_effects_nil() {
        let effects = parse_effects(LuaValue::Nil).unwrap();
        assert!(effects.is_empty());
    }

    #[test]
    fn test_parse_effects_from_return_value() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        lua.globals().set("scope", scope.clone()).unwrap();

        // Pattern 2: return effect descriptors
        let result: LuaValue = lua
            .load(
                r#"
                return {
                    scope:push_modifier("production.minerals", { multiplier = 0.15 }),
                    scope:set_flag("automated_mining", true),
                }
                "#,
            )
            .eval()
            .unwrap();

        let returned = parse_effects(result).unwrap();
        assert_eq!(returned.len(), 2);
        assert!(matches!(
            &returned[0],
            DescriptiveEffect::PushModifier { .. }
        ));
        assert!(matches!(&returned[1], DescriptiveEffect::SetFlag { .. }));
    }

    #[test]
    fn test_parse_effects_fire_event() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let fire_event_fn = create_fire_event_descriptor(lua).unwrap();
        lua.globals()
            .set("effect_fire_event", fire_event_fn)
            .unwrap();

        let result: LuaValue = lua
            .load(
                r#"
                return {
                    effect_fire_event("first_contact", { species = "alien" }),
                }
                "#,
            )
            .eval()
            .unwrap();

        let effects = parse_effects(result).unwrap();
        assert_eq!(effects.len(), 1);
        match &effects[0] {
            DescriptiveEffect::FireEvent { event_id, payload } => {
                assert_eq!(event_id, "first_contact");
                assert_eq!(payload.get("species"), Some(&"alien".to_string()));
            }
            _ => panic!("Expected FireEvent"),
        }
    }

    #[test]
    fn test_parse_effects_hidden() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        lua.globals().set("scope", scope.clone()).unwrap();
        let hide_fn = create_hide_function(lua).unwrap();
        lua.globals().set("hide", hide_fn).unwrap();

        let result: LuaValue = lua
            .load(
                r#"
                return {
                    hide("Something mysterious...", scope:set_flag("secret", true)),
                }
                "#,
            )
            .eval()
            .unwrap();

        let effects = parse_effects(result).unwrap();
        assert_eq!(effects.len(), 1);
        match &effects[0] {
            DescriptiveEffect::Hidden { label, inner } => {
                assert_eq!(label, "Something mysterious...");
                assert!(matches!(**inner, DescriptiveEffect::SetFlag { .. }));
            }
            _ => panic!("Expected Hidden"),
        }
    }

    #[test]
    fn test_collect_effects_imperative_pattern() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        lua.globals().set("scope", scope.clone()).unwrap();

        // Pattern 1: imperative (no return value)
        lua.load(
            r#"
            scope:push_modifier("production.minerals", { multiplier = 0.15 })
            scope:set_flag("automated_mining", true)
            "#,
        )
        .exec()
        .unwrap();

        let effects = collect_effects(&scope, LuaValue::Nil).unwrap();
        assert_eq!(effects.len(), 2);
    }

    #[test]
    fn test_collect_effects_declarative_pattern() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        lua.globals().set("scope", scope.clone()).unwrap();

        // Pattern 2: declarative (return value)
        let result: LuaValue = lua
            .load(
                r#"
                return {
                    scope:push_modifier("production.minerals", { multiplier = 0.15 }),
                    scope:set_flag("automated_mining", true),
                }
                "#,
            )
            .eval()
            .unwrap();

        // Both accumulator and return have the same effects.
        // collect_effects should deduplicate by returning the accumulated set.
        let effects = collect_effects(&scope, result).unwrap();
        assert_eq!(effects.len(), 2);
    }

    #[test]
    fn test_take_effects_clears_accumulator() {
        let scope = EffectScope::new();
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        lua.globals().set("scope", scope.clone()).unwrap();

        lua.load(r#"scope:push_modifier("x", { multiplier = 0.1 })"#)
            .exec()
            .unwrap();

        let effects = scope.take_effects();
        assert_eq!(effects.len(), 1);

        // Second take should be empty
        let effects2 = scope.take_effects();
        assert!(effects2.is_empty());
    }
}
