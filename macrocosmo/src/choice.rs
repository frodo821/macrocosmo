//! Player choice dialog system (#152).
//!
//! `PendingChoice` is a resource holding the currently-active modal choice
//! presented to the player. When present, it auto-pauses the game and is
//! rendered as a modal dialog by `draw_choice_dialog_system`. Choices are
//! created from Lua via `show_choice { ... }`; the Rust side drains the
//! queued table, evaluates `condition` and `cost` for each option, and
//! runs the `on_chosen` callback through the `DescriptiveEffect` pipeline
//! (#153) when the player selects an option.
//!
//! Unlike `NotificationQueue`, this is a single-slot resource: the player
//! must dismiss/answer the current choice before the next one is surfaced.
//! Subsequent `show_choice` calls from Lua are queued in `pending_queue`.

use std::collections::HashSet;

use bevy::prelude::*;
use mlua::prelude::*;

use crate::amount::Amt;
use crate::condition::{Condition, EvalContext, ScopeData, ScopedFlags};
use crate::effect::DescriptiveEffect;
use crate::player::PlayerEmpire;
use crate::scripting::condition_parser::parse_condition;
use crate::scripting::effect_scope::{collect_effects, EffectScope};
use crate::scripting::ScriptEngine;
use crate::technology::{GameFlags, GlobalParams, TechTree};
use crate::time_system::GameSpeed;

/// Upfront resource cost required to pick an option. When the player selects
/// an option with a cost, the amounts are subtracted from the player's
/// capital system stockpile before `on_chosen` runs. Options whose cost
/// exceeds available resources are shown greyed-out and non-clickable.
#[derive(Clone, Debug, Default)]
pub struct ChoiceCost {
    pub minerals: Amt,
    pub energy: Amt,
}

impl ChoiceCost {
    pub fn is_zero(&self) -> bool {
        self.minerals == Amt::ZERO && self.energy == Amt::ZERO
    }
}

/// A single option inside a `PendingChoice`. The callback is stored as a Lua
/// reference (key into the `_pending_choices_by_id[choice_id].options[i]`
/// sub-table) to keep the `PendingChoice` resource cheaply cloneable and
/// avoid holding a `mlua::Function` across frames (which needs a live `Lua`).
#[derive(Clone, Debug)]
pub struct ChoiceOption {
    pub label: String,
    pub description: Option<String>,
    pub condition: Option<Condition>,
    pub cost: ChoiceCost,
    /// 1-based index into the Lua-side `options` table for this choice. Used
    /// at apply time to locate the `on_chosen` function.
    pub lua_option_index: usize,
    /// Set to true when the UI computes that `condition` is unsatisfied.
    /// Populated by the dialog just before rendering; the Lua side never
    /// reads this.
    pub condition_unmet: bool,
    /// Set to true when the UI computes that the cost is unaffordable.
    pub cost_unmet: bool,
    /// Human-readable reason string ("requires X", "not enough minerals") for
    /// UI tooltip display. Empty when the option is available.
    pub unmet_reason: String,
}

/// The single currently-active modal choice. `None` means no choice is
/// pending and the dialog is hidden.
#[derive(Resource, Default)]
pub struct PendingChoice {
    pub current: Option<ActiveChoice>,
    /// Additional choices queued behind `current`. Shown one at a time;
    /// answering the current choice promotes the first queued entry.
    pub queue: Vec<ActiveChoice>,
}

/// A choice currently under the player's attention. `lua_id` is the key under
/// which the full Lua table (including per-option `on_chosen` callbacks)
/// lives in the global `_active_choices` registry table.
#[derive(Clone, Debug)]
pub struct ActiveChoice {
    pub lua_id: u64,
    pub title: String,
    pub description: String,
    #[allow(dead_code)]
    pub icon: Option<String>,
    pub target_system: Option<Entity>,
    pub options: Vec<ChoiceOption>,
    /// Effects that the most recent option's `on_chosen` produced, cached so
    /// the UI can display a brief result summary after resolution. Cleared
    /// when the next choice is promoted.
    pub last_effects: Vec<DescriptiveEffect>,
}

impl PendingChoice {
    /// Push a fully-parsed choice onto the pending queue. If no choice is
    /// currently active, it becomes the active one immediately.
    pub fn enqueue(&mut self, choice: ActiveChoice) {
        if self.current.is_none() {
            self.current = Some(choice);
        } else {
            self.queue.push(choice);
        }
    }

