use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::amount::Amt;
use crate::colony::{Colony, Production, ProductionFocus};
use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::modifier::ModifiedValue;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::time_system::GameClock;

pub struct TechnologyPlugin;

impl Plugin for TechnologyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            load_technologies.after(crate::scripting::init_scripting),
        )
        .insert_resource(ResearchQueue::default())
        .insert_resource(ResearchPool::default())
        .insert_resource(LastResearchTick(0))
        .insert_resource(GlobalParams::default())
        .insert_resource(GameFlags::default())
        .insert_resource(EmpireModifiers::default())
        .insert_resource(RecentlyResearched::default())
        .add_systems(
            Update,
            (emit_research, receive_research, tick_research, flush_research)
                .chain()
                .after(crate::time_system::advance_game_time),
        )
        .add_systems(
            Update,
            (propagate_tech_knowledge, receive_tech_knowledge)
                .chain()
                .after(tick_research)
                .after(crate::time_system::advance_game_time),
        );
    }
}

/// Global parameters modified by researched technologies.
/// Contains ship/movement-related bonuses. Production and population bonuses
/// have been moved to the modifier system (EmpireModifiers).
#[derive(Resource, Debug, Clone)]
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
#[derive(Resource)]
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
#[derive(Resource, Default, Debug, Clone)]
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

pub fn load_technologies(mut commands: Commands, engine: Res<crate::scripting::ScriptEngine>) {
    let tech_dir = Path::new("scripts/tech");
    let techs = if tech_dir.exists() {
        match engine.load_directory(tech_dir) {
            Err(e) => {
                warn!("Failed to load tech scripts: {e}; falling back to hardcoded definitions");
                create_initial_tech_tree_vec()
            }
            Ok(()) => match parse_tech_definitions(engine.lua()) {
                Ok(parsed) if !parsed.is_empty() => parsed,
                Ok(_) => {
                    info!("No tech definitions found in scripts; using hardcoded fallback");
                    create_initial_tech_tree_vec()
                }
                Err(e) => {
                    warn!("Failed to parse tech definitions: {e}; falling back to hardcoded definitions");
                    create_initial_tech_tree_vec()
                }
            },
        }
    } else {
        info!("scripts/tech directory not found; using hardcoded tech definitions");
        create_initial_tech_tree_vec()
    };

    let tree = TechTree::from_vec(techs);
    info!(
        "Tech tree loaded with {} technologies",
        tree.technologies.len()
    );
    commands.insert_resource(tree);
}

/// Unique identifier for a technology.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TechId(pub u32);

/// The branch a technology belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TechBranch {
    Social,
    Physics,
    Industrial,
    Military,
}

impl TechBranch {
    pub fn all() -> &'static [TechBranch] {
        &[
            TechBranch::Social,
            TechBranch::Physics,
            TechBranch::Industrial,
            TechBranch::Military,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            TechBranch::Social => "Social",
            TechBranch::Physics => "Physics",
            TechBranch::Industrial => "Industrial",
            TechBranch::Military => "Military",
        }
    }
}

/// Upfront resource cost to begin researching a technology.
/// Research points (flow) are tracked separately via `cost_research`.
#[derive(Debug, Clone, Default)]
pub struct TechCost {
    /// Research points needed to complete (flow cost).
    pub research: Amt,
    /// Minerals consumed upfront when research starts.
    pub minerals: Amt,
    /// Energy consumed upfront when research starts.
    pub energy: Amt,
}

impl TechCost {
    /// Create a research-only cost (no upfront resource cost).
    pub const fn research_only(research: Amt) -> Self {
        Self {
            research,
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
        }
    }
}

/// A single technology definition.
#[derive(Debug, Clone)]
pub struct Technology {
    pub id: TechId,
    pub name: String,
    pub description: String,
    pub branch: TechBranch,
    pub cost: TechCost,
    pub prerequisites: Vec<TechId>,
}

/// The complete technology tree, indexed by TechId.
#[derive(Resource, Debug, Clone, Default)]
pub struct TechTree {
    pub technologies: HashMap<TechId, Technology>,
    pub researched: HashSet<TechId>,
}

impl TechTree {
    pub fn from_vec(techs: Vec<Technology>) -> Self {
        let technologies = techs.into_iter().map(|t| (t.id, t)).collect();
        Self {
            technologies,
            researched: HashSet::new(),
        }
    }

