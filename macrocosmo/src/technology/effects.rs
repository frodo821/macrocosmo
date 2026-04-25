use std::collections::HashMap;

use bevy::prelude::*;
use mlua::Lua;

use crate::amount::SignedAmt;
use crate::condition::ScopedFlags;
use crate::effect::DescriptiveEffect;
use crate::modifier::{Modifier, ParsedModifier};
use crate::player::Empire;
use crate::scripting::ScriptEngine;
use crate::scripting::effect_scope::{EffectScope, collect_effects};
use crate::technology::tree::TechId;
use crate::technology::{EmpireModifiers, GameBalance, GameFlags, GlobalParams};

use super::research::RecentlyResearched;

/// #245: Queue of tech-sourced modifiers whose `target` refers to a colony
/// aggregator (`colony.*_per_hexadies`), a job slot (`colony.<job>_slot`), or
/// a per-job rate bucket (`job:<id>::...`). `apply_tech_effects` appends one
/// entry per matching `DescriptiveEffect::PushModifier` encountered during a
/// tech's `on_researched` callback; `sync_tech_colony_modifiers` then
/// broadcasts those modifiers into every colony every tick.
///
/// The append-only semantics make late-spawning colonies idempotently pick
/// up already-researched tech effects on their first tick.
#[derive(Component, Default, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct PendingColonyTechModifiers {
    pub entries: Vec<(TechId, ParsedModifier)>,
}

impl PendingColonyTechModifiers {
    pub fn push(&mut self, tech_id: TechId, pm: ParsedModifier) {
        // Replace any existing entry with the same (tech_id, target) so that
        // repeated research (or preview drains) remain idempotent.
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|(t, m)| t == &tech_id && m.target == pm.target)
        {
            existing.1 = pm;
        } else {
            self.entries.push((tech_id, pm));
        }
    }
}

/// Stores the effects applied by each researched technology, for UI display.
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
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
#[derive(Resource, Default, Debug, Reflect)]
#[reflect(Resource)]
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
                debug!("preview: on_researched for tech {} failed: {e}", tech_id.0);
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
        // #332-B3: the legacy `_pending_*` queues used to be populated by
        // the deprecated global `modify_global(...)` / `set_flag(name)`
        // helpers. The tech `on_researched` callback tree now only emits
        // `EffectScope` descriptors (preview-safe by construction — they
        // are collected, not applied), so no side-effect drain is needed.
        // The global helpers are removed in B4; leaving the drain here
        // would be defensive noise that mistracks the invariant.

        if !effects.is_empty() {
            preview.effects.insert(tech_id, effects);
        }
    }

    info!(
        "TechEffectsPreview built: {} techs with previewable effects",
        preview.effects.len()
    );
}

// #332-B4: removed `drain_pending_global_mods` and `apply_global_mod`.
// The legacy `modify_global(param, value)` global helper that
// populated `_pending_global_mods` is retired in favour of the
// gamestate setter path and `EffectScope` descriptors; there are no
// remaining production callers.

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
            &mut EmpireModifiers,
            Option<&mut PendingColonyTechModifiers>,
            Option<&mut crate::empire::CommsParams>,
        ),
        With<Empire>,
    >,
    mut balance: ResMut<GameBalance>,
    mut effects_log: ResMut<TechEffectsLog>,
) {
    let Some(engine) = engine else {
        return;
    };

    for (
        recently,
        mut game_flags,
        mut scoped_flags,
        mut global_params,
        mut empire_modifiers,
        mut pending_colony_mods,
        mut comms_params,
    ) in &mut empire_q
    {
        // Fallback storage if the empire entity lacks `PendingColonyTechModifiers`
        // (e.g. legacy test fixtures). In that case colony-targeted modifiers have
        // no one to broadcast them, so we drop them with a warning and the
        // subsequent routing still logs its own diagnostic.
        let mut scratch_pending = PendingColonyTechModifiers::default();
        let pending_ref: &mut PendingColonyTechModifiers = match &mut pending_colony_mods {
            Some(p) => &mut *p,
            None => &mut scratch_pending,
        };
        // #233: Same fallback for CommsParams. Legacy empire entities without the
        // component get a scratch bucket; the field values still "apply" but have
        // no runtime effect, preserving forward-compat for old fixtures.
        let mut scratch_comms = crate::empire::CommsParams::default();
        let comms_ref: &mut crate::empire::CommsParams = match &mut comms_params {
            Some(c) => &mut *c,
            None => &mut scratch_comms,
        };

        if recently.techs.is_empty() {
            continue;
        }

        let lua = engine.lua();

        // Get the _tech_definitions table
        let Ok(tech_defs) = lua.globals().get::<mlua::Table>("_tech_definitions") else {
            warn!("_tech_definitions table not found in Lua globals");
            continue;
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
                    &mut balance,
                    &mut empire_modifiers,
                    pending_ref,
                    comms_ref,
                    tech_id,
                );
            }

            info!("Applied {} effects for tech {}", effects.len(), tech_id.0);

            // Log for UI display
            effects_log.effects.insert(tech_id.clone(), effects);

            // #332-B3: dropped the `_pending_global_mods` / `_pending_flags`
            // drain. The legacy global `modify_global` / `set_flag` helpers
            // have no production Lua callers (tech callbacks use
            // `EffectScope` descriptors exclusively, which are already
            // applied above); B4 removes the globals outright.
        }

        if !scratch_pending.entries.is_empty() {
            warn!(
                "PendingColonyTechModifiers component missing on empire entity; {} colony-targeted tech modifier(s) dropped (setup issue)",
                scratch_pending.entries.len()
            );
        }
    }
}