    /// Drop the current choice and promote the next queued one (if any).
    /// Returns the lua_id of the choice that was removed, or `None` if
    /// there was no active choice.
    pub fn resolve_current(&mut self) -> Option<u64> {
        let removed = self.current.take()?;
        if !self.queue.is_empty() {
            self.current = Some(self.queue.remove(0));
        }
        Some(removed.lua_id)
    }

    pub fn is_active(&self) -> bool {
        self.current.is_some()
    }
}

/// Outcome of applying an option's `on_chosen` callback. Passed back out of
/// the apply helper so the dialog can display the resulting effects (for UX
/// feedback) and the plugin can decide when to unpause.
#[derive(Default)]
pub struct ChoiceApplyResult {
    pub effects: Vec<DescriptiveEffect>,
}

/// Plugin wiring up the `PendingChoice` resource plus the per-frame drain of
/// Lua-staged choices.
pub struct ChoicesPlugin;

impl Plugin for ChoicesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingChoice>()
            .init_resource::<PendingChoiceSelection>()
            .add_systems(
                Update,
                (drain_pending_choices, apply_pending_choice_selection)
                    .chain()
                    .after(crate::time_system::advance_game_time),
            );
    }
}

/// Drain `_pending_choices` from Lua and enqueue each entry on the
/// `PendingChoice` resource. Also auto-pauses the game while a choice is
/// active. Runs every frame to pick up script-initiated prompts.
pub fn drain_pending_choices(
    engine: Res<ScriptEngine>,
    mut pending: ResMut<PendingChoice>,
    mut speed: ResMut<GameSpeed>,
) {
    let lua = engine.lua();
    let Ok(queue) = lua.globals().get::<mlua::Table>("_pending_choices") else {
        // No queue table yet; nothing to do. Just make sure the pause flag
        // reflects current state.
        maybe_pause_for_active_choice(&pending, &mut speed);
        return;
    };
    let Ok(len) = queue.len() else {
        return;
    };

    if len > 0 {
        for i in 1..=len {
            let Ok(entry) = queue.get::<mlua::Table>(i) else {
                continue;
            };
            match parse_pending_choice(lua, &entry) {
                Ok(active) => pending.enqueue(active),
                Err(e) => warn!("Failed to parse pending choice: {e}"),
            }
        }

        // Reset the pending queue
        if let Ok(new_table) = lua.create_table() {
            let _ = lua.globals().set("_pending_choices", new_table);
        }
    }

    maybe_pause_for_active_choice(&pending, &mut speed);
}

fn maybe_pause_for_active_choice(pending: &PendingChoice, speed: &mut GameSpeed) {
    if pending.is_active() && !speed.is_paused() {
        speed.pause();
    }
}

/// Parse a single Lua `show_choice` entry into an `ActiveChoice`. The full
/// Lua table (including per-option `on_chosen` functions) is stashed in the
/// `_active_choices[lua_id]` registry so the apply step can call back into
/// Lua later without having to re-parse anything.
fn parse_pending_choice(lua: &Lua, entry: &mlua::Table) -> Result<ActiveChoice, mlua::Error> {
    // Allocate an id and stash the raw table so we can find it again later.
    let globals = lua.globals();
    let active = match globals.get::<mlua::Table>("_active_choices") {
        Ok(t) => t,
        Err(_) => {
            let t = lua.create_table()?;
            globals.set("_active_choices", t.clone())?;
            t
        }
    };

    // Simple monotonically-increasing counter stored in the globals.
    let next_id: u64 = globals.get("_active_choices_next_id").unwrap_or(1);
    globals.set("_active_choices_next_id", next_id + 1)?;

    active.set(next_id, entry.clone())?;

    let title: String = entry.get("title").unwrap_or_default();
    let description: String = entry.get("description").unwrap_or_default();
    let icon: Option<String> = entry.get("icon").ok();
    let target_system: Option<Entity> = entry
        .get::<u64>("target_system")
        .ok()
        .map(Entity::from_bits);

    let options_table: mlua::Table = entry.get("options")?;
    let options_len = options_table.len()?;
    let mut options = Vec::with_capacity(options_len as usize);
    for i in 1..=options_len {
        let opt: mlua::Table = options_table.get(i)?;
        options.push(parse_choice_option(&opt, i as usize)?);
    }

    Ok(ActiveChoice {
        lua_id: next_id,
        title,
        description,
        icon,
        target_system,
        options,
        last_effects: Vec::new(),
    })
}

