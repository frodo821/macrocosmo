use std::collections::HashSet;

/// Atomic condition that checks a single game-state predicate.
#[derive(Clone, Debug, PartialEq)]
pub enum ConditionAtom {
    HasTech(String),
    HasModifier(String),
    HasBuilding(String),
}

/// Composable condition tree. Used by structure prerequisites, event triggers, etc.
#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    Atom(ConditionAtom),
    /// All children must be satisfied.
    All(Vec<Condition>),
    /// At least one child must be satisfied.
    Any(Vec<Condition>),
    /// Exactly one child must be satisfied.
    OneOf(Vec<Condition>),
    /// The child must NOT be satisfied.
    Not(Box<Condition>),
}

/// Context for evaluating conditions against current game state.
pub struct EvalContext<'a> {
    pub researched_techs: &'a HashSet<String>,
    pub active_modifiers: &'a HashSet<String>,
    pub buildings: &'a HashSet<String>,
}

/// Result of evaluating a condition tree, preserving structure for UI display.
#[derive(Clone, Debug)]
pub enum ConditionResult {
    Atom {
        atom: ConditionAtom,
        satisfied: bool,
    },
    All {
        satisfied: bool,
        children: Vec<ConditionResult>,
    },
    Any {
        satisfied: bool,
        children: Vec<ConditionResult>,
    },
    OneOf {
        satisfied: bool,
        satisfied_count: usize,
        children: Vec<ConditionResult>,
    },
    Not {
        satisfied: bool,
        child: Box<ConditionResult>,
    },
}

impl Condition {
    pub fn evaluate(&self, ctx: &EvalContext) -> ConditionResult {
        match self {
            Condition::Atom(atom) => {
                let satisfied = match atom {
                    ConditionAtom::HasTech(id) => ctx.researched_techs.contains(id),
                    ConditionAtom::HasModifier(id) => ctx.active_modifiers.contains(id),
                    ConditionAtom::HasBuilding(id) => ctx.buildings.contains(id),
                };
                ConditionResult::Atom {
                    atom: atom.clone(),
                    satisfied,
                }
            }
            Condition::All(children) => {
                let results: Vec<_> = children.iter().map(|c| c.evaluate(ctx)).collect();
                let satisfied = results.iter().all(|r| r.is_satisfied());
                ConditionResult::All {
                    satisfied,
                    children: results,
                }
            }
            Condition::Any(children) => {
                let results: Vec<_> = children.iter().map(|c| c.evaluate(ctx)).collect();
                let satisfied = results.iter().any(|r| r.is_satisfied());
                ConditionResult::Any {
                    satisfied,
                    children: results,
                }
            }
            Condition::OneOf(children) => {
                let results: Vec<_> = children.iter().map(|c| c.evaluate(ctx)).collect();
                let count = results.iter().filter(|r| r.is_satisfied()).count();
                ConditionResult::OneOf {
                    satisfied: count == 1,
                    satisfied_count: count,
                    children: results,
                }
            }
            Condition::Not(child) => {
                let result = child.evaluate(ctx);
                let satisfied = !result.is_satisfied();
                ConditionResult::Not {
                    satisfied,
                    child: Box::new(result),
                }
            }
        }
    }
}

impl ConditionResult {
    pub fn is_satisfied(&self) -> bool {
        match self {
            ConditionResult::Atom { satisfied, .. } => *satisfied,
            ConditionResult::All { satisfied, .. } => *satisfied,
            ConditionResult::Any { satisfied, .. } => *satisfied,
            ConditionResult::OneOf { satisfied, .. } => *satisfied,
            ConditionResult::Not { satisfied, .. } => *satisfied,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(
        techs: &[&str],
        modifiers: &[&str],
        buildings: &[&str],
    ) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
        (
            techs.iter().map(|s| s.to_string()).collect(),
            modifiers.iter().map(|s| s.to_string()).collect(),
            buildings.iter().map(|s| s.to_string()).collect(),
        )
    }

    #[test]
    fn test_atom_has_tech() {
        let (techs, mods, bldgs) = ctx_with(&["laser_weapons"], &[], &[]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        let cond = Condition::Atom(ConditionAtom::HasTech("laser_weapons".into()));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::HasTech("plasma_weapons".into()));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_atom_has_modifier() {
        let (techs, mods, bldgs) = ctx_with(&[], &["war_economy"], &[]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        let cond = Condition::Atom(ConditionAtom::HasModifier("war_economy".into()));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::HasModifier("peace_economy".into()));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_atom_has_building() {
        let (techs, mods, bldgs) = ctx_with(&[], &[], &["shipyard"]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        let cond = Condition::Atom(ConditionAtom::HasBuilding("shipyard".into()));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::HasBuilding("factory".into()));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_all_combinator() {
        let (techs, mods, bldgs) = ctx_with(&["tech_a", "tech_b"], &[], &[]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::HasTech("tech_a".into())),
            Condition::Atom(ConditionAtom::HasTech("tech_b".into())),
        ]);
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::HasTech("tech_a".into())),
            Condition::Atom(ConditionAtom::HasTech("tech_c".into())),
        ]);
        assert!(!cond.evaluate(&ctx).is_satisfied());

