pub mod effects;
mod parsing;
mod research;
mod tree;
pub mod unlocks;

use bevy::prelude::*;
use std::collections::HashSet;

use crate::modifier::ModifiedValue;
use crate::amount::Amt;

// Re-export everything for backward compatibility
pub use effects::{apply_tech_effects, build_tech_effects_preview, TechEffectsLog, TechEffectsPreview};
pub use parsing::{
    create_initial_tech_tree, create_initial_tech_tree_vec, parse_tech_branch_definitions,
    parse_tech_definitions,
};
pub use research::{
    emit_research, flush_research, propagate_tech_knowledge, receive_research,
    receive_tech_knowledge, tick_research, LastResearchTick, PendingKnowledgePropagation,
    PendingResearch, RecentlyResearched, ResearchPool, ResearchQueue, TechKnowledge,
};
pub use tree::{
    default_tech_branches, TechBranchDefinition, TechBranchRegistry, TechCost, TechId, TechTree,
    Technology,
};
pub use unlocks::{build_tech_unlock_index, TechUnlockIndex, UnlockEntry, UnlockKind};

pub struct TechnologyPlugin;

impl Plugin for TechnologyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TechBranchRegistry>()
            .init_resource::<GameBalance>()
            .add_systems(
                Startup,
                load_game_balance.after(crate::scripting::load_all_scripts),
            )
            .add_systems(
                Startup,
                load_tech_branches.after(crate::scripting::load_all_scripts),
            )
            .add_systems(
                Startup,
                load_technologies
                    .after(crate::scripting::load_all_scripts)
                    .after(load_tech_branches)
                    .after(crate::player::spawn_player_empire),
            )
            .add_systems(
                Startup,
                build_tech_unlock_index
                    .after(load_technologies)
                    .after(crate::ship_design::load_ship_designs)
                    .after(crate::colony::load_building_registry)
                    .after(crate::deep_space::load_structure_definitions),
            )
        .insert_resource(LastResearchTick(0))
        .init_resource::<TechEffectsLog>()
        .init_resource::<TechEffectsPreview>()
        .init_resource::<TechUnlockIndex>()
        .add_systems(
            Startup,
            build_tech_effects_preview
                .after(load_technologies)
                .after(crate::scripting::load_all_scripts),
        )
        .add_systems(
            Update,
            (emit_research, receive_research, tick_research, flush_research)
                .chain()
                .after(crate::time_system::advance_game_time),
        )
        .add_systems(
            Update,
            apply_tech_effects
                .after(tick_research)
                .before(propagate_tech_knowledge)
                .after(crate::time_system::advance_game_time),
        )
        .add_systems(
            Update,
            (propagate_tech_knowledge, receive_tech_knowledge)
                .chain()
                .after(apply_tech_effects)
                .after(crate::time_system::advance_game_time),
        )
        // #160: Keep AuthorityParams' base values in sync with GameBalance.
        // Runs after apply_tech_effects (which may alter GameBalance) and
        // before tick_timed_effects / tick_authority.
        .add_systems(
            Update,
            sync_authority_params_from_balance
                .after(apply_tech_effects)
                .before(crate::colony::tick_timed_effects)
                .after(crate::time_system::advance_game_time),
        );
    }
}

/// Global parameters modified by researched technologies.
/// Contains ship/movement-related bonuses. Production and population bonuses
/// have been moved to the modifier system (EmpireModifiers).
#[derive(Resource, Component, Debug, Clone)]
pub struct GlobalParams {
    /// Added to base sublight speed
    pub sublight_speed_bonus: f64,
    /// Multiplied with base FTL speed
    pub ftl_speed_multiplier: f64,
    /// Added to base FTL range
    pub ftl_range_bonus: f64,
    /// Added to base survey range
    pub survey_range_bonus: f64,
    /// Multiplied with build time (lower = faster)
    pub build_speed_multiplier: f64,
}

impl Default for GlobalParams {
    fn default() -> Self {
        Self {
            sublight_speed_bonus: 0.0,
            ftl_speed_multiplier: 1.0,
            ftl_range_bonus: 0.0,
            survey_range_bonus: 0.0,
            build_speed_multiplier: 1.0,
        }
    }
}

/// Empire-wide modifiers applied via the modifier system.
/// Replaces the production/population fields that were in GlobalParams.
#[derive(Resource, Component)]
pub struct EmpireModifiers {
    pub population_growth: ModifiedValue,
}