/// Apply a single DescriptiveEffect to game state.
#[allow(clippy::too_many_arguments)]
fn apply_effect(
    effect: &DescriptiveEffect,
    game_flags: &mut GameFlags,
    scoped_flags: &mut ScopedFlags,
    global_params: &mut GlobalParams,
    balance: &mut GameBalance,
    empire_modifiers: &mut EmpireModifiers,
    pending_colony_mods: &mut PendingColonyTechModifiers,
    comms_params: &mut crate::empire::CommsParams,
    source_tech_id: &TechId,
) {
    match effect {
        DescriptiveEffect::PushModifier {
            target,
            base_add,
            multiplier,
            add,
            ..
        } => {
            // #160: Route "balance.*" targets to GameBalance's modifier stack.
            if let Some(field_name) = target.strip_prefix("balance.") {
                if let Some(mv) = balance.field_mut(field_name) {
                    let modifier_id = format!("tech:{}:{}", source_tech_id.0, target);
                    mv.push_modifier(Modifier {
                        id: modifier_id,
                        label: format!("From tech '{}'", source_tech_id.0),
                        base_add: SignedAmt::from_f64(*base_add),
                        multiplier: SignedAmt::from_f64(*multiplier),
                        add: SignedAmt::from_f64(*add),
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    warn!(
                        "Unknown balance target '{target}' from tech '{}'",
                        source_tech_id.0
                    );
                }
            } else {
                route_tech_modifier(
                    target,
                    *base_add,
                    *multiplier,
                    *add,
                    global_params,
                    empire_modifiers,
                    pending_colony_mods,
                    comms_params,
                    source_tech_id,
                );
            }
        }
        DescriptiveEffect::PopModifier { .. } => {
            // PopModifier is for removing temporary modifiers; not applicable at tech level
            debug!("PopModifier in on_researched is a no-op (tech effects are permanent)");
        }
        DescriptiveEffect::SetFlag { name, value, .. } => {
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
            apply_effect(
                inner,
                game_flags,
                scoped_flags,
                global_params,
                balance,
                empire_modifiers,
                pending_colony_mods,
                comms_params,
                source_tech_id,
            );
        }
    }
}

/// #245: Route a single tech-sourced modifier (non-`balance.*`) to its
/// destination:
/// - `ship.*`, `sensor.range`, `construction.speed` → `GlobalParams`
/// - `population.growth` → `EmpireModifiers`
/// - `colony.*_per_hexadies`, `colony.<job>_slot`, `job:*::*` →
///   `PendingColonyTechModifiers` (broadcast to every colony each tick by
///   `sync_tech_colony_modifiers`)
/// - `combat.*`, `diplomacy.*` → warn (target systems not yet implemented)
/// - Unknown targets → debug (harmless, future work)
#[allow(clippy::too_many_arguments)]
fn route_tech_modifier(
    target: &str,
    base_add: f64,
    multiplier: f64,
    add: f64,
    global_params: &mut GlobalParams,
    empire_modifiers: &mut EmpireModifiers,
    pending_colony_mods: &mut PendingColonyTechModifiers,
    comms_params: &mut crate::empire::CommsParams,
    source_tech_id: &TechId,
) {
    // 1) Ship/sensor/construction targets → GlobalParams (legacy routes kept).
    match target {
        "ship.sublight_speed"
        | "ship.ftl_speed"
        | "ship.ftl_range"
        | "sensor.range"
        | "construction.speed" => {
            apply_modifier_to_params(global_params, target, base_add, multiplier, add);
            return;
        }
        _ => {}
    }

    // 1b) #233: FTL Comm Relay modifiers → CommsParams.
    match target {
        "empire.comm_relay_range"
        | "empire.comm_relay_inv_latency"
        | "fleet.comm_relay_range"
        | "fleet.comm_relay_inv_latency" => {
            let modifier_id = format!("tech:{}:{}", source_tech_id.0, target);
            let modifier = Modifier {
                id: modifier_id,
                label: format!("From tech '{}'", source_tech_id.0),
                base_add: SignedAmt::from_f64(base_add),
                multiplier: SignedAmt::from_f64(multiplier),
                add: SignedAmt::from_f64(add),
                expires_at: None,
                on_expire_event: None,
            };
            let slot = match target {
                "empire.comm_relay_range" => &mut comms_params.empire_relay_range,
                "empire.comm_relay_inv_latency" => &mut comms_params.empire_relay_inv_latency,
                "fleet.comm_relay_range" => &mut comms_params.fleet_relay_range,
                "fleet.comm_relay_inv_latency" => &mut comms_params.fleet_relay_inv_latency,
                _ => unreachable!(),
            };
            slot.push_modifier(modifier);
            return;
        }
        _ => {}
    }

    // 2) Population growth → EmpireModifiers.
    if target == "population.growth" {
        let modifier_id = format!("tech:{}:{}", source_tech_id.0, target);
        empire_modifiers.population_growth.push_modifier(Modifier {
            id: modifier_id,
            label: format!("From tech '{}'", source_tech_id.0),
            base_add: SignedAmt::from_f64(base_add),
            multiplier: SignedAmt::from_f64(multiplier),
            add: SignedAmt::from_f64(add),
            expires_at: None,
            on_expire_event: None,
        });
        return;
    }

    // 3) Colony-scoped targets → PendingColonyTechModifiers queue.
    let is_job_scoped = target.starts_with("job:") && target.contains("::");
    let is_colony_agg = matches!(
        target,
        "colony.minerals_per_hexadies"
            | "colony.energy_per_hexadies"
            | "colony.food_per_hexadies"
            | "colony.research_per_hexadies"
            | "colony.authority_per_hexadies"
    );
    let is_colony_slot = target.starts_with("colony.") && target.ends_with("_slot");
    if is_job_scoped || is_colony_agg || is_colony_slot {
        pending_colony_mods.push(
            source_tech_id.clone(),
            ParsedModifier {
                target: target.to_string(),
                base_add,
                multiplier,
                add,
            },
        );
        return;
    }

    // 4) combat.* / diplomacy.* — system not yet wired (scope of #245 is colony
    // broadcast only). Warn once per call so balance-related mods don't go
    // silent.
    if target.starts_with("combat.") || target.starts_with("diplomacy.") {
        warn!(
            "Tech '{}' targets '{}'; {} system not yet implemented (no-op)",
            source_tech_id.0,
            target,
            if target.starts_with("combat.") {
                "combat"
            } else {
                "diplomacy"
            }
        );
        return;
    }

    // 5) Legacy / unrecognised targets. Catches `production.minerals`-style
    // strings from un-migrated scripts so the regression is visible.
    warn!(
        "Tech '{}' has unrouted modifier target '{}'; ignored",
        source_tech_id.0, target
    );
}

/// #245: Broadcast every `(TechId, ParsedModifier)` entry in
/// `PendingColonyTechModifiers` into every colony's `Production`,
/// `ColonyJobRates`, and `ColonyJobs` components.
///
/// `ModifiedValue::push_modifier` replaces any existing modifier with the same
/// id, so running this every tick is idempotent: the same tech pushes the
/// same id each tick, numerical values don't drift. This also means colonies
/// spawned after the tech was researched pick up the modifier on their first
/// tick without any retroactive bookkeeping.
///
/// Modifier ids follow the `tech:<tech_id>:<target>` convention, matching the
/// balance/ship-route ids used elsewhere in the effect pipeline.
///
/// Target handling:
/// - `colony.<X>_per_hexadies` → pushed into `Production.<X>_per_hexadies`.
/// - `job:<job_id>::<target>` → pushed into the matching bucket of
///   `ColonyJobRates`.
/// - `colony.<job_id>_slot` → increases `JobSlot.capacity` beyond the building
///   baseline. A new slot is appended if the colony didn't have one yet.
pub fn sync_tech_colony_modifiers(
    pending_q: Query<&PendingColonyTechModifiers, With<Empire>>,
    mut colonies: Query<(
        &mut crate::colony::Production,
        Option<&mut crate::colony::ColonyJobRates>,
        Option<&mut crate::species::ColonyJobs>,
    )>,
) {
    // Collect all pending entries from all empires.
    // TODO(#418): scope colony modifier application per-empire via FactionOwner.
    let mut all_entries: Vec<(&TechId, &ParsedModifier)> = Vec::new();
    for pending in &pending_q {
        for (tech_id, pm) in &pending.entries {
            all_entries.push((tech_id, pm));
        }
    }
    if all_entries.is_empty() {
        return;
    }
    // Build a pseudo-pending to iterate below.
    let pending_entries = all_entries;

    for (mut prod, mut rates_opt, mut jobs_opt) in &mut colonies {
        for (tech_id, pm) in &pending_entries {
            let modifier_id = format!("tech:{}:{}", tech_id.0, pm.target);

            // job:<id>::<inner_target> → ColonyJobRates bucket.
            if let Some((job_id, inner_target)) = pm.job_scope() {
                let Some(rates) = rates_opt.as_mut() else {
                    debug!(
                        "Colony lacks ColonyJobRates; skipping tech job mod '{}'",
                        pm.target
                    );
                    continue;
                };
                let bucket = rates.bucket_mut(job_id, inner_target);
                bucket.push_modifier(pm.to_modifier(modifier_id, format!("Tech '{}'", tech_id.0)));
                continue;
            }

            // colony.<job_id>_slot → JobSlot.capacity (additive on top of
            // building-sourced capacity). Only integer slot counts are
            // meaningful; fractional parts are truncated.
            if let Some(slot_rest) = pm
                .target
                .strip_prefix("colony.")
                .and_then(|r| r.strip_suffix("_slot"))
            {
                let Some(jobs) = jobs_opt.as_mut() else {
                    continue;
                };
                let contribution = (pm.base_add + pm.add).max(0.0).floor() as u32;
                if contribution == 0 {
                    continue;
                }
                let job_id = slot_rest.to_string();
                if let Some(slot) = jobs.slots.iter_mut().find(|s| s.job_id == job_id) {
                    // Re-applying: clamp to (building_cap + contribution) to
                    // keep the push idempotent across ticks. The delta is
                    // stored outside `capacity_from_buildings` so building
                    // re-sync doesn't wipe it.
                    let building_cap = slot.capacity_from_buildings;
                    let external_before = slot.capacity.saturating_sub(building_cap);
                    let external_after = external_before.max(contribution);
                    slot.capacity = building_cap.saturating_add(external_after);
                } else {
                    jobs.slots.push(crate::species::JobSlot {
                        job_id,
                        capacity: contribution,
                        assigned: 0,
                        capacity_from_buildings: 0,
                    });
                }
                continue;
            }

            // colony.<X>_per_hexadies → Production aggregator.
            let bucket = match pm.target.as_str() {
                "colony.minerals_per_hexadies" => Some(&mut prod.minerals_per_hexadies),
                "colony.energy_per_hexadies" => Some(&mut prod.energy_per_hexadies),
                "colony.food_per_hexadies" => Some(&mut prod.food_per_hexadies),
                "colony.research_per_hexadies" => Some(&mut prod.research_per_hexadies),
                _ => None,
            };
            if let Some(mv) = bucket {
                mv.push_modifier(pm.to_modifier(modifier_id, format!("Tech '{}'", tech_id.0)));
            }
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
            debug!("Modifier target '{target}' stored in TechEffectsLog (no GlobalParams mapping)");
        }
    }
}

/// Find the on_researched function for a tech by scanning _tech_definitions.
fn find_on_researched(tech_defs: &mlua::Table, tech_id: &str) -> Option<mlua::Function> {
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

    // #332-B4: removed `test_drain_pending_global_mods` /
    // `test_drain_pending_global_mods_empty` / `test_apply_global_mod`
    // — the helpers they exercised (`drain_pending_global_mods`,
    // `apply_global_mod`) are gone along with the `modify_global`
    // global that populated the queue.

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
        let mut balance = GameBalance::default();
        let mut empire_mods = EmpireModifiers::default();
        let mut pending = PendingColonyTechModifiers::default();

        let effect = DescriptiveEffect::SetFlag {
            name: "test_flag".into(),
            value: true,
            description: None,
        };

        let tech_id = TechId("test_tech".into());
        let mut comms = crate::empire::CommsParams::default();
        apply_effect(
            &effect,
            &mut game_flags,
            &mut scoped_flags,
            &mut global_params,
            &mut balance,
            &mut empire_mods,
            &mut pending,
            &mut comms,
            &tech_id,
        );

        assert!(game_flags.check("test_flag"));
        assert!(scoped_flags.check("test_flag"));
    }

    #[test]
    fn test_apply_effect_push_modifier_balance_target() {
        // #160: balance.* targets should route to GameBalance instead of GlobalParams.
        let mut game_flags = GameFlags::default();
        let mut scoped_flags = ScopedFlags::default();
        let mut global_params = GlobalParams::default();
        let mut balance = GameBalance::default();
        let mut empire_mods = EmpireModifiers::default();
        let mut pending = PendingColonyTechModifiers::default();

        let effect = DescriptiveEffect::PushModifier {
            target: "balance.survey_duration".into(),
            base_add: 0.0,
            multiplier: -0.5, // -50%
            add: 0.0,
            description: None,
        };

        let tech_id = TechId("shrink_survey".into());
        let mut comms = crate::empire::CommsParams::default();
        apply_effect(
            &effect,
            &mut game_flags,
            &mut scoped_flags,
            &mut global_params,
            &mut balance,
            &mut empire_mods,
            &mut pending,
            &mut comms,
            &tech_id,
        );

        // survey_duration base = 30, mult = 1.0 + (-0.5) = 0.5 → 15
        assert_eq!(balance.survey_duration(), 15);
    }

    #[test]
    fn test_apply_effect_push_modifier_unknown_balance_target_warns() {
        let mut game_flags = GameFlags::default();
        let mut scoped_flags = ScopedFlags::default();
        let mut global_params = GlobalParams::default();
        let mut balance = GameBalance::default();
        let mut empire_mods = EmpireModifiers::default();
        let mut pending = PendingColonyTechModifiers::default();

        let effect = DescriptiveEffect::PushModifier {
            target: "balance.nonexistent_field".into(),
            base_add: 0.0,
            multiplier: 0.5,
            add: 0.0,
            description: None,
        };

        // Should not panic; logs a warning and leaves balance untouched.
        let tech_id = TechId("buggy_tech".into());
        let mut comms = crate::empire::CommsParams::default();
        apply_effect(
            &effect,
            &mut game_flags,
            &mut scoped_flags,
            &mut global_params,
            &mut balance,
            &mut empire_mods,
            &mut pending,
            &mut comms,
            &tech_id,
        );

        assert_eq!(balance.survey_duration(), 30);
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
    fn preview_captures_scope_effect_without_applying() {
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

        // The callback uses scope methods (which the preview *should*
        // capture). #332-B3 removed the preview drain of the legacy
        // `_pending_*` queues because `EffectScope` is the sole callback
        // path; the `modify_global` / `set_flag` globals are retired in
        // B4.
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

        // The preview captured the scope effect.
        let preview = app.world().resource::<TechEffectsPreview>();
        let effects = preview.for_tech(&TechId("speedy".into()));
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].display_text(), "Speed +0.5");
    }
}