    /// Insert a technology into the tree.
    pub fn add(&mut self, tech: Technology) {
        self.technologies.insert(tech.id, tech);
    }

    /// Get a technology by its id.
    pub fn get(&self, id: TechId) -> Option<&Technology> {
        self.technologies.get(&id)
    }

    pub fn is_researched(&self, id: TechId) -> bool {
        self.researched.contains(&id)
    }

    pub fn can_research(&self, id: TechId) -> bool {
        if self.researched.contains(&id) {
            return false;
        }
        let Some(tech) = self.technologies.get(&id) else {
            return false;
        };
        tech.prerequisites
            .iter()
            .all(|pre| self.researched.contains(pre))
    }

    /// Alias used by the research panel UI.
    pub fn is_available(&self, id: TechId) -> bool {
        self.can_research(id)
    }

    pub fn available_technologies(&self) -> Vec<&Technology> {
        self.technologies
            .values()
            .filter(|t| self.can_research(t.id))
            .collect()
    }

    pub fn complete_research(&mut self, id: TechId) {
        self.researched.insert(id);
    }

    /// Return all technologies in a given branch.
    pub fn branch(&self, branch: TechBranch) -> Vec<&Technology> {
        self.technologies
            .values()
            .filter(|t| t.branch == branch)
            .collect()
    }

    /// Get all technologies for a branch, sorted by cost.
    pub fn techs_in_branch(&self, branch: TechBranch) -> Vec<&Technology> {
        let mut techs: Vec<&Technology> = self
            .technologies
            .values()
            .filter(|t| t.branch == branch)
            .collect();
        techs.sort_by(|a, b| a.cost.research.cmp(&b.cost.research));
        techs
    }

    /// Check that every prerequisite referenced in the tree actually exists.
    pub fn validate_prerequisites(&self) -> Result<(), Vec<(TechId, TechId)>> {
        let mut missing = Vec::new();
        for tech in self.technologies.values() {
            for prereq in &tech.prerequisites {
                if !self.technologies.contains_key(prereq) {
                    missing.push((tech.id, *prereq));
                }
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing)
        }
    }
}

/// Current research target and accumulated points.
#[derive(Resource, Default)]
pub struct ResearchQueue {
    pub current: Option<TechId>,
    pub accumulated: f64,
    pub blocked: bool,
}

impl ResearchQueue {
    pub fn start_research(&mut self, tech_id: TechId) {
        self.current = Some(tech_id);
        self.accumulated = 0.0;
        self.blocked = false;
    }

    pub fn cancel_research(&mut self) {
        self.current = None;
        self.accumulated = 0.0;
    }

    pub fn block(&mut self) {
        self.blocked = true;
    }

    pub fn unblock(&mut self) {
        self.blocked = false;
    }

    pub fn add_progress(&mut self, amount: f64) {
        self.accumulated += amount;
    }
}

/// Global research points pool (accumulated from colonies).
#[derive(Resource, Default)]
pub struct ResearchPool {
    pub points: f64,
}

/// Tracks the last game tick at which research was collected, to compute delta.
#[derive(Resource)]
pub struct LastResearchTick(pub i64);

/// A research packet in transit from a colony to the capital at light speed.
#[derive(Component)]
pub struct PendingResearch {
    pub amount: f64,
    pub arrives_at: i64,
}

/// Each tick, colonies emit research points as PendingResearch entities that
/// travel at light speed to the capital. Capital colonies contribute instantly.
pub fn emit_research(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastResearchTick>,
    colonies: Query<(&Colony, &Production, Option<&ProductionFocus>)>,
    player_q: Query<&StationedAt, With<Player>>,
    positions: Query<&Position>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as f64;

    // Find capital system position
    let capital_system = player_q.single().ok().map(|s| s.system);
    let capital_pos = capital_system.and_then(|sys| positions.get(sys).ok());

    for (colony, prod, focus) in &colonies {
        let rw = match focus {
            Some(f) => f.research_weight,
            None => Amt::units(1),
        };
        let d_amt = Amt::units(d as u64);
        // Building bonuses are already included via modifiers on Production
        let amount = prod.research_per_hexadies.final_value().mul_amt(rw).mul_amt(d_amt).to_f64();
        if amount <= 0.0 {
            continue;
        }

        // Calculate light delay from colony to capital
        let delay = match (capital_system, capital_pos) {
            (Some(cap_sys), Some(_)) if colony.system == cap_sys => 0,
            (Some(_), Some(cap_pos)) => {
                if let Ok(colony_pos) = positions.get(colony.system) {
                    let dist = physics::distance_ly(colony_pos, cap_pos);
                    physics::light_delay_hexadies(dist)
                } else {
                    0
                }
            }
            _ => 0,
        };

        commands.spawn(PendingResearch {
            amount,
            arrives_at: clock.elapsed + delay,
        });
    }
}

