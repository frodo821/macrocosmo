use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::colony::ResourceStockpile;
use crate::time_system::GameClock;

pub struct TechnologyPlugin;

impl Plugin for TechnologyPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TechTree::default())
            .insert_resource(ResearchQueue::default())
            .insert_resource(ResearchPool::default())
            .insert_resource(LastResearchTick(0))
            .add_systems(Update, (collect_research, tick_research).chain());
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TechBranch {
    Social,
    Physics,
    Industrial,
    Military,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TechId(pub u32);

#[derive(Clone, Debug)]
pub struct Technology {
    pub id: TechId,
    pub name: String,
    pub branch: TechBranch,
    pub cost: f64,
    pub prerequisites: Vec<TechId>,
    pub effects: Vec<TechEffect>,
    pub description: String,
}

#[derive(Clone, Debug)]
pub enum TechEffect {
    ModifySubLightSpeed(f64),
    ModifyFTLSpeed(f64),
    ModifyFTLRange(f64),
    ModifySurveyRange(f64),
    ModifyProductionRate {
        resource: ResourceType,
        multiplier: f64,
    },
    ModifyPopulationGrowth(f64),
    ModifyBuildSpeed(f64),
    UnlockBuilding(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceType {
    Minerals,
    Energy,
    Research,
}

/// Holds all technology definitions and which ones have been researched.
#[derive(Resource, Default)]
pub struct TechTree {
    pub technologies: HashMap<TechId, Technology>,
    pub researched: HashSet<TechId>,
}

impl TechTree {
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
fn collect_research(
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
fn tick_research(
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
        assert!(!available.contains(&TechId(1))); // already researched
        assert!(available.contains(&TechId(2))); // now available
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
