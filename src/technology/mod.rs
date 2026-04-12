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
