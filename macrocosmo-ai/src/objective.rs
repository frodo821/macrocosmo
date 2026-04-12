//! Objective data types.
//!
//! An `Objective` describes what the AI is trying to achieve and how its
//! progress is measured. It is pure data — no evaluation logic beyond
//! constructors. Evaluation is split across:
//!
//! - `condition::Condition::evaluate` for preconditions / success criteria
//! - `feasibility::evaluate` for the feasibility score
//! - Game-side code for decomposition and delegation

use ahash::AHashMap;
use serde::{Deserialize, Serialize};

use crate::condition::Condition;
use crate::feasibility::FeasibilityFormula;
use crate::ids::ObjectiveId;
use crate::value_expr::ValueExpr;

/// Free-form parameter bag attached to an `Objective` (e.g., target faction,
/// target metric threshold). Game code interprets keys.
pub type ObjectiveParams = AHashMap<std::sync::Arc<str>, ValueExpr>;

/// The set of preconditions that must hold for an objective to be considered
/// feasible to pursue. A single `Condition` suffices — use `Condition::All`
/// (or the `Condition::and(...)` helper) for conjunction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreconditionSet {
    pub condition: Condition,
}

impl PreconditionSet {
    pub fn new(condition: Condition) -> Self {
        Self { condition }
    }

    pub fn always() -> Self {
        Self {
            condition: Condition::Always,
        }
    }
}

impl Default for PreconditionSet {
    fn default() -> Self {
        Self::always()
    }
}

/// The condition under which an objective is considered complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuccessCriteria {
    pub condition: Condition,
}

impl SuccessCriteria {
    pub fn new(condition: Condition) -> Self {
        Self { condition }
    }
}

/// A rule for decomposing a parent objective into sub-objectives when a
/// trigger condition holds. Game-side orchestration applies these rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecompositionRule {
    pub parent: ObjectiveId,
    pub children: Vec<ObjectiveId>,
    pub trigger: Condition,
}

/// A full objective definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Objective {
    pub id: ObjectiveId,
    pub params: ObjectiveParams,
    pub precondition: PreconditionSet,
    pub success: SuccessCriteria,
    pub feasibility: FeasibilityFormula,
}

impl Objective {
    pub fn new(
        id: ObjectiveId,
        precondition: PreconditionSet,
        success: SuccessCriteria,
        feasibility: FeasibilityFormula,
    ) -> Self {
        Self {
            id,
            params: AHashMap::new(),
            precondition,
            success,
            feasibility,
        }
    }

    pub fn with_param(
        mut self,
        key: impl Into<std::sync::Arc<str>>,
        value: ValueExpr,
    ) -> Self {
        self.params.insert(key.into(), value);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feasibility::{FeasibilityFormula, FeasibilityTerm};

    #[test]
    fn precondition_default_is_always() {
        let p = PreconditionSet::default();
        assert_eq!(p.condition, Condition::Always);
    }

    #[test]
    fn objective_ctor_builds_sane_struct() {
        let id = ObjectiveId::from("defensive_posture");
        let obj = Objective::new(
            id.clone(),
            PreconditionSet::always(),
            SuccessCriteria::new(Condition::Always),
            FeasibilityFormula::WeightedSum(vec![FeasibilityTerm::new(
                1.0,
                ValueExpr::Literal(0.5),
            )]),
        )
        .with_param("intensity", ValueExpr::Literal(0.8));
        assert_eq!(obj.id, id);
        assert!(obj.params.contains_key("intensity"));
    }

    #[test]
    fn decomposition_rule_roundtrips() {
        let parent = ObjectiveId::from("eliminate_threat");
        let child_a = ObjectiveId::from("attack_target");
        let child_b = ObjectiveId::from("fortify_border");
        let rule = DecompositionRule {
            parent: parent.clone(),
            children: vec![child_a.clone(), child_b.clone()],
            trigger: Condition::Always,
        };
        assert_eq!(rule.parent, parent);
        assert_eq!(rule.children.len(), 2);
    }
}
