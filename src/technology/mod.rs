use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::colony::ResourceStockpile;
use crate::time_system::GameClock;

pub struct TechnologyPlugin;

impl Plugin for TechnologyPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(create_initial_tech_tree())
            .insert_resource(ResearchQueue::default())
            .insert_resource(ResearchPool::default())
            .insert_resource(LastResearchTick(0))
            .add_systems(Update, (collect_research, tick_research).chain());
    }
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

/// The type of resource a production modifier applies to.
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceType {
    Minerals,
    Energy,
    Research,
    All,
}

/// An effect granted when a technology is researched.
#[derive(Debug, Clone, PartialEq)]
pub enum TechEffect {
    ModifyPopulationGrowth(f64),
    ModifyProductionRate {
        resource: ResourceType,
        multiplier: f64,
    },
    ModifySubLightSpeed(f64),
    ModifyFTLRange(f64),
    ModifyFTLSpeed(f64),
    ModifySurveyRange(f64),
    ModifyBuildSpeed(f64),
    UnlockBuilding(String),
    ModifyHullStrength(f64),
    ModifyWeaponDamage(f64),
    Placeholder(String),
}

/// A single technology definition.
#[derive(Debug, Clone)]
pub struct Technology {
    pub id: TechId,
    pub name: String,
    pub description: String,
    pub branch: TechBranch,
    pub cost: f64,
    pub prerequisites: Vec<TechId>,
    pub effects: Vec<TechEffect>,
}

/// The complete technology tree, indexed by TechId.
#[derive(Resource, Debug, Clone, Default)]
pub struct TechTree {
    pub technologies: HashMap<TechId, Technology>,
    pub researched: HashSet<TechId>,
}

impl TechTree {
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
}

/// Global research points pool (accumulated from colonies).
#[derive(Resource, Default)]
pub struct ResearchPool {
    pub points: f64,
}

/// Tracks the last game tick at which research was collected, to compute delta.
#[derive(Resource)]
pub struct LastResearchTick(pub i64);

/// Drains research from colony stockpiles into the global ResearchPool.
pub fn collect_research(
    clock: Res<GameClock>,
    last_tick: Res<LastResearchTick>,
    mut pool: ResMut<ResearchPool>,
    mut stockpiles: Query<&mut ResourceStockpile>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    for mut stockpile in &mut stockpiles {
        if stockpile.research > 0.0 {
            pool.points += stockpile.research;
            stockpile.research = 0.0;
        }
    }
}

/// Processes research each tick: transfers points from pool to current project.
pub fn tick_research(
    clock: Res<GameClock>,
    mut last_tick: ResMut<LastResearchTick>,
    mut tech_tree: ResMut<TechTree>,
    mut queue: ResMut<ResearchQueue>,
    mut pool: ResMut<ResearchPool>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    last_tick.0 = clock.elapsed;

    let Some(current_tech_id) = queue.current else {
        return;
    };

    let tech_cost = {
        let Some(tech) = tech_tree.technologies.get(&current_tech_id) else {
            queue.current = None;
            return;
        };
        tech.cost
    };

    // Transfer available research points from pool
    let needed = tech_cost - queue.accumulated;
    if needed > 0.0 {
        let transfer = pool.points.min(needed);
        if transfer > 0.0 {
            pool.points -= transfer;
            queue.accumulated += transfer;
        }
    }

    // Check completion
    if queue.accumulated >= tech_cost {
        let tech_name = tech_tree
            .technologies
            .get(&current_tech_id)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        tech_tree.complete_research(current_tech_id);
        queue.current = None;
        queue.accumulated = 0.0;
        info!("Research complete: {}", tech_name);
    }
}