impl Default for EmpireModifiers {
    fn default() -> Self {
        Self {
            population_growth: ModifiedValue::new(Amt::ZERO),
        }
    }
}

/// #160: Scriptable balance constants. Every field is a `ModifiedValue` so that
/// technologies, events, modules, etc. can push multiplier/add modifiers at
/// runtime (target strings: `"balance.<field_name>"`).
///
/// Base values are populated from `scripts/config/balance.lua` at startup (see
/// `load_game_balance`). If no balance definition is found, the hardcoded
/// defaults below apply — these match the legacy `pub const` values they
/// replaced so existing behaviour is preserved.
///
/// Convention for scale:
/// - *_DURATION / *_TIME / *_HEXADIES fields store hexadies as whole units
///   (`Amt::units(n)`). Consumers call `.to_i64()` via `final_value().whole()`.
/// - `*_SPEED_C`, `*_RANGE_LY`, `*_FACTOR`, `*_RATE_PER_HEXADIES` store
///   decimal values via fixed-point (`Amt::from_f64(v)`). Consumers call
///   `.to_f64()` via `final_value().to_f64()`.
/// - `COLONIZATION_*_COST` stores whole units (`Amt::units(300)`).
/// - `BASE_AUTHORITY_PER_HEXADIES` / `AUTHORITY_COST_PER_COLONY` are kept in
///   sync with `AuthorityParams` via `sync_authority_params_from_balance`.
#[derive(Resource, Component, Debug, Clone)]
pub struct GameBalance {
    /// Initial FTL speed as multiple of light speed. Base = 10.0
    pub initial_ftl_speed_c: ModifiedValue,
    /// Survey operation duration in hexadies. Base = 30
    pub survey_duration: ModifiedValue,
    /// Settling (colonization-by-ship) duration in hexadies. Base = 60
    pub settling_duration: ModifiedValue,
    /// Maximum distance in light-years to initiate a survey. Base = 5.0
    pub survey_range_ly: ModifiedValue,
    /// Additional FTL range (LY) granted by a Port facility. Base = 10.0
    pub port_ftl_range_bonus: ModifiedValue,
    /// Multiplier applied to FTL travel time when departing from a Port.
    /// Base = 0.8 (20% faster)
    pub port_travel_time_factor: ModifiedValue,
    /// Ship hull/armor repair rate at a Port, per hexady. Base = 5.0
    pub repair_rate_per_hexadies: ModifiedValue,
    /// Mineral cost of a same-system colonization order. Base = 300
    pub colonization_mineral_cost: ModifiedValue,
    /// Energy cost of a same-system colonization order. Base = 200
    pub colonization_energy_cost: ModifiedValue,
    /// Build time (hexadies) of a same-system colonization order. Base = 90
    pub colonization_build_time: ModifiedValue,
    /// Authority produced per hexady by the capital colony. Base = 1.0
    pub base_authority_per_hexadies: ModifiedValue,
    /// Authority cost per hexady per non-capital colony. Base = 0.5
    pub authority_cost_per_colony: ModifiedValue,
}

impl Default for GameBalance {
    fn default() -> Self {
        Self {
            initial_ftl_speed_c: ModifiedValue::new(Amt::from_f64(10.0)),
            survey_duration: ModifiedValue::new(Amt::units(30)),
            settling_duration: ModifiedValue::new(Amt::units(60)),
            survey_range_ly: ModifiedValue::new(Amt::from_f64(5.0)),
            port_ftl_range_bonus: ModifiedValue::new(Amt::from_f64(10.0)),
            port_travel_time_factor: ModifiedValue::new(Amt::from_f64(0.8)),
            repair_rate_per_hexadies: ModifiedValue::new(Amt::from_f64(5.0)),
            colonization_mineral_cost: ModifiedValue::new(Amt::units(300)),
            colonization_energy_cost: ModifiedValue::new(Amt::units(200)),
            colonization_build_time: ModifiedValue::new(Amt::units(90)),
            base_authority_per_hexadies: ModifiedValue::new(Amt::units(1)),
            authority_cost_per_colony: ModifiedValue::new(Amt::new(0, 500)),
        }
    }
}