fn parse_choice_option(opt: &mlua::Table, index: usize) -> Result<ChoiceOption, mlua::Error> {
    let label: String = opt.get("label").unwrap_or_default();
    let description: Option<String> = opt.get("description").ok();

    // Condition: optional table in the `condition` field. If missing or not
    // a table, the option is always available.
    let condition: Option<Condition> = match opt.get::<mlua::Value>("condition") {
        Ok(mlua::Value::Table(t)) => Some(parse_condition(&t)?),
        _ => None,
    };

    // Cost: optional { minerals = N, energy = N } table.
    let cost = if let Ok(cost_table) = opt.get::<mlua::Table>("cost") {
        let minerals: u64 = cost_table.get("minerals").unwrap_or(0);
        let energy: u64 = cost_table.get("energy").unwrap_or(0);
        ChoiceCost {
            minerals: Amt::units(minerals),
            energy: Amt::units(energy),
        }
    } else {
        ChoiceCost::default()
    };

    Ok(ChoiceOption {
        label,
        description,
        condition,
        cost,
        lua_option_index: index,
        condition_unmet: false,
        cost_unmet: false,
        unmet_reason: String::new(),
    })
}

/// Evaluate each option's `condition` and `cost` against the current empire
/// state + capital stockpile. Updates the `condition_unmet` / `cost_unmet`
/// flags in-place so the dialog can grey unsatisfied options out.
pub fn evaluate_choice_availability(
    choice: &mut ActiveChoice,
    tech_tree: &TechTree,
    game_flags: &GameFlags,
    scoped_flags: &ScopedFlags,
    capital_stockpile: Option<(Amt, Amt)>,
) {
    let researched_techs: HashSet<String> = tech_tree
        .technologies
        .iter()
        .filter(|(_, t)| tech_tree.is_researched(&t.id))
        .map(|(id, _)| id.0.clone())
        .collect();
    let active_modifiers: HashSet<String> = HashSet::new();
    let empire_flags = &scoped_flags.flags;
    let empire_buildings: HashSet<String> = HashSet::new();

    // Union with game_flags so legacy flag storage still works.
    let mut flags_union: HashSet<String> = empire_flags.clone();
    flags_union.extend(game_flags.flags.iter().cloned());

    let ctx = EvalContext {
        researched_techs: &researched_techs,
        active_modifiers: &active_modifiers,
        empire: Some(ScopeData {
            flags: &flags_union,
            buildings: &empire_buildings,
        }),
        system: None,
        planet: None,
        ship: None,
    };

    let (m_avail, e_avail) = capital_stockpile.unwrap_or((Amt::ZERO, Amt::ZERO));

    for opt in choice.options.iter_mut() {
        opt.condition_unmet = false;
        opt.cost_unmet = false;
        opt.unmet_reason.clear();

        if let Some(cond) = &opt.condition {
            let result = cond.evaluate(&ctx);
            if !result.is_satisfied() {
                opt.condition_unmet = true;
                opt.unmet_reason = "prerequisite not met".to_string();
            }
        }

        if !opt.cost.is_zero() {
            let mut shortage: Vec<String> = Vec::new();
            if opt.cost.minerals > m_avail {
                shortage.push(format!(
                    "need {} minerals (have {})",
                    opt.cost.minerals, m_avail
                ));
            }
            if opt.cost.energy > e_avail {
                shortage.push(format!(
                    "need {} energy (have {})",
                    opt.cost.energy, e_avail
                ));
            }
            if !shortage.is_empty() {
                opt.cost_unmet = true;
                if !opt.unmet_reason.is_empty() {
                    opt.unmet_reason.push_str("; ");
                }
                opt.unmet_reason.push_str(&shortage.join(", "));
            }
        }
    }
}

/// Run the `on_chosen` Lua callback for a given option and collect its
/// `DescriptiveEffect`s. Does NOT apply cost subtraction (the caller handles
/// that) and does NOT apply effects to game state (the caller does that via
/// the existing effect pipeline).
///
/// Returns an empty `Vec` if the option has no `on_chosen` callback or it
/// produced no effects.
pub fn run_on_chosen(
    engine: &ScriptEngine,
    lua_id: u64,
    option_index: usize,
) -> Result<Vec<DescriptiveEffect>, mlua::Error> {
    let lua = engine.lua();
    let active: mlua::Table = lua.globals().get("_active_choices")?;
    let entry: mlua::Table = active.get(lua_id)?;
    let options: mlua::Table = entry.get("options")?;
    let opt: mlua::Table = options.get(option_index)?;

    let func: Option<mlua::Function> = opt.get("on_chosen").ok();
    let Some(func) = func else {
        return Ok(Vec::new());
    };

    let scope = EffectScope::new();
    let ret: mlua::Value = func.call(scope.clone())?;
    collect_effects(&scope, ret)
}