/// Build the initial technology tree with starter technologies for every branch.
pub fn create_initial_tech_tree() -> TechTree {
    let mut tree = TechTree::default();

    // -- Social (101-199) --
    tree.add(Technology {
        id: TechId(101),
        name: "Population Growth I".into(),
        description: "Basic genetic and social programs to boost birth rates.".into(),
        branch: TechBranch::Social,
        cost: 200.0,
        prerequisites: vec![],
        effects: vec![TechEffect::ModifyPopulationGrowth(0.005)],
    });
    tree.add(Technology {
        id: TechId(102),
        name: "Autonomous AI Basics".into(),
        description: "Foundational research into autonomous decision-making agents.".into(),
        branch: TechBranch::Social,
        cost: 300.0,
        prerequisites: vec![TechId(101)],
        effects: vec![TechEffect::Placeholder(
            "Enables basic AI governance assistants".into(),
        )],
    });
    tree.add(Technology {
        id: TechId(103),
        name: "Governance Efficiency".into(),
        description: "Streamlined bureaucracy improves all production.".into(),
        branch: TechBranch::Social,
        cost: 400.0,
        prerequisites: vec![TechId(101)],
        effects: vec![TechEffect::ModifyProductionRate {
            resource: ResourceType::All,
            multiplier: 1.1,
        }],
    });

    // -- Physics (201-299) --
    tree.add(Technology {
        id: TechId(201),
        name: "Improved Sublight Drives".into(),
        description: "Push sub-light cruise speed from 0.75c to 0.85c.".into(),
        branch: TechBranch::Physics,
        cost: 200.0,
        prerequisites: vec![],
        effects: vec![TechEffect::ModifySubLightSpeed(0.1)],
    });
    tree.add(Technology {
        id: TechId(202),
        name: "Extended FTL Range".into(),
        description: "Extend FTL jump range from 30 to 40 light-years.".into(),
        branch: TechBranch::Physics,
        cost: 300.0,
        prerequisites: vec![],
        effects: vec![TechEffect::ModifyFTLRange(10.0)],
    });
    tree.add(Technology {
        id: TechId(203),
        name: "FTL Speed Enhancement".into(),
        description: "Boost FTL cruise speed from 10c to 15c.".into(),
        branch: TechBranch::Physics,
        cost: 400.0,
        prerequisites: vec![TechId(201)],
        effects: vec![TechEffect::ModifyFTLSpeed(5.0)],
    });
    tree.add(Technology {
        id: TechId(204),
        name: "Advanced Sensors".into(),
        description: "Improve survey sensor range from 5 to 8 light-years.".into(),
        branch: TechBranch::Physics,
        cost: 250.0,
        prerequisites: vec![],
        effects: vec![TechEffect::ModifySurveyRange(3.0)],
    });
    tree.add(Technology {
        id: TechId(205),
        name: "Automated Courier Network".into(),
        description: "Enables automated communication routes between colonies.".into(),
        branch: TechBranch::Physics,
        cost: 500.0,
        prerequisites: vec![TechId(204)],
        effects: vec![TechEffect::Placeholder(
            "Enables automated communication routes".into(),
        )],
    });

    // -- Industrial (301-399) --
    tree.add(Technology {
        id: TechId(301),
        name: "Efficient Mining".into(),
        description: "Better extraction techniques boost mineral output by 20%.".into(),
        branch: TechBranch::Industrial,
        cost: 200.0,
        prerequisites: vec![],
        effects: vec![TechEffect::ModifyProductionRate {
            resource: ResourceType::Minerals,
            multiplier: 1.2,
        }],
    });
    tree.add(Technology {
        id: TechId(302),
        name: "Energy Grid Optimization".into(),
        description: "Smarter power grids boost energy output by 20%.".into(),
        branch: TechBranch::Industrial,
        cost: 200.0,
        prerequisites: vec![],
        effects: vec![TechEffect::ModifyProductionRate {
            resource: ResourceType::Energy,
            multiplier: 1.2,
        }],
    });
    tree.add(Technology {
        id: TechId(303),
        name: "Rapid Construction".into(),
        description: "Prefabrication methods increase build speed by 50%.".into(),
        branch: TechBranch::Industrial,
        cost: 350.0,
        prerequisites: vec![TechId(301), TechId(302)],
        effects: vec![TechEffect::ModifyBuildSpeed(1.5)],
    });
    tree.add(Technology {
        id: TechId(304),
        name: "Advanced Shipyard".into(),
        description: "Unlocks the Advanced Shipyard building for heavier vessels.".into(),
        branch: TechBranch::Industrial,
        cost: 400.0,
        prerequisites: vec![TechId(303)],
        effects: vec![TechEffect::UnlockBuilding("AdvancedShipyard".into())],
    });

    // -- Military (401-499) --
    tree.add(Technology {
        id: TechId(401),
        name: "Hull Reinforcement".into(),
        description: "Composite armour plating increases hull strength by 25%.".into(),
        branch: TechBranch::Military,
        cost: 200.0,
        prerequisites: vec![],
        effects: vec![TechEffect::ModifyHullStrength(1.25)],
    });
    tree.add(Technology {
        id: TechId(402),
        name: "Basic Armaments".into(),
        description: "Standard weapon systems for military vessels.".into(),
        branch: TechBranch::Military,
        cost: 300.0,
        prerequisites: vec![],
        effects: vec![TechEffect::ModifyWeaponDamage(1.2)],
    });
    tree.add(Technology {
        id: TechId(403),
        name: "Defense Platforms".into(),
        description: "Orbital defense stations to protect colonies.".into(),
        branch: TechBranch::Military,
        cost: 400.0,
        prerequisites: vec![TechId(401), TechId(402)],
        effects: vec![TechEffect::Placeholder(
            "Unlocks orbital defense platform construction".into(),
        )],
    });

    tree
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tech(id: u32, name: &str, cost: f64, prerequisites: Vec<u32>) -> Technology {
        Technology {
            id: TechId(id),
            name: name.to_string(),
            branch: TechBranch::Physics,
            cost,
            prerequisites: prerequisites.into_iter().map(TechId).collect(),
            effects: vec![],
            description: String::new(),
        }
    }

    fn make_tree(techs: Vec<Technology>) -> TechTree {
        let mut tree = TechTree::default();
        for tech in techs {
            tree.technologies.insert(tech.id, tech);
        }
        tree
    }

    #[test]
    fn tree_has_expected_technology_count() {
        let tree = create_initial_tech_tree();
        // 3 Social + 5 Physics + 4 Industrial + 3 Military = 15
        assert_eq!(tree.technologies.len(), 15);
    }

    #[test]
    fn branch_counts_are_correct() {
        let tree = create_initial_tech_tree();
        assert_eq!(tree.branch(TechBranch::Social).len(), 3);
        assert_eq!(tree.branch(TechBranch::Physics).len(), 5);
        assert_eq!(tree.branch(TechBranch::Industrial).len(), 4);
        assert_eq!(tree.branch(TechBranch::Military).len(), 3);
    }

    #[test]
    fn all_prerequisites_exist() {
        let tree = create_initial_tech_tree();
        assert!(
            tree.validate_prerequisites().is_ok(),
            "Some prerequisites reference missing tech ids"
        );
    }

    #[test]
    fn prerequisite_chains_are_valid() {
        let tree = create_initial_tech_tree();

        let t102 = tree.get(TechId(102)).unwrap();
        assert!(t102.prerequisites.contains(&TechId(101)));
        let t103 = tree.get(TechId(103)).unwrap();
        assert!(t103.prerequisites.contains(&TechId(101)));

        let t203 = tree.get(TechId(203)).unwrap();
        assert!(t203.prerequisites.contains(&TechId(201)));
        let t205 = tree.get(TechId(205)).unwrap();
        assert!(t205.prerequisites.contains(&TechId(204)));

        let t303 = tree.get(TechId(303)).unwrap();
        assert!(t303.prerequisites.contains(&TechId(301)));
        assert!(t303.prerequisites.contains(&TechId(302)));
        let t304 = tree.get(TechId(304)).unwrap();
        assert!(t304.prerequisites.contains(&TechId(303)));

        let t403 = tree.get(TechId(403)).unwrap();
        assert!(t403.prerequisites.contains(&TechId(401)));
        assert!(t403.prerequisites.contains(&TechId(402)));
    }

    #[test]
    fn no_technology_requires_itself() {
        let tree = create_initial_tech_tree();
        for tech in tree.technologies.values() {
            assert!(
                !tech.prerequisites.contains(&tech.id),
                "Tech {:?} lists itself as a prerequisite",
                tech.id
            );
        }
    }

    #[test]
    fn costs_are_positive() {
        let tree = create_initial_tech_tree();
        for tech in tree.technologies.values() {
            assert!(
                tech.cost > 0.0,
                "Tech {:?} has non-positive cost {}",
                tech.id,
                tech.cost
            );
        }
    }

    #[test]
    fn can_research_no_prerequisites() {
        let tree = make_tree(vec![make_tech(1, "Basic Physics", 100.0, vec![])]);
        assert!(tree.can_research(TechId(1)));
    }

    #[test]
    fn cannot_research_missing_prerequisites() {
        let tree = make_tree(vec![
            make_tech(1, "Basic Physics", 100.0, vec![]),
            make_tech(2, "Advanced Physics", 200.0, vec![1]),
        ]);
        assert!(!tree.can_research(TechId(2)));
    }

    #[test]
    fn can_research_after_completing_prerequisites() {
        let mut tree = make_tree(vec![
            make_tech(1, "Basic Physics", 100.0, vec![]),
            make_tech(2, "Advanced Physics", 200.0, vec![1]),
        ]);
        tree.complete_research(TechId(1));
        assert!(tree.can_research(TechId(2)));
    }

    #[test]
    fn cannot_research_already_researched() {
        let mut tree = make_tree(vec![make_tech(1, "Basic Physics", 100.0, vec![])]);
        tree.complete_research(TechId(1));
        assert!(!tree.can_research(TechId(1)));
    }

    #[test]
    fn is_researched() {
        let mut tree = make_tree(vec![make_tech(1, "Basic Physics", 100.0, vec![])]);
        assert!(!tree.is_researched(TechId(1)));
        tree.complete_research(TechId(1));
        assert!(tree.is_researched(TechId(1)));
    }

    #[test]
    fn cannot_research_nonexistent_tech() {
        let tree = TechTree::default();
        assert!(!tree.can_research(TechId(999)));
    }

    #[test]
    fn available_technologies_returns_only_researchable() {
        let mut tree = make_tree(vec![
            make_tech(1, "Basic Physics", 100.0, vec![]),
            make_tech(2, "Advanced Physics", 200.0, vec![1]),
            make_tech(3, "Basic Social", 100.0, vec![]),
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

    #[test]
    fn complete_research_marks_as_researched() {
        let mut tree = make_tree(vec![make_tech(1, "Test", 50.0, vec![])]);
        tree.complete_research(TechId(1));
        assert!(tree.researched.contains(&TechId(1)));
    }

    #[test]
    fn multiple_prerequisites_all_required() {
        let mut tree = make_tree(vec![
            make_tech(1, "A", 100.0, vec![]),
            make_tech(2, "B", 100.0, vec![]),
            make_tech(3, "C", 200.0, vec![1, 2]),
        ]);

        assert!(!tree.can_research(TechId(3)));
        tree.complete_research(TechId(1));
        assert!(!tree.can_research(TechId(3)));
        tree.complete_research(TechId(2));
        assert!(tree.can_research(TechId(3)));
    }
}
