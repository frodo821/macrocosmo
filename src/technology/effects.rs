use std::collections::HashMap;

use bevy::prelude::*;
use mlua::Lua;

use crate::condition::ScopedFlags;
use crate::effect::DescriptiveEffect;
use crate::player::PlayerEmpire;
use crate::scripting::effect_scope::{collect_effects, EffectScope};
use crate::scripting::ScriptEngine;
use crate::technology::tree::TechId;
use crate::technology::{GameFlags, GlobalParams};

use super::research::RecentlyResearched;

/// Stores the effects applied by each researched technology, for UI display.
#[derive(Resource, Default)]
pub struct TechEffectsLog {
    pub effects: HashMap<TechId, Vec<DescriptiveEffect>>,
}

/// Pre-computed preview of effects each technology would produce when researched.
///
/// Built once at startup by dry-running each tech's `on_researched` callback
/// against a fresh `EffectScope` (effects are only collected, not applied to
/// game state). Consumed by the research panel UI so players can see what
/// every tech does before unlocking it.
///
/// This is distinct from `TechEffectsLog`, which records effects only after
/// a tech has actually been researched.
#[derive(Resource, Default, Debug)]
pub struct TechEffectsPreview {
    pub effects: HashMap<TechId, Vec<DescriptiveEffect>>,
}