/// Receives PendingResearch entities that have arrived and adds them to the pool.
pub fn receive_research(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut pool: ResMut<ResearchPool>,
    pending: Query<(Entity, &PendingResearch)>,
) {
    for (entity, pr) in &pending {
        if clock.elapsed >= pr.arrives_at {
            pool.points += pr.amount;
            commands.entity(entity).despawn();
        }
    }
}

/// Processes research each tick: transfers points from pool to current project.
/// When research completes, the tech is marked as researched. The on_researched
/// Lua callback will be invoked separately by the scripting system.
pub fn tick_research(
    clock: Res<GameClock>,
    mut last_tick: ResMut<LastResearchTick>,
    mut tech_tree: ResMut<TechTree>,
    mut queue: ResMut<ResearchQueue>,
    mut pool: ResMut<ResearchPool>,
    mut recently_researched: ResMut<RecentlyResearched>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    last_tick.0 = clock.elapsed;

    let Some(current_tech_id) = queue.current else {
        return;
    };

    // Skip progress if research is blocked
    if queue.blocked {
        return;
    }

    let research_cost = {
        let Some(tech) = tech_tree.technologies.get(&current_tech_id) else {
            queue.current = None;
            return;
        };
        tech.cost.research.to_f64()
    };

    // Transfer available research points from pool
    let needed = research_cost - queue.accumulated;
    if needed > 0.0 {
        let transfer = pool.points.min(needed);
        if transfer > 0.0 {
            pool.points -= transfer;
            queue.accumulated += transfer;
        }
    }

    // Check completion
    if queue.accumulated >= research_cost {
        let tech_name = tech_tree
            .technologies
            .get(&current_tech_id)
            .map(|t| t.name.clone())
            .unwrap_or_default();

        tech_tree.complete_research(current_tech_id);
        recently_researched.techs.push(current_tech_id);

        queue.current = None;
        queue.accumulated = 0.0;
        info!("Research complete: {}", tech_name);
    }
}

/// Flush unused research points at the end of each tick (use it or lose it).
pub fn flush_research(mut pool: ResMut<ResearchPool>) {
    pool.points = 0.0;
}

// ---- Technology knowledge propagation ----

/// Tracks which technologies a star system "knows about".
/// Tech effects only apply to colonies in systems that have received the knowledge.
#[derive(Component, Default, Debug)]
pub struct TechKnowledge {
    pub known_techs: HashSet<TechId>,
}

/// Techs that were just researched this tick, to be propagated to systems.
#[derive(Resource, Default)]
pub struct RecentlyResearched {
    pub techs: Vec<TechId>,
}

/// A technology propagating from the capital to a target system at light speed.
#[derive(Component)]
pub struct PendingKnowledgePropagation {
    pub tech_id: TechId,
    pub target_system: Entity,
    pub arrives_at: i64,
}

/// When techs are recently researched, propagate knowledge to all colonized systems.
/// The capital gets the tech immediately; remote colonies receive it after light delay.
pub fn propagate_tech_knowledge(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut recently_researched: ResMut<RecentlyResearched>,
    colonies: Query<&Colony>,
    stars: Query<(Entity, &StarSystem, &Position)>,
    mut tech_knowledge: Query<&mut TechKnowledge>,
) {
    if recently_researched.techs.is_empty() {
        return;
    }

    // Find capital system
    let capital = stars.iter().find(|(_, s, _)| s.is_capital);
    let Some((capital_entity, _, capital_pos)) = capital else {
        recently_researched.techs.clear();
        return;
    };
    let capital_pos = *capital_pos;

    // Collect colonized system entities
    let colonized_systems: HashSet<Entity> = colonies.iter().map(|c| c.system).collect();

    for tech_id in recently_researched.techs.drain(..) {
        // Capital gets it immediately
        if let Ok(mut knowledge) = tech_knowledge.get_mut(capital_entity) {
            knowledge.known_techs.insert(tech_id);
        }

        // Other colonized systems get it after light delay
        for (sys_entity, _, sys_pos) in stars.iter() {
            if sys_entity == capital_entity {
                continue;
            }
            if !colonized_systems.contains(&sys_entity) {
                continue;
            }
            let distance = physics::distance_ly(&capital_pos, sys_pos);
            let delay = physics::light_delay_hexadies(distance);
            commands.spawn(PendingKnowledgePropagation {
                tech_id,
                target_system: sys_entity,
                arrives_at: clock.elapsed + delay,
            });
        }
    }
}