impl GameBalance {
    pub fn initial_ftl_speed_c(&self) -> f64 {
        self.initial_ftl_speed_c.final_value().to_f64()
    }
    pub fn survey_duration(&self) -> i64 {
        self.survey_duration.final_value().to_f64().round() as i64
    }
    pub fn settling_duration(&self) -> i64 {
        self.settling_duration.final_value().to_f64().round() as i64
    }
    pub fn survey_range_ly(&self) -> f64 {
        self.survey_range_ly.final_value().to_f64()
    }
    pub fn port_ftl_range_bonus(&self) -> f64 {
        self.port_ftl_range_bonus.final_value().to_f64()
    }
    pub fn port_travel_time_factor(&self) -> f64 {
        self.port_travel_time_factor.final_value().to_f64()
    }
    pub fn repair_rate_per_hexadies(&self) -> f64 {
        self.repair_rate_per_hexadies.final_value().to_f64()
    }
    pub fn colonization_mineral_cost(&self) -> Amt {
        self.colonization_mineral_cost.final_value()
    }
    pub fn colonization_energy_cost(&self) -> Amt {
        self.colonization_energy_cost.final_value()
    }
    pub fn colonization_build_time(&self) -> i64 {
        self.colonization_build_time.final_value().to_f64().round() as i64
    }
    pub fn base_authority_per_hexadies(&self) -> Amt {
        self.base_authority_per_hexadies.final_value()
    }
    pub fn authority_cost_per_colony(&self) -> Amt {
        self.authority_cost_per_colony.final_value()
    }

    /// Look up a balance field's `ModifiedValue` by string target name (for
    /// use by the modifier pipeline). Returns `None` if the target is not a
    /// recognised balance field. The target is the part after the
    /// `"balance."` prefix (stripped by the caller).
    pub fn field_mut(&mut self, name: &str) -> Option<&mut ModifiedValue> {
        match name {
            "initial_ftl_speed_c" => Some(&mut self.initial_ftl_speed_c),
            "survey_duration" => Some(&mut self.survey_duration),
            "settling_duration" => Some(&mut self.settling_duration),
            "survey_range_ly" => Some(&mut self.survey_range_ly),
            "port_ftl_range_bonus" => Some(&mut self.port_ftl_range_bonus),
            "port_travel_time_factor" => Some(&mut self.port_travel_time_factor),
            "repair_rate_per_hexadies" => Some(&mut self.repair_rate_per_hexadies),
            "colonization_mineral_cost" => Some(&mut self.colonization_mineral_cost),
            "colonization_energy_cost" => Some(&mut self.colonization_energy_cost),
            "colonization_build_time" => Some(&mut self.colonization_build_time),
            "base_authority_per_hexadies" => Some(&mut self.base_authority_per_hexadies),
            "authority_cost_per_colony" => Some(&mut self.authority_cost_per_colony),
            _ => None,
        }
    }
}

/// Tracks boolean flags set by technology effects (e.g. unlocked buildings).
#[derive(Resource, Component, Default, Debug, Clone)]
pub struct GameFlags {
    pub flags: HashSet<String>,
}

impl GameFlags {
    pub fn set(&mut self, flag: &str) {
        self.flags.insert(flag.to_string());
    }

    pub fn check(&self, flag: &str) -> bool {
        self.flags.contains(flag)
    }
}

/// Parse tech branch definitions from Lua and populate the TechBranchRegistry.
/// Falls back to `default_tech_branches()` when no scripts define any branches
/// (e.g. minimal test setups). Runs before `load_technologies` so that tech
/// definitions can be validated against the registry.
pub fn load_tech_branches(
    engine: Res<crate::scripting::ScriptEngine>,
    mut registry: ResMut<TechBranchRegistry>,
) {
    let branches = match parse_tech_branch_definitions(engine.lua()) {
        Ok(parsed) if !parsed.is_empty() => parsed,
        Ok(_) => {
            info!("No tech branch definitions found in scripts; using defaults");
            default_tech_branches()
        }
        Err(e) => {
            warn!("Failed to parse tech branch definitions: {e}; using defaults");
            default_tech_branches()
        }
    };

    let count = branches.len();
    for def in branches {
        registry.insert(def);
    }
    info!("Tech branch registry loaded with {} branches", count);
}