/// Dispose of the Lua-side handle for a choice (called once the option has
/// been applied).
pub fn release_active_choice(engine: &ScriptEngine, lua_id: u64) {
    let lua = engine.lua();
    if let Ok(active) = lua.globals().get::<mlua::Table>("_active_choices") {
        let _ = active.set(lua_id, mlua::Value::Nil);
    }
}

/// Apply a `DescriptiveEffect` returned by `on_chosen` to empire-level
/// state. Mirrors the subset of `technology::effects::apply_effect` that
/// makes sense for player choices (SetFlag, FireEvent, PushModifier to
/// global params, Hidden). PopModifier is a no-op for now.
pub fn apply_choice_effect(
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
            apply_modifier_to_params(global_params, target, *base_add, *multiplier, *add);
        }
        DescriptiveEffect::PopModifier { .. } => {
            debug!("PopModifier from choice option is a no-op");
        }
        DescriptiveEffect::SetFlag { name, value, .. } => {
            if *value {
                game_flags.set(name);
                scoped_flags.set(name);
            }
        }
        DescriptiveEffect::FireEvent { event_id, .. } => {
            // Actual event firing is wired up through the `EventSystem` +
            // `fire_event` Lua call; the `DescriptiveEffect` variant is for
            // UI display. A future patch can integrate these; for now we
            // simply log.
            info!("Choice effect requests event fire: {event_id}");
        }
        DescriptiveEffect::Hidden { inner, .. } => {
            apply_choice_effect(inner, game_flags, scoped_flags, global_params);
        }
    }
}

/// Map a subset of well-known modifier targets to `GlobalParams` fields.
/// Mirrors the mapping in `technology::effects::apply_modifier_to_params`.
fn apply_modifier_to_params(
    params: &mut GlobalParams,
    target: &str,
    base_add: f64,
    multiplier: f64,
    add: f64,
) {
    match target {
        "ship.sublight_speed" => params.sublight_speed_bonus += base_add + add,
        "ship.ftl_range" => params.ftl_range_bonus += base_add + add,
        "sensor.range" => params.survey_range_bonus += base_add + add,
        "ship.ftl_speed" => {
            if multiplier != 0.0 {
                params.ftl_speed_multiplier += multiplier;
            }
            params.sublight_speed_bonus += base_add + add;
        }
        "construction.speed" => {
            if multiplier != 0.0 {
                params.build_speed_multiplier *= 1.0 / (1.0 + multiplier);
            }
        }
        _ => {
            debug!("Choice modifier target '{target}' has no GlobalParams mapping");
        }
    }
}