/// Receive pending knowledge propagations that have arrived.
pub fn receive_tech_knowledge(
    mut commands: Commands,
    clock: Res<GameClock>,
    pending: Query<(Entity, &PendingKnowledgePropagation)>,
    mut tech_knowledge: Query<&mut TechKnowledge>,
) {
    for (entity, prop) in pending.iter() {
        if clock.elapsed >= prop.arrives_at {
            if let Ok(mut knowledge) = tech_knowledge.get_mut(prop.target_system) {
                knowledge.known_techs.insert(prop.tech_id);
            }
            commands.entity(entity).despawn();
        }
    }
}

// ---- Lua parsing ----

/// Read `_tech_definitions` from the Lua state and convert to `Vec<Technology>`.
/// The `on_researched` callback stays in the Lua table and is not extracted here;
/// it will be invoked by the scripting system when research completes.
pub fn parse_tech_definitions(lua: &mlua::Lua) -> Result<Vec<Technology>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_tech_definitions")?;
    let mut techs = Vec::new();
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id = TechId(table.get::<u32>("id")?);
        let name: String = table.get("name")?;
        let branch = match table.get::<String>("branch")?.as_str() {
            "social" => TechBranch::Social,
            "physics" => TechBranch::Physics,
            "industrial" => TechBranch::Industrial,
            "military" => TechBranch::Military,
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "Unknown branch: {other}"
                )))
            }
        };
        // Support both scalar cost (backward compat: research-only) and table cost
        let cost: TechCost = match table.get::<mlua::Value>("cost")? {
            mlua::Value::Number(n) => TechCost {
                research: Amt::from_f64(n),
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
            },
            mlua::Value::Integer(n) => TechCost {
                research: Amt::units(n as u64),
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
            },
            mlua::Value::Table(t) => TechCost {
                research: Amt::from_f64(t.get::<f64>("research").unwrap_or(0.0)),
                minerals: Amt::from_f64(t.get::<f64>("minerals").unwrap_or(0.0)),
                energy: Amt::from_f64(t.get::<f64>("energy").unwrap_or(0.0)),
            },
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "cost must be a number or table".to_string(),
                ))
            }
        };

        let prereqs_table: mlua::Table = table.get("prerequisites")?;
        let prerequisites: Vec<TechId> = prereqs_table
            .sequence_values::<u32>()
            .map(|r| r.map(TechId))
            .collect::<Result<_, _>>()?;

        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();

        techs.push(Technology {
            id,
            name,
            branch,
            cost,
            prerequisites,
            description,
        });
    }
    Ok(techs)
}