/// Parse technology definitions from Lua accumulators.
/// Scripts are loaded by `load_all_scripts`; this system only parses the results.
/// Falls back to hardcoded definitions if parsing fails or yields nothing.
///
/// Validates each tech's `branch` against the loaded `TechBranchRegistry` and
/// emits a warning (not an error) for unknown branches — the tech is still
/// registered to keep the game playable in the face of script typos.
pub fn load_technologies(
    mut commands: Commands,
    engine: Res<crate::scripting::ScriptEngine>,
    branch_registry: Res<TechBranchRegistry>,
    empire_q: Query<Entity, With<crate::player::PlayerEmpire>>,
) {
    let techs = match parse_tech_definitions(engine.lua()) {
        Ok(parsed) if !parsed.is_empty() => parsed,
        Ok(_) => {
            info!("No tech definitions found in scripts; using hardcoded fallback");
            create_initial_tech_tree_vec()
        }
        Err(e) => {
            warn!("Failed to parse tech definitions: {e}; falling back to hardcoded definitions");
            create_initial_tech_tree_vec()
        }
    };

    // Validate branches against registry — warn (don't fail) on unknown branches.
    if !branch_registry.is_empty() {
        for tech in &techs {
            if branch_registry.get(&tech.branch).is_none() {
                warn!(
                    "Tech '{}' references unknown branch '{}'",
                    tech.id.0, tech.branch
                );
            }
        }
    }

    let tree = TechTree::from_vec(techs);
    info!(
        "Tech tree loaded with {} technologies",
        tree.technologies.len()
    );

    // Insert onto the player empire entity (replacing the default)
    if let Ok(empire_entity) = empire_q.single() {
        commands.entity(empire_entity).insert(tree);
    } else {
        warn!("No player empire entity found; inserting TechTree as resource fallback");
        commands.insert_resource(tree);
    }
}

/// #160: Load `GameBalance` baseline values from the `_balance_definition`
/// Lua global (populated by `define_balance { ... }` in
/// `scripts/config/balance.lua`). Each field listed in the Lua table
/// overrides the corresponding `ModifiedValue`'s *base* value; missing
/// fields keep their default. Calling `define_balance` more than once
/// results in last-wins semantics with a warning logged.
pub fn load_game_balance(
    engine: Res<crate::scripting::ScriptEngine>,
    mut balance: ResMut<GameBalance>,
) {
    let lua = engine.lua();
    let Ok(value): Result<mlua::Value, _> = lua.globals().get("_balance_definition") else {
        return;
    };
    let table = match value {
        mlua::Value::Table(t) => t,
        mlua::Value::Nil => return,
        other => {
            warn!(
                "_balance_definition is not a table: {:?}; using defaults",
                other
            );
            return;
        }
    };

    // Helper closures that set the base value of a ModifiedValue from a Lua
    // number. `as_units` controls whether the f64 is treated as whole units
    // (for integer hexadies / cost fields) or as a decimal via `from_f64`.
    fn set_integer(mv: &mut ModifiedValue, t: &mlua::Table, key: &str) {
        if let Ok(v) = t.get::<f64>(key) {
            mv.set_base(Amt::units(v.round() as u64));
        }
    }
    fn set_decimal(mv: &mut ModifiedValue, t: &mlua::Table, key: &str) {
        if let Ok(v) = t.get::<f64>(key) {
            mv.set_base(Amt::from_f64(v));
        }
    }

    set_decimal(&mut balance.initial_ftl_speed_c, &table, "initial_ftl_speed_c");
    set_integer(&mut balance.survey_duration, &table, "survey_duration");
    set_integer(&mut balance.settling_duration, &table, "settling_duration");
    set_decimal(&mut balance.survey_range_ly, &table, "survey_range_ly");
    set_decimal(&mut balance.port_ftl_range_bonus, &table, "port_ftl_range_bonus");
    set_decimal(&mut balance.port_travel_time_factor, &table, "port_travel_time_factor");
    set_decimal(&mut balance.repair_rate_per_hexadies, &table, "repair_rate_per_hexadies");
    set_integer(&mut balance.colonization_mineral_cost, &table, "colonization_mineral_cost");
    set_integer(&mut balance.colonization_energy_cost, &table, "colonization_energy_cost");
    set_integer(&mut balance.colonization_build_time, &table, "colonization_build_time");
    set_decimal(&mut balance.base_authority_per_hexadies, &table, "base_authority_per_hexadies");
    set_decimal(&mut balance.authority_cost_per_colony, &table, "authority_cost_per_colony");

    info!("GameBalance loaded from Lua");
}

