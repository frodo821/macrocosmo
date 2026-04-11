use std::collections::{HashMap, HashSet};

use crate::amount::Amt;

/// Unique identifier for a technology.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TechId(pub String);

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
#[derive(bevy::prelude::Resource, bevy::prelude::Component, Debug, Clone, Default)]
pub struct TechTree {
    pub technologies: HashMap<TechId, Technology>,
    pub researched: HashSet<TechId>,
}

impl TechTree {
    pub fn from_vec(techs: Vec<Technology>) -> Self {
        let technologies = techs.into_iter().map(|t| (t.id.clone(), t)).collect();
        Self {
            technologies,
            researched: HashSet::new(),
        }
    }

    /// Insert a technology into the tree.
    pub fn add(&mut self, tech: Technology) {
        self.technologies.insert(tech.id.clone(), tech);
    }

    /// Get a technology by its id.
    pub fn get(&self, id: &TechId) -> Option<&Technology> {
        self.technologies.get(id)
    }

    pub fn is_researched(&self, id: &TechId) -> bool {
        self.researched.contains(id)
    }

    pub fn can_research(&self, id: &TechId) -> bool {
        if self.researched.contains(id) {
            return false;
        }
        let Some(tech) = self.technologies.get(id) else {
            return false;
        };
        tech.prerequisites
            .iter()
            .all(|pre| self.researched.contains(pre))
    }

    /// Alias used by the research panel UI.
    pub fn is_available(&self, id: &TechId) -> bool {
        self.can_research(id)
    }

    pub fn available_technologies(&self) -> Vec<&Technology> {
        self.technologies
            .values()
            .filter(|t| self.can_research(&t.id))
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
                    missing.push((tech.id.clone(), prereq.clone()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_research_no_prerequisites() {
        let tree = TechTree::from_vec(vec![Technology {
            id: TechId("test_1".into()),
            name: "Basic".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: String::new(),
        }]);
        assert!(tree.can_research(&TechId("test_1".into())));
    }

    #[test]
    fn cannot_research_missing_prerequisites() {
        let tree = TechTree::from_vec(vec![
            Technology {
                id: TechId("test_1".into()),
                name: "Basic".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(100)),
                prerequisites: vec![],
                description: String::new(),
            },
            Technology {
                id: TechId("test_2".into()),
                name: "Advanced".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(200)),
                prerequisites: vec![TechId("test_1".into())],
                description: String::new(),
            },
        ]);
        assert!(!tree.can_research(&TechId("test_2".into())));
    }

    #[test]
    fn can_research_after_completing_prerequisites() {
        let mut tree = TechTree::from_vec(vec![
            Technology {
                id: TechId("test_1".into()),
                name: "Basic".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(100)),
                prerequisites: vec![],
                description: String::new(),
            },
            Technology {
                id: TechId("test_2".into()),
                name: "Advanced".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(200)),
                prerequisites: vec![TechId("test_1".into())],
                description: String::new(),
            },
        ]);
        tree.complete_research(TechId("test_1".into()));
        assert!(tree.can_research(&TechId("test_2".into())));
    }

    #[test]
    fn cannot_research_already_researched() {
        let mut tree = TechTree::from_vec(vec![Technology {
            id: TechId("test_1".into()),
            name: "Basic".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: String::new(),
        }]);
        tree.complete_research(TechId("test_1".into()));
        assert!(!tree.can_research(&TechId("test_1".into())));
    }

    #[test]
    fn is_researched() {
        let mut tree = TechTree::from_vec(vec![Technology {
            id: TechId("test_1".into()),
            name: "Basic".into(),
            branch: TechBranch::Physics,
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: String::new(),
        }]);
        assert!(!tree.is_researched(&TechId("test_1".into())));
        tree.complete_research(TechId("test_1".into()));
        assert!(tree.is_researched(&TechId("test_1".into())));
    }

    #[test]
    fn available_technologies_returns_only_researchable() {
        let mut tree = TechTree::from_vec(vec![
            Technology {
                id: TechId("test_1".into()),
                name: "Basic".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(100)),
                prerequisites: vec![],
                description: String::new(),
            },
            Technology {
                id: TechId("test_2".into()),
                name: "Advanced".into(),
                branch: TechBranch::Physics,
                cost: TechCost::research_only(Amt::units(200)),
                prerequisites: vec![TechId("test_1".into())],
                description: String::new(),
            },
            Technology {
                id: TechId("test_3".into()),
                name: "Other".into(),
                branch: TechBranch::Social,
                cost: TechCost::research_only(Amt::units(100)),
                prerequisites: vec![],
                description: String::new(),
            },
        ]);

        let available: Vec<TechId> = tree.available_technologies().iter().map(|t| t.id.clone()).collect();
        assert!(available.contains(&TechId("test_1".into())));
        assert!(available.contains(&TechId("test_3".into())));
        assert!(!available.contains(&TechId("test_2".into())));

        tree.complete_research(TechId("test_1".into()));
        let available: Vec<TechId> = tree.available_technologies().iter().map(|t| t.id.clone()).collect();
        assert!(!available.contains(&TechId("test_1".into())));
        assert!(available.contains(&TechId("test_2".into())));
        assert!(available.contains(&TechId("test_3".into())));
    }
}