/// System that finalises an option selection staged from the UI. The UI sets
/// `PendingChoiceSelection` with the chosen option index; this system
/// subtracts cost, runs `on_chosen`, applies the resulting effects, clears
/// the choice, and unpauses the game once no choice remains active.
#[derive(Resource, Default)]
pub struct PendingChoiceSelection {
    /// 1-based option index the player just clicked. Consumed on apply.
    pub pick: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
pub fn apply_pending_choice_selection(
    engine: Res<ScriptEngine>,
    mut pending: ResMut<PendingChoice>,
    mut selection: ResMut<PendingChoiceSelection>,
    mut speed: ResMut<GameSpeed>,
    stars: Query<(Entity, &crate::galaxy::StarSystem)>,
    mut stockpiles: Query<&mut crate::colony::ResourceStockpile, With<crate::galaxy::StarSystem>>,
    mut empire_q: Query<
        (&mut GameFlags, &mut ScopedFlags, &mut GlobalParams),
        With<PlayerEmpire>,
    >,
) {
    let Some(pick) = selection.pick.take() else {
        return;
    };
    let Some(active) = pending.current.as_mut() else {
        return;
    };
    let Ok((mut game_flags, mut scoped_flags, mut global_params)) = empire_q.single_mut()
    else {
        warn!("No PlayerEmpire for choice selection");
        return;
    };

    let Some(option) = active.options.get(pick - 1).cloned() else {
        warn!("Choice selection index out of range: {pick}");
        return;
    };

    if option.condition_unmet || option.cost_unmet {
        warn!(
            "Ignoring unavailable choice option '{}': {}",
            option.label, option.unmet_reason
        );
        return;
    }

    // Subtract cost from the capital stockpile (best-effort — if no capital
    // exists, the cost is skipped with a log).
    if !option.cost.is_zero() {
        let capital_entity = stars
            .iter()
            .find(|(_, s)| s.is_capital)
            .map(|(e, _)| e);
        if let Some(cap) = capital_entity {
            if let Ok(mut stockpile) = stockpiles.get_mut(cap) {
                stockpile.minerals = stockpile.minerals.sub(option.cost.minerals);
                stockpile.energy = stockpile.energy.sub(option.cost.energy);
            } else {
                warn!("Capital system has no ResourceStockpile; cost skipped");
            }
        } else {
            warn!("No capital system found; choice cost skipped");
        }
    }

    // Run on_chosen callback and apply resulting effects.
    let lua_id = active.lua_id;
    let effects = match run_on_chosen(&engine, lua_id, option.lua_option_index) {
        Ok(e) => e,
        Err(e) => {
            warn!("on_chosen for choice option '{}' failed: {e}", option.label);
            Vec::new()
        }
    };
    for effect in &effects {
        apply_choice_effect(effect, &mut game_flags, &mut scoped_flags, &mut global_params);
    }

    // Remember the resulting effects briefly (so the UI can show the summary
    // before the dialog closes). We capture them on the resolved choice so
    // the `last_effects` survive one more frame if needed.
    active.last_effects = effects;

    // Release the Lua handle and pop the choice.
    release_active_choice(&engine, lua_id);
    pending.resolve_current();

    // Unpause only when nothing remains.
    if !pending.is_active() {
        speed.unpause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    fn make_engine_with_choice_api() -> ScriptEngine {
        let engine = ScriptEngine::new().unwrap();
        // Ensure `_pending_choices` table exists. Production code wires this
        // up in `setup_globals`; tests that bypass that path must set it
        // manually. We always rely on `setup_globals` (already called by
        // ScriptEngine::new) to provide it.
        engine
    }

    #[test]
    fn show_choice_push_to_pending_queue() {
        let engine = make_engine_with_choice_api();
        let lua = engine.lua();

        lua.load(
            r#"
            show_choice {
                title = "Ancient Ruins",
                description = "Survey team found ruins.",
                options = {
                    { label = "Study them" },
                    { label = "Leave" },
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let pending: mlua::Table = lua.globals().get("_pending_choices").unwrap();
        assert_eq!(pending.len().unwrap(), 1);
        let entry: mlua::Table = pending.get(1).unwrap();
        assert_eq!(entry.get::<String>("title").unwrap(), "Ancient Ruins");
        let opts: mlua::Table = entry.get("options").unwrap();
        assert_eq!(opts.len().unwrap(), 2);
    }

    #[test]
    fn parse_pending_choice_populates_options() {
        let engine = make_engine_with_choice_api();
        let lua = engine.lua();

        let entry: mlua::Table = lua
            .load(
                r#"
                return {
                    title = "Test",
                    description = "Desc",
                    options = {
                        { label = "A", description = "First" },
                        { label = "B", cost = { minerals = 50 } },
                    },
                }
                "#,
            )
            .eval()
            .unwrap();

        let active = parse_pending_choice(lua, &entry).unwrap();
        assert_eq!(active.title, "Test");
        assert_eq!(active.description, "Desc");
        assert_eq!(active.options.len(), 2);
        assert_eq!(active.options[0].label, "A");
        assert_eq!(active.options[0].description.as_deref(), Some("First"));
        assert_eq!(active.options[1].cost.minerals, Amt::units(50));
    }

    #[test]
    fn evaluate_marks_unaffordable_option() {
        // Build a trivial active choice with a cost of 1000 minerals and
        // available stockpile of 100.
        let mut active = ActiveChoice {
            lua_id: 1,
            title: "T".into(),
            description: "D".into(),
            icon: None,
            target_system: None,
            options: vec![ChoiceOption {
                label: "Pricey".into(),
                description: None,
                condition: None,
                cost: ChoiceCost {
                    minerals: Amt::units(1000),
                    energy: Amt::ZERO,
                },
                lua_option_index: 1,
                condition_unmet: false,
                cost_unmet: false,
                unmet_reason: String::new(),
            }],
            last_effects: Vec::new(),
        };

        let tree = TechTree::default();
        let flags = GameFlags::default();
        let scoped = ScopedFlags::default();
        evaluate_choice_availability(
            &mut active,
            &tree,
            &flags,
            &scoped,
            Some((Amt::units(100), Amt::units(100))),
        );
        assert!(active.options[0].cost_unmet);
        assert!(!active.options[0].condition_unmet);
        assert!(active.options[0].unmet_reason.contains("minerals"));
    }

    #[test]
    fn evaluate_marks_unmet_condition() {
        let engine = make_engine_with_choice_api();
        let lua = engine.lua();

        // Build a condition table via Lua and parse it.
        let cond_table: mlua::Table = lua
            .load(r#"return has_tech("nonexistent_tech")"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&cond_table).unwrap();

        let mut active = ActiveChoice {
            lua_id: 1,
            title: "T".into(),
            description: "D".into(),
            icon: None,
            target_system: None,
            options: vec![ChoiceOption {
                label: "Needs Tech".into(),
                description: None,
                condition: Some(cond),
                cost: ChoiceCost::default(),
                lua_option_index: 1,
                condition_unmet: false,
                cost_unmet: false,
                unmet_reason: String::new(),
            }],
            last_effects: Vec::new(),
        };

        let tree = TechTree::default();
        let flags = GameFlags::default();
        let scoped = ScopedFlags::default();
        evaluate_choice_availability(&mut active, &tree, &flags, &scoped, None);
        assert!(active.options[0].condition_unmet);
    }

    #[test]
    fn drain_runs_and_enqueues_single() {
        let mut app = App::new();
        app.insert_resource(ScriptEngine::new().unwrap());
        app.init_resource::<PendingChoice>();
        app.insert_resource(GameSpeed {
            hexadies_per_second: 1.0,
            previous_speed: 1.0,
        });
        app.add_systems(Update, drain_pending_choices);

        {
            let engine = app.world().resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    show_choice {
                        title = "Choose",
                        description = "",
                        options = { { label = "Yes" }, { label = "No" } },
                    }
                    "#,
                )
                .exec()
                .unwrap();
        }

        app.update();
        let pending = app.world().resource::<PendingChoice>();
        assert!(pending.is_active());
        let active = pending.current.as_ref().unwrap();
        assert_eq!(active.title, "Choose");
        assert_eq!(active.options.len(), 2);

        // Pause should have been applied.
        let speed = app.world().resource::<GameSpeed>();
        assert!(speed.is_paused());
    }

    #[test]
    fn run_on_chosen_collects_effects() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            show_choice {
                title = "T",
                description = "",
                options = {
                    {
                        label = "Apply",
                        on_chosen = function(scope)
                            return {
                                scope:set_flag("ruins_studied", true, { description = "Studied ruins" }),
                            }
                        end,
                    },
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        // Manually run parse_pending_choice to get an lua_id.
        let pending: mlua::Table = lua.globals().get("_pending_choices").unwrap();
        let entry: mlua::Table = pending.get(1).unwrap();
        let active = parse_pending_choice(lua, &entry).unwrap();

        let effects = run_on_chosen(&engine, active.lua_id, 1).unwrap();
        assert_eq!(effects.len(), 1);
        match &effects[0] {
            DescriptiveEffect::SetFlag { name, value, .. } => {
                assert_eq!(name, "ruins_studied");
                assert!(*value);
            }
            _ => panic!("expected SetFlag, got {:?}", effects[0]),
        }
    }

    #[test]
    fn resolve_promotes_next_in_queue() {
        let mut pending = PendingChoice::default();
        pending.enqueue(ActiveChoice {
            lua_id: 1,
            title: "One".into(),
            description: String::new(),
            icon: None,
            target_system: None,
            options: Vec::new(),
            last_effects: Vec::new(),
        });
        pending.enqueue(ActiveChoice {
            lua_id: 2,
            title: "Two".into(),
            description: String::new(),
            icon: None,
            target_system: None,
            options: Vec::new(),
            last_effects: Vec::new(),
        });

        assert_eq!(pending.current.as_ref().unwrap().lua_id, 1);
        assert_eq!(pending.queue.len(), 1);
        let id = pending.resolve_current().unwrap();
        assert_eq!(id, 1);
        assert_eq!(pending.current.as_ref().unwrap().lua_id, 2);
        assert!(pending.queue.is_empty());

        let id2 = pending.resolve_current().unwrap();
        assert_eq!(id2, 2);
        assert!(pending.current.is_none());
        assert!(pending.resolve_current().is_none());
    }
}