impl TechEffectsPreview {
    /// Returns the previewed effects for a tech, or an empty slice if none
    /// (either the tech has no `on_researched` callback or it failed to
    /// preview cleanly).
    pub fn for_tech(&self, tech_id: &TechId) -> &[DescriptiveEffect] {
        self.effects
            .get(tech_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

/// Startup system that builds `TechEffectsPreview` by dry-running each tech's
/// `on_researched` callback. Side effects are NOT applied to game state — the
/// `EffectScope` simply collects `DescriptiveEffect` records, which we read out
/// and discard the rest (pending flags / global mods are dropped).
///
/// Runs after `load_technologies` and `load_all_scripts` so all tech
/// definitions are available in `_tech_definitions`.
pub fn build_tech_effects_preview(
    engine: Option<Res<ScriptEngine>>,
    tech_trees: Query<&crate::technology::TechTree>,
    tech_tree_res: Option<Res<crate::technology::TechTree>>,
    mut preview: ResMut<TechEffectsPreview>,
) {
    preview.effects.clear();

    let Some(engine) = engine else {
        return;
    };
    let lua = engine.lua();

    // Snapshot tech IDs so we don't borrow the world during Lua execution.
    let tech_ids: Vec<TechId> = if let Some(tree) = tech_trees.iter().next() {
        tree.technologies.keys().cloned().collect()
    } else if let Some(tree) = tech_tree_res.as_deref() {
        tree.technologies.keys().cloned().collect()
    } else {
        return;
    };

    let Ok(tech_defs) = lua.globals().get::<mlua::Table>("_tech_definitions") else {
        return;
    };

    for tech_id in tech_ids {
        let Some(func) = find_on_researched(&tech_defs, &tech_id.0) else {
            continue;
        };
        let scope = EffectScope::new();
        let result = match func.call::<mlua::Value>(scope.clone()) {
            Ok(v) => v,
            Err(e) => {
                debug!(
                    "preview: on_researched for tech {} failed: {e}",
                    tech_id.0
                );
                continue;
            }
        };
        let effects = match collect_effects(&scope, result) {
            Ok(e) => e,
            Err(e) => {
                debug!(
                    "preview: collect_effects for tech {} failed: {e}",
                    tech_id.0
                );
                continue;
            }
        };
        // Drain side-effect tables that the callback may have populated, so
        // they don't leak into real research later. (Real research re-runs
        // the callback through `apply_tech_effects`, which both collects and
        // applies; the preview pass must leave game state untouched.)
        let _ = drain_pending_global_mods(lua);
        let _ = crate::scripting::lifecycle::drain_pending_flags(lua);

        if !effects.is_empty() {
            preview.effects.insert(tech_id, effects);
        }
    }

    info!(
        "TechEffectsPreview built: {} techs with previewable effects",
        preview.effects.len()
    );
}

/// Drain `_pending_global_mods` from Lua and return (param_name, value) pairs.
pub fn drain_pending_global_mods(lua: &Lua) -> Vec<(String, f64)> {
    let Ok(mods) = lua.globals().get::<mlua::Table>("_pending_global_mods") else {
        return Vec::new();
    };
    let Ok(len) = mods.len() else {
        return Vec::new();
    };
    if len == 0 {
        return Vec::new();
    }

    let mut result = Vec::new();
    for i in 1..=len {
        if let Ok(entry) = mods.get::<mlua::Table>(i) {
            if let (Ok(param), Ok(value)) = (
                entry.get::<String>("param"),
                entry.get::<f64>("value"),
            ) {
                result.push((param, value));
            }
        }
    }

    // Clear the table
    if let Ok(new_table) = lua.create_table() {
        let _ = lua.globals().set("_pending_global_mods", new_table);
    }

    result
}

/// Apply a global param modification to GlobalParams.
fn apply_global_mod(params: &mut GlobalParams, param_name: &str, value: f64) {
    match param_name {
        "sublight_speed_bonus" => params.sublight_speed_bonus += value,
        "ftl_speed_multiplier" => params.ftl_speed_multiplier += value,
        "ftl_range_bonus" => params.ftl_range_bonus += value,
        "survey_range_bonus" => params.survey_range_bonus += value,
        "build_speed_multiplier" => params.build_speed_multiplier *= 1.0 + value,
        _ => {
            warn!("Unknown global param: {param_name}");
        }
    }
}

/// System that executes `on_researched` Lua callbacks for recently completed techs.
///
/// For each tech in `RecentlyResearched`:
/// 1. Look up `on_researched` in the `_tech_definitions` Lua table
/// 2. Create an `EffectScope` and call the function
/// 3. Collect effects via `collect_effects()`
/// 4. Apply each `DescriptiveEffect` to game state
/// 5. Log effects in `TechEffectsLog` for UI display
///
/// This system must run AFTER `tick_research` (which populates `RecentlyResearched`)
/// and BEFORE `propagate_tech_knowledge` (which drains `RecentlyResearched`).
pub fn apply_tech_effects(
    engine: Option<Res<ScriptEngine>>,
    mut empire_q: Query<
        (
            &RecentlyResearched,
            &mut GameFlags,
            &mut ScopedFlags,
            &mut GlobalParams,
        ),
        With<PlayerEmpire>,
    >,
    mut effects_log: ResMut<TechEffectsLog>,
) {
    let Some(engine) = engine else {
        return;
    };

    let Ok((recently, mut game_flags, mut scoped_flags, mut global_params)) =
        empire_q.single_mut()
    else {
        return;
    };

    if recently.techs.is_empty() {
        return;
    }

    let lua = engine.lua();

    // Get the _tech_definitions table
    let Ok(tech_defs) = lua.globals().get::<mlua::Table>("_tech_definitions") else {
        warn!("_tech_definitions table not found in Lua globals");
        return;
    };

    for tech_id in &recently.techs {
        // Find this tech's definition in _tech_definitions
        let on_researched_fn = find_on_researched(&tech_defs, &tech_id.0);
        let Some(func) = on_researched_fn else {
            debug!("No on_researched callback for tech {}", tech_id.0);
            continue;
        };

        // Create EffectScope and call the callback
        let scope = EffectScope::new();
        let result = func.call::<mlua::Value>(scope.clone());

        let effects = match result {
            Ok(return_value) => match collect_effects(&scope, return_value) {
                Ok(effects) => effects,
                Err(e) => {
                    warn!("Failed to collect effects for tech {}: {e}", tech_id.0);
                    continue;
                }
            },
            Err(e) => {
                warn!("on_researched callback failed for tech {}: {e}", tech_id.0);
                continue;
            }
        };

        if effects.is_empty() {
            continue;
        }

        // Apply each effect
        for effect in &effects {
            apply_effect(
                effect,
                &mut game_flags,
                &mut scoped_flags,
                &mut global_params,
            );
        }

        info!(
            "Applied {} effects for tech {}",
            effects.len(),
            tech_id.0
        );

        // Log for UI display
        effects_log.effects.insert(tech_id.clone(), effects);

        // Drain any pending global mods that the callback may have set via modify_global()
        let pending_mods = drain_pending_global_mods(lua);
        for (param, value) in pending_mods {
            apply_global_mod(&mut global_params, &param, value);
        }

        // Drain any pending flags set via set_flag()
        let pending_flags = crate::scripting::lifecycle::drain_pending_flags(lua);
        for flag in &pending_flags {
            game_flags.set(flag);
            scoped_flags.set(flag);
        }
    }
}

/// Apply a single DescriptiveEffect to game state.
fn apply_effect(
    effect: &DescriptiveEffect,
    game_flags: &mut GameFlags,
    scoped_flags: &mut ScopedFlags,
    global_params: &mut GlobalParams,
) {
    match effect {
        DescriptiveEffect::PushModifier {
            target,
            base_add,
            multiplier,
            add,
            ..
        } => {
            // Map well-known modifier targets to GlobalParams fields
            apply_modifier_to_params(global_params, target, *base_add, *multiplier, *add);
        }
        DescriptiveEffect::PopModifier { .. } => {
            // PopModifier is for removing temporary modifiers; not applicable at tech level
            debug!("PopModifier in on_researched is a no-op (tech effects are permanent)");
        }
        DescriptiveEffect::SetFlag {
            name,
            value,
            ..
        } => {
            if *value {
                game_flags.set(name);
                scoped_flags.set(name);
            }
            // Note: unsetting flags from tech research is unusual but supported
        }
        DescriptiveEffect::FireEvent { event_id, .. } => {
            // Fire events are handled by the event system; queue them
            info!("Tech effect requests event fire: {event_id} (not yet wired to EventSystem)");
        }
        DescriptiveEffect::Hidden { inner, .. } => {
            apply_effect(inner, game_flags, scoped_flags, global_params);
        }
    }
}

/// Map modifier targets to GlobalParams fields.
/// Targets like "ship.sublight_speed", "ship.ftl_range", etc. map to GlobalParams.
/// Other targets are logged but not applied (they may be used by future systems).
fn apply_modifier_to_params(
    params: &mut GlobalParams,
    target: &str,
    base_add: f64,
    multiplier: f64,
    add: f64,
) {
    match target {
        "ship.sublight_speed" => {
            params.sublight_speed_bonus += base_add + add;
        }
        "ship.ftl_speed" => {
            if multiplier != 0.0 {
                params.ftl_speed_multiplier += multiplier;
            }
            params.sublight_speed_bonus += base_add + add; // fallback additive
        }
        "ship.ftl_range" => {
            params.ftl_range_bonus += base_add + add;
        }
        "sensor.range" => {
            params.survey_range_bonus += base_add + add;
        }
        "construction.speed" => {
            if multiplier != 0.0 {
                // multiplier is fractional, e.g. 0.10 means +10%
                params.build_speed_multiplier *= 1.0 / (1.0 + multiplier);
            }
        }
        // Production/combat/diplomacy modifiers are stored in TechEffectsLog
        // for display but don't currently have GlobalParams fields.
        // They will be consumed by more granular modifier systems in the future.
        _ => {
            debug!(
                "Modifier target '{target}' stored in TechEffectsLog (no GlobalParams mapping)"
            );
        }
    }
}

/// Find the on_researched function for a tech by scanning _tech_definitions.
fn find_on_researched(
    tech_defs: &mlua::Table,
    tech_id: &str,
) -> Option<mlua::Function> {
    let len = tech_defs.len().ok()?;
    for i in 1..=len {
        let Ok(def) = tech_defs.get::<mlua::Table>(i) else {
            continue;
        };
        let Ok(id) = def.get::<String>("id") else {
            continue;
        };
        if id == tech_id {
            return def.get::<mlua::Function>("on_researched").ok();
        }
    }
    // Also check by looking up a keyed entry (in case definitions are stored by id)
    if let Ok(def) = tech_defs.get::<mlua::Table>(tech_id.to_string()) {
        return def.get::<mlua::Function>("on_researched").ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_drain_pending_global_mods() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            modify_global("sublight_speed_bonus", 0.5)
            modify_global("ftl_range_bonus", 2.0)
            "#,
        )
        .exec()
        .unwrap();

        let mods = drain_pending_global_mods(lua);
        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].0, "sublight_speed_bonus");
        assert!((mods[0].1 - 0.5).abs() < 1e-10);
        assert_eq!(mods[1].0, "ftl_range_bonus");
        assert!((mods[1].1 - 2.0).abs() < 1e-10);

        // After draining, should be empty
        let mods_after = drain_pending_global_mods(lua);
        assert!(mods_after.is_empty());
    }