/// #160: Apply `balance.<field>` modifier effects produced by tech
/// `on_researched` callbacks. Scans `_pending_balance_mods` (populated by
/// the `DescriptiveEffect` apply pipeline) and pushes the corresponding
/// `Modifier` onto the right `ModifiedValue`.
///
/// Also synchronises the legacy `AuthorityParams` resource's base values
/// from `GameBalance` so that edits to the balance resource flow through to
/// authority calculations even while the existing `AuthorityParams` modifier
/// stack remains intact.
pub fn sync_authority_params_from_balance(
    balance: Res<GameBalance>,
    mut empire_q: Query<
        &mut crate::colony::AuthorityParams,
        With<crate::player::PlayerEmpire>,
    >,
) {
    if !balance.is_changed() {
        return;
    }
    let Ok(mut params) = empire_q.single_mut() else {
        return;
    };
    let prod_base = balance.base_authority_per_hexadies.effective_base();
    let cost_base = balance.authority_cost_per_colony.effective_base();
    if params.production.base() != prod_base {
        params.production.set_base(prod_base);
    }
    if params.cost_per_colony.base() != cost_base {
        params.cost_per_colony.set_base(cost_base);
    }
}

// =========================================================================
// #160: GameBalance tests
// =========================================================================

#[cfg(test)]
mod game_balance_tests {
    use super::*;
    use crate::modifier::Modifier;
    use crate::amount::SignedAmt;

    fn make_mult_modifier(id: &str, mult: SignedAmt) -> Modifier {
        Modifier {
            id: id.to_string(),
            label: id.to_string(),
            base_add: SignedAmt::ZERO,
            multiplier: mult,
            add: SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        }
    }

    /// Default construction mirrors the legacy hardcoded constants so that
    /// *without* `define_balance`, behaviour is identical to pre-#160.
    #[test]
    fn default_matches_legacy_constants() {
        let b = GameBalance::default();
        assert!((b.initial_ftl_speed_c() - 10.0).abs() < 1e-6);
        assert_eq!(b.survey_duration(), 30);
        assert_eq!(b.settling_duration(), 60);
        assert!((b.survey_range_ly() - 5.0).abs() < 1e-6);
        assert!((b.port_ftl_range_bonus() - 10.0).abs() < 1e-6);
        assert!((b.port_travel_time_factor() - 0.8).abs() < 1e-6);
        assert!((b.repair_rate_per_hexadies() - 5.0).abs() < 1e-6);
        assert_eq!(b.colonization_mineral_cost(), Amt::units(300));
        assert_eq!(b.colonization_energy_cost(), Amt::units(200));
        assert_eq!(b.colonization_build_time(), 90);
        assert_eq!(b.base_authority_per_hexadies(), Amt::units(1));
        assert_eq!(b.authority_cost_per_colony(), Amt::new(0, 500));
    }

    /// `field_mut` returns a reference for every documented target, and
    /// `None` for unknown targets — this is the primary safety contract for
    /// the `balance.*` modifier routing in `apply_effect`.
    #[test]
    fn field_mut_covers_all_fields() {
        let mut b = GameBalance::default();
        let fields = [
            "initial_ftl_speed_c",
            "survey_duration",
            "settling_duration",
            "survey_range_ly",
            "port_ftl_range_bonus",
            "port_travel_time_factor",
            "repair_rate_per_hexadies",
            "colonization_mineral_cost",
            "colonization_energy_cost",
            "colonization_build_time",
            "base_authority_per_hexadies",
            "authority_cost_per_colony",
        ];
        for f in fields {
            assert!(
                b.field_mut(f).is_some(),
                "field_mut should know '{f}'"
            );
        }
        assert!(b.field_mut("does_not_exist").is_none());
    }

    /// Pushing a multiplier modifier flows through to the accessor result.
    #[test]
    fn push_multiplier_modifier_changes_effective_value() {
        let mut b = GameBalance::default();
        // -50% on survey_duration → 30 * 0.5 = 15
        let mv = b.field_mut("survey_duration").unwrap();
        mv.push_modifier(make_mult_modifier("tech:x", SignedAmt::new(0, -500)));
        assert_eq!(b.survey_duration(), 15);

        // +50% on repair_rate_per_hexadies → 5.0 * 1.5 = 7.5
        let mv = b.field_mut("repair_rate_per_hexadies").unwrap();
        mv.push_modifier(make_mult_modifier("tech:y", SignedAmt::new(0, 500)));
        assert!((b.repair_rate_per_hexadies() - 7.5).abs() < 1e-6);
    }