/// Hardcoded fallback tech definitions used when Lua scripts are unavailable.
pub fn create_initial_tech_tree_vec() -> Vec<Technology> {
    vec![
        // === Social Branch ===
        Technology {
            id: TechId(100),
            name: "Xenolinguistics".into(),
            branch: TechBranch::Social,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: "Foundational study of alien communication patterns".into(),
        },
        Technology {
            id: TechId(101),
            name: "Colonial Administration".into(),
            branch: TechBranch::Social,
            cost: TechCost::research_only(Amt::units(150)),
            prerequisites: vec![],
            description: "Improved governance structures for distant colonies".into(),
        },
        Technology {
            id: TechId(102),
            name: "Interstellar Commerce".into(),
            branch: TechBranch::Social,
            cost: TechCost::research_only(Amt::units(250)),
            prerequisites: vec![TechId(101)],
            description: "Trade frameworks spanning star systems".into(),
        },
        Technology {
            id: TechId(103),
            name: "Cultural Exchange Protocols".into(),
            branch: TechBranch::Social,
            cost: TechCost::research_only(Amt::units(300)),
            prerequisites: vec![TechId(100)],
            description: "Formalised frameworks for cross-species cultural interaction".into(),
        },
        // === Physics Branch ===
        Technology {
            id: TechId(200),
            name: "Advanced Sensor Arrays".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: "Next-generation sensors for deep space observation".into(),
        },
        Technology {
            id: TechId(201),
            name: "Improved Sublight Drives".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(200)),
            prerequisites: vec![],
            description: "Enhances sublight drive efficiency".into(),
        },
        Technology {
            id: TechId(202),
            name: "FTL Theory".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(400)),
            prerequisites: vec![TechId(201)],
            description: "Theoretical foundations for faster-than-light travel".into(),
        },
        Technology {
            id: TechId(203),
            name: "Warp Field Stabilisation".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(600)),
            prerequisites: vec![TechId(202)],
            description: "Stabilise warp fields for safer FTL travel".into(),
        },
        // === Industrial Branch ===
        Technology {
            id: TechId(300),
            name: "Automated Mining".into(),
            branch: TechBranch::Industrial,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: "Robotic systems for autonomous resource extraction".into(),
        },
        Technology {
            id: TechId(301),
            name: "Orbital Fabrication".into(),
            branch: TechBranch::Industrial,
            cost: TechCost::research_only(Amt::units(200)),
            prerequisites: vec![TechId(300)],
            description: "Manufacturing facilities in orbit for zero-gravity construction".into(),
        },
        Technology {
            id: TechId(302),
            name: "Fusion Power Plants".into(),
            branch: TechBranch::Industrial,
            cost: TechCost::research_only(Amt::units(300)),
            prerequisites: vec![TechId(300)],
            description: "Harness fusion reactions for abundant clean energy".into(),
        },
        Technology {
            id: TechId(303),
            name: "Nano-Assembly".into(),
            branch: TechBranch::Industrial,
            cost: TechCost::research_only(Amt::units(500)),
            prerequisites: vec![TechId(301)],
            description: "Molecular-scale construction for unprecedented precision".into(),
        },
        // === Military Branch ===
        Technology {
            id: TechId(400),
            name: "Kinetic Weapons".into(),
            branch: TechBranch::Military,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: "Mass-driver based weapon systems".into(),
        },
        Technology {
            id: TechId(401),
            name: "Deflector Shields".into(),
            branch: TechBranch::Military,
            cost: TechCost::research_only(Amt::units(200)),
            prerequisites: vec![],
            description: "Energy barriers to deflect incoming projectiles".into(),
        },
        Technology {
            id: TechId(402),
            name: "Composite Armor".into(),
            branch: TechBranch::Military,
            cost: TechCost::research_only(Amt::units(250)),
            prerequisites: vec![TechId(400)],
            description: "Multi-layered hull plating for enhanced protection".into(),
        },
    ]
}