    #[test]
    fn test_drain_pending_global_mods_empty() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let mods = drain_pending_global_mods(lua);
        assert!(mods.is_empty());
    }

    #[test]
    fn test_apply_global_mod() {
        let mut params = GlobalParams::default();
        apply_global_mod(&mut params, "sublight_speed_bonus", 0.5);
        assert!((params.sublight_speed_bonus - 0.5).abs() < 1e-10);

        apply_global_mod(&mut params, "ftl_range_bonus", 3.0);
        assert!((params.ftl_range_bonus - 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_find_on_researched() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_tech {
                id = "test_tech",
                name = "Test",
                on_researched = function(scope)
                    scope:set_flag("test_flag", true)
                end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let tech_defs: mlua::Table = lua.globals().get("_tech_definitions").unwrap();
        let func = find_on_researched(&tech_defs, "test_tech");
        assert!(func.is_some());

        let func_missing = find_on_researched(&tech_defs, "nonexistent_tech");
        assert!(func_missing.is_none());
    }

    #[test]
    fn test_on_researched_sets_flags_via_scope() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_tech {
                id = "flag_tech",
                name = "Flag Tech",
                on_researched = function(scope)
                    scope:set_flag("my_test_flag", true, { description = "A test flag" })
                    scope:push_modifier("production.minerals", { multiplier = 0.15 })
                end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let tech_defs: mlua::Table = lua.globals().get("_tech_definitions").unwrap();
        let func = find_on_researched(&tech_defs, "flag_tech").unwrap();

        let scope = EffectScope::new();
        let result = func.call::<mlua::Value>(scope.clone()).unwrap();
        let effects = collect_effects(&scope, result).unwrap();

        assert_eq!(effects.len(), 2);

        // First effect: SetFlag
        match &effects[0] {
            DescriptiveEffect::SetFlag {
                name,
                value,
                description,
            } => {
                assert_eq!(name, "my_test_flag");
                assert!(*value);
                assert_eq!(description.as_deref(), Some("A test flag"));
            }
            _ => panic!("Expected SetFlag, got {:?}", effects[0]),
        }

        // Second effect: PushModifier
        match &effects[1] {
            DescriptiveEffect::PushModifier {
                target, multiplier, ..
            } => {
                assert_eq!(target, "production.minerals");
                assert!((multiplier - 0.15).abs() < 1e-10);
            }
            _ => panic!("Expected PushModifier, got {:?}", effects[1]),
        }
    }

    #[test]
    fn test_apply_modifier_to_params_ship_speed() {
        let mut params = GlobalParams::default();
        apply_modifier_to_params(&mut params, "ship.sublight_speed", 0.0, 0.0, 0.1);
        assert!((params.sublight_speed_bonus - 0.1).abs() < 1e-10);
    }

    #[test]
    fn test_apply_modifier_to_params_ftl_range() {
        let mut params = GlobalParams::default();
        apply_modifier_to_params(&mut params, "ship.ftl_range", 0.0, 0.0, 5.0);
        assert!((params.ftl_range_bonus - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_apply_modifier_to_params_survey_range() {
        let mut params = GlobalParams::default();
        apply_modifier_to_params(&mut params, "sensor.range", 0.0, 0.0, 2.0);
        assert!((params.survey_range_bonus - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_apply_modifier_to_params_construction() {
        let mut params = GlobalParams::default();
        // +10% construction speed means build_speed_multiplier decreases
        apply_modifier_to_params(&mut params, "construction.speed", 0.0, 0.10, 0.0);
        // 1.0 / 1.10 ~ 0.909
        assert!((params.build_speed_multiplier - (1.0 / 1.1)).abs() < 1e-10);
    }

    #[test]
    fn test_apply_effect_set_flag() {
        let mut game_flags = GameFlags::default();
        let mut scoped_flags = ScopedFlags::default();
        let mut global_params = GlobalParams::default();

        let effect = DescriptiveEffect::SetFlag {
            name: "test_flag".into(),
            value: true,
            description: None,
        };

        apply_effect(&effect, &mut game_flags, &mut scoped_flags, &mut global_params);

        assert!(game_flags.check("test_flag"));
        assert!(scoped_flags.check("test_flag"));
    }

    #[test]
    fn test_tech_effects_log() {
        let mut log = TechEffectsLog::default();
        let tech_id = TechId("test_tech".into());
        let effects = vec![DescriptiveEffect::SetFlag {
            name: "flag".into(),
            value: true,
            description: None,
        }];
        log.effects.insert(tech_id.clone(), effects);
        assert_eq!(log.effects.get(&tech_id).unwrap().len(), 1);
    }

    // ---------------------------------------------------------------
    // #156: TechEffectsPreview (research-panel UI dry-run preview)
    // ---------------------------------------------------------------

    /// Build a preview by running the system in a fresh ECS world with the
    /// given Lua source and tech tree. Returns the populated resource.
    fn run_preview(lua_src: &str, tree: crate::technology::TechTree) -> TechEffectsPreview {
        let engine = ScriptEngine::new().unwrap();
        engine.lua().load(lua_src).exec().unwrap();

        let mut app = App::new();
        app.insert_resource(engine);
        app.init_resource::<TechEffectsPreview>();
        app.insert_resource(tree);
        app.add_systems(Update, build_tech_effects_preview);
        app.update();
        app.world_mut()
            .remove_resource::<TechEffectsPreview>()
            .expect("TechEffectsPreview should exist after update")
    }

    #[test]
    fn preview_collects_effects_from_on_researched() {
        use crate::technology::tree::{TechCost, Technology};
        let tree = crate::technology::TechTree::from_vec(vec![Technology {
            id: TechId("automated_mining".into()),
            name: "Automated Mining".into(),
            branch: "industrial".into(),
            cost: TechCost::research_only(crate::amount::Amt::units(100)),
            prerequisites: vec![],
            description: String::new(),
            dangerous: false,
        }]);

        let preview = run_preview(
            r#"
            define_tech {
                id = "automated_mining",
                name = "Automated Mining",
                on_researched = function(scope)
                    scope:push_modifier("production.minerals", { multiplier = 0.15, description = "Mineral production +15%" })
                    scope:set_flag("automated_mining_unlocked", true, { description = "Enables automated mining facilities" })
                end,
            }
            "#,
            tree,
        );

        let effects = preview.for_tech(&TechId("automated_mining".into()));
        assert_eq!(effects.len(), 2);
        assert_eq!(effects[0].display_text(), "Mineral production +15%");
        assert_eq!(
            effects[1].display_text(),
            "Enables automated mining facilities"
        );
    }

    #[test]
    fn preview_skips_techs_without_on_researched() {
        use crate::technology::tree::{TechCost, Technology};
        let tree = crate::technology::TechTree::from_vec(vec![Technology {
            id: TechId("plain".into()),
            name: "Plain".into(),
            branch: "physics".into(),
            cost: TechCost::research_only(crate::amount::Amt::units(50)),
            prerequisites: vec![],
            description: String::new(),
            dangerous: false,
        }]);

        let preview = run_preview(
            r#"
            define_tech { id = "plain", name = "Plain" }
            "#,
            tree,
        );

        // No on_researched -> no entry, but the resource exists and is empty.
        assert!(preview.for_tech(&TechId("plain".into())).is_empty());
        assert!(preview.effects.is_empty());
    }

    #[test]
    fn preview_for_tech_returns_empty_for_unknown_id() {
        let preview = TechEffectsPreview::default();
        assert!(preview.for_tech(&TechId("nonexistent".into())).is_empty());
    }

    #[test]
    fn preview_does_not_leak_pending_global_mods() {
        use crate::technology::tree::{TechCost, Technology};
        let tree = crate::technology::TechTree::from_vec(vec![Technology {
            id: TechId("speedy".into()),
            name: "Speedy".into(),
            branch: "physics".into(),
            cost: TechCost::research_only(crate::amount::Amt::units(75)),
            prerequisites: vec![],
            description: String::new(),
            dangerous: false,
        }]);

        // The callback uses both scope methods (which the preview *should*
        // capture) and modify_global / set_flag (which the preview must
        // drain so they don't leak into the next real research event).
        let engine = ScriptEngine::new().unwrap();
        engine
            .lua()
            .load(
                r#"
            define_tech {
                id = "speedy",
                name = "Speedy",
                on_researched = function(scope)
                    scope:push_modifier("ship.sublight_speed", { add = 0.5, description = "Speed +0.5" })
                    modify_global("sublight_speed_bonus", 0.5)
                    set_flag("speedy_unlocked")
                end,
            }
            "#,
            )
            .exec()
            .unwrap();

        let mut app = App::new();
        app.insert_resource(engine);
        app.init_resource::<TechEffectsPreview>();
        app.insert_resource(tree);
        app.add_systems(Update, build_tech_effects_preview);
        app.update();

        // Side-effect tables must be empty after the preview pass.
        let engine = app.world().resource::<ScriptEngine>();
        let lua = engine.lua();
        let pending_mods: mlua::Table = lua.globals().get("_pending_global_mods").unwrap();
        assert_eq!(pending_mods.len().unwrap(), 0);
        let pending_flags: mlua::Table = lua.globals().get("_pending_flags").unwrap();
        assert_eq!(pending_flags.len().unwrap(), 0);

        // But the preview captured the scope effect.
        let preview = app.world().resource::<TechEffectsPreview>();
        let effects = preview.for_tech(&TechId("speedy".into()));
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].display_text(), "Speed +0.5");
    }
}