    /// Without any modifier, accessors return the baseline. Verifies the
    /// "modifier未適用時は baseline 値" acceptance criterion from #160.
    #[test]
    fn unmodified_accessors_return_baseline() {
        let b = GameBalance::default();
        assert_eq!(b.survey_duration(), 30);
        assert_eq!(b.colonization_build_time(), 90);
        assert!((b.initial_ftl_speed_c() - 10.0).abs() < 1e-6);
    }

    /// Lua `define_balance { ... }` populates the `_balance_definition`
    /// global; `load_game_balance` reads it and updates the resource bases.
    #[test]
    fn load_game_balance_reads_lua_definition() {
        let engine = crate::scripting::ScriptEngine::new().unwrap();
        engine
            .lua()
            .load(
                r#"
                define_balance {
                    initial_ftl_speed_c      = 12.5,
                    survey_duration          = 25,
                    settling_duration        = 50,
                    survey_range_ly          = 7.0,
                    port_ftl_range_bonus     = 15.0,
                    port_travel_time_factor  = 0.7,
                    repair_rate_per_hexadies = 4.5,
                    colonization_mineral_cost   = 250,
                    colonization_energy_cost    = 150,
                    colonization_build_time     = 80,
                    base_authority_per_hexadies = 2.0,
                    authority_cost_per_colony   = 0.25,
                }
                "#,
            )
            .exec()
            .unwrap();

        let mut app = App::new();
        app.insert_resource(engine);
        app.init_resource::<GameBalance>();
        app.add_systems(Update, load_game_balance);
        app.update();

        let b = app.world().resource::<GameBalance>();
        assert!((b.initial_ftl_speed_c() - 12.5).abs() < 1e-6);
        assert_eq!(b.survey_duration(), 25);
        assert_eq!(b.settling_duration(), 50);
        assert!((b.survey_range_ly() - 7.0).abs() < 1e-6);
        assert!((b.port_ftl_range_bonus() - 15.0).abs() < 1e-6);
        assert!((b.port_travel_time_factor() - 0.7).abs() < 1e-6);
        assert!((b.repair_rate_per_hexadies() - 4.5).abs() < 1e-6);
        assert_eq!(b.colonization_mineral_cost(), Amt::units(250));
        assert_eq!(b.colonization_energy_cost(), Amt::units(150));
        assert_eq!(b.colonization_build_time(), 80);
        assert_eq!(b.base_authority_per_hexadies(), Amt::units(2));
        assert_eq!(b.authority_cost_per_colony(), Amt::new(0, 250));
    }

    /// Missing Lua definition → defaults intact (legacy behaviour).
    #[test]
    fn load_game_balance_without_lua_definition_keeps_defaults() {
        let engine = crate::scripting::ScriptEngine::new().unwrap();
        // Do NOT call define_balance.

        let mut app = App::new();
        app.insert_resource(engine);
        app.init_resource::<GameBalance>();
        app.add_systems(Update, load_game_balance);
        app.update();

        let b = app.world().resource::<GameBalance>();
        // Defaults intact.
        assert_eq!(b.survey_duration(), 30);
        assert_eq!(b.settling_duration(), 60);
    }

    /// Partial Lua definition overrides only the listed fields; others stay
    /// at their default.
    #[test]
    fn load_game_balance_partial_definition() {
        let engine = crate::scripting::ScriptEngine::new().unwrap();
        engine
            .lua()
            .load(r#"define_balance { survey_duration = 20 }"#)
            .exec()
            .unwrap();

        let mut app = App::new();
        app.insert_resource(engine);
        app.init_resource::<GameBalance>();
        app.add_systems(Update, load_game_balance);
        app.update();

        let b = app.world().resource::<GameBalance>();
        assert_eq!(b.survey_duration(), 20); // overridden
        assert_eq!(b.settling_duration(), 60); // default
        assert!((b.initial_ftl_speed_c() - 10.0).abs() < 1e-6); // default
    }

    /// Second `define_balance` call wins (last-wins semantics per #160 spec).
    #[test]
    fn define_balance_last_call_wins() {
        let engine = crate::scripting::ScriptEngine::new().unwrap();
        engine
            .lua()
            .load(
                r#"
                define_balance { survey_duration = 10 }
                define_balance { survey_duration = 40 }
                "#,
            )
            .exec()
            .unwrap();

        let mut app = App::new();
        app.insert_resource(engine);
        app.init_resource::<GameBalance>();
        app.add_systems(Update, load_game_balance);
        app.update();

        let b = app.world().resource::<GameBalance>();
        assert_eq!(b.survey_duration(), 40);
    }
}