/// Convenience function: build a TechTree from hardcoded definitions.
pub fn create_initial_tech_tree() -> TechTree {
    TechTree::from_vec(create_initial_tech_tree_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardcoded_tech_tree() {
        let tree = create_initial_tech_tree();
        assert_eq!(tree.technologies.len(), 15);
        assert!(tree.get(TechId(100)).is_some());
        assert!(tree.get(TechId(402)).is_some());
    }

    #[test]
    fn test_parse_lua_tech_definitions() {
        let lua = mlua::Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua).unwrap();

        lua.load(
            r#"
            define_tech {
                id = 999,
                name = "Test Tech",
                branch = "physics",
                cost = 42.0,
                prerequisites = {},
                description = "A test technology",
                on_researched = function()
                    -- TODO: push_empire_modifier
                end,
            }
            define_tech {
                id = 1000,
                name = "Advanced Test Tech",
                branch = "military",
                cost = 100.0,
                prerequisites = { 999 },
                description = "Depends on test tech",
                on_researched = function()
                    -- TODO: push_empire_modifier
                end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let techs = parse_tech_definitions(&lua).unwrap();
        assert_eq!(techs.len(), 2);

        let first = &techs[0];
        assert_eq!(first.id, TechId(999));
        assert_eq!(first.name, "Test Tech");
        assert_eq!(first.branch, TechBranch::Physics);
        assert_eq!(first.cost.research, Amt::units(42));
        assert!(first.prerequisites.is_empty());

        let second = &techs[1];
        assert_eq!(second.id, TechId(1000));
        assert_eq!(second.prerequisites, vec![TechId(999)]);
    }

    #[test]
    fn test_parse_lua_tech_table_cost() {
        let lua = mlua::Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua).unwrap();

        lua.load(
            r#"
            define_tech {
                id = 888,
                name = "Expensive Tech",
                branch = "industrial",
                cost = { research = 200.0, minerals = 50.0, energy = 30.0 },
                prerequisites = {},
                description = "A tech with table cost",
                on_researched = function() end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let techs = parse_tech_definitions(&lua).unwrap();
        assert_eq!(techs.len(), 1);
        let tech = &techs[0];
        assert_eq!(tech.cost.research, Amt::units(200));
        assert_eq!(tech.cost.minerals, Amt::units(50));
        assert_eq!(tech.cost.energy, Amt::units(30));
    }

    #[test]
    fn test_load_lua_files_from_disk() {
        let engine = crate::scripting::ScriptEngine::new().unwrap();
        let tech_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/tech");
        engine
            .load_directory(&tech_dir)
            .expect("Failed to load tech scripts from disk");
        let techs = parse_tech_definitions(engine.lua()).expect("Failed to parse tech scripts");
        // Should load all 15 technologies from the 4 Lua files
        assert_eq!(techs.len(), 15);
        // Verify one tech from each branch
        assert!(techs
            .iter()
            .any(|t| t.id == TechId(100) && t.branch == TechBranch::Social));
        assert!(techs
            .iter()
            .any(|t| t.id == TechId(201) && t.branch == TechBranch::Physics));
        assert!(techs
            .iter()
            .any(|t| t.id == TechId(300) && t.branch == TechBranch::Industrial));
        assert!(techs
            .iter()
            .any(|t| t.id == TechId(402) && t.branch == TechBranch::Military));
    }

    #[test]
    fn can_research_no_prerequisites() {
        let tree = TechTree::from_vec(vec![Technology {
            id: TechId(1),
            name: "Basic".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: String::new(),
        }]);
        assert!(tree.can_research(TechId(1)));
    }

    #[test]
    fn cannot_research_missing_prerequisites() {
        let tree = TechTree::from_vec(vec![
            Technology {
                id: TechId(1),
                name: "Basic".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(100)),
                prerequisites: vec![],
                description: String::new(),
            },
            Technology {
                id: TechId(2),
                name: "Advanced".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(200)),
                prerequisites: vec![TechId(1)],
                description: String::new(),
            },
        ]);
        assert!(!tree.can_research(TechId(2)));
    }

    #[test]
    fn can_research_after_completing_prerequisites() {
        let mut tree = TechTree::from_vec(vec![
            Technology {
                id: TechId(1),
                name: "Basic".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(100)),
                prerequisites: vec![],
                description: String::new(),
            },
            Technology {
                id: TechId(2),
                name: "Advanced".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(200)),
                prerequisites: vec![TechId(1)],
                description: String::new(),
            },
        ]);
        tree.complete_research(TechId(1));
        assert!(tree.can_research(TechId(2)));
    }

    #[test]
    fn cannot_research_already_researched() {
        let mut tree = TechTree::from_vec(vec![Technology {
            id: TechId(1),
            name: "Basic".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: String::new(),
        }]);
        tree.complete_research(TechId(1));
        assert!(!tree.can_research(TechId(1)));
    }

    #[test]
    fn is_researched() {
        let mut tree = TechTree::from_vec(vec![Technology {
            id: TechId(1),
            name: "Basic".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: String::new(),
        }]);
        assert!(!tree.is_researched(TechId(1)));
        tree.complete_research(TechId(1));
        assert!(tree.is_researched(TechId(1)));
    }

    #[test]
    fn available_technologies_returns_only_researchable() {
        let mut tree = TechTree::from_vec(vec![
            Technology {
                id: TechId(1),
                name: "Basic".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(100)),
                prerequisites: vec![],
                description: String::new(),
            },
            Technology {
                id: TechId(2),
                name: "Advanced".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(200)),
                prerequisites: vec![TechId(1)],
                description: String::new(),
            },
            Technology {
                id: TechId(3),
                name: "Other".into(),
                branch: TechBranch::Social,
                cost: TechCost::research_only(Amt::units(100)),
                prerequisites: vec![],
                description: String::new(),
            },
        ]);

        let available: Vec<TechId> = tree.available_technologies().iter().map(|t| t.id).collect();
        assert!(available.contains(&TechId(1)));
        assert!(available.contains(&TechId(3)));
        assert!(!available.contains(&TechId(2)));

        tree.complete_research(TechId(1));
        let available: Vec<TechId> = tree.available_technologies().iter().map(|t| t.id).collect();
        assert!(!available.contains(&TechId(1)));
        assert!(available.contains(&TechId(2)));
        assert!(available.contains(&TechId(3)));
    }
}