        // Empty All is vacuously true
        let cond = Condition::All(vec![]);
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_any_combinator() {
        let (techs, mods, bldgs) = ctx_with(&["tech_a"], &[], &[]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        let cond = Condition::Any(vec![
            Condition::Atom(ConditionAtom::HasTech("tech_a".into())),
            Condition::Atom(ConditionAtom::HasTech("tech_b".into())),
        ]);
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Any(vec![
            Condition::Atom(ConditionAtom::HasTech("tech_x".into())),
            Condition::Atom(ConditionAtom::HasTech("tech_y".into())),
        ]);
        assert!(!cond.evaluate(&ctx).is_satisfied());

        // Empty Any is false
        let cond = Condition::Any(vec![]);
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_one_of_combinator() {
        let (techs, mods, bldgs) = ctx_with(&["tech_a", "tech_b"], &[], &[]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        // Two satisfied -> not exactly one
        let cond = Condition::OneOf(vec![
            Condition::Atom(ConditionAtom::HasTech("tech_a".into())),
            Condition::Atom(ConditionAtom::HasTech("tech_b".into())),
        ]);
        let result = cond.evaluate(&ctx);
        assert!(!result.is_satisfied());
        if let ConditionResult::OneOf {
            satisfied_count, ..
        } = result
        {
            assert_eq!(satisfied_count, 2);
        }

        // Exactly one satisfied
        let cond = Condition::OneOf(vec![
            Condition::Atom(ConditionAtom::HasTech("tech_a".into())),
            Condition::Atom(ConditionAtom::HasTech("tech_c".into())),
        ]);
        assert!(cond.evaluate(&ctx).is_satisfied());

        // None satisfied
        let cond = Condition::OneOf(vec![
            Condition::Atom(ConditionAtom::HasTech("tech_x".into())),
            Condition::Atom(ConditionAtom::HasTech("tech_y".into())),
        ]);
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_not_combinator() {
        let (techs, mods, bldgs) = ctx_with(&["tech_a"], &[], &[]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        let cond = Condition::Not(Box::new(Condition::Atom(ConditionAtom::HasTech(
            "tech_a".into(),
        ))));
        assert!(!cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Not(Box::new(Condition::Atom(ConditionAtom::HasTech(
            "tech_b".into(),
        ))));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_nested_conditions() {
        let (techs, mods, bldgs) = ctx_with(&["laser", "shields"], &["war_economy"], &["shipyard"]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        // All(HasTech("laser"), Any(HasModifier("war_economy"), HasBuilding("factory")))
        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::HasTech("laser".into())),
            Condition::Any(vec![
                Condition::Atom(ConditionAtom::HasModifier("war_economy".into())),
                Condition::Atom(ConditionAtom::HasBuilding("factory".into())),
            ]),
        ]);
        assert!(cond.evaluate(&ctx).is_satisfied());

        // Not(All(HasTech("laser"), HasTech("plasma")))
        let cond = Condition::Not(Box::new(Condition::All(vec![
            Condition::Atom(ConditionAtom::HasTech("laser".into())),
            Condition::Atom(ConditionAtom::HasTech("plasma".into())),
        ])));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_condition_result_preserves_structure() {
        let (techs, mods, bldgs) = ctx_with(&["tech_a"], &[], &[]);
        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            buildings: &bldgs,
        };

        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::HasTech("tech_a".into())),
            Condition::Atom(ConditionAtom::HasTech("tech_b".into())),
        ]);
        let result = cond.evaluate(&ctx);

        if let ConditionResult::All {
            satisfied, children, ..
        } = &result
        {
            assert!(!satisfied);
            assert_eq!(children.len(), 2);
            assert!(children[0].is_satisfied());
            assert!(!children[1].is_satisfied());
        } else {
            panic!("Expected All result");
        }
    }
}
