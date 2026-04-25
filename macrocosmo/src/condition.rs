use std::collections::HashSet;

use bevy::prelude::{Component, ReflectComponent};
use bevy::reflect::Reflect;

/// Scope for condition evaluation.
#[derive(Clone, Debug, PartialEq, bevy::reflect::Reflect)]
pub enum ConditionScope {
    /// Search all scopes: ship -> planet -> system -> empire (backward-compatible default)
    Any,
    Empire,
    System,
    Planet,
    Ship,
}

/// The kind of atomic condition check.
#[derive(Clone, Debug, PartialEq, bevy::reflect::Reflect)]
pub enum AtomKind {
    HasTech(String),
    HasModifier(String),
    HasBuilding(String),
    HasFlag(String),

    // --- Diplomacy atoms (#322) ---
    /// Check if the relation state with the target is a specific value.
    TargetStateIs {
        state: String,
    },
    /// Check if the relation state with the target is one of several values.
    TargetStateIn {
        states: Vec<String>,
    },
    /// Check if standing with target is >= threshold.
    TargetStandingAtLeast {
        threshold: f64,
    },
    /// Check if actor's relative military power vs target is >= ratio.
    RelativePowerAtLeast {
        ratio: f64,
    },
    /// Check if target faction's allowed_diplomatic_options contains the given option id.
    TargetAllowsOption {
        option_id: String,
    },
    /// Check if the actor has a specific modifier (distinct from HasModifier which checks empire scope).
    ActorHasModifier {
        modifier_id: String,
    },
    /// Check if the actor holds the capital system of the target.
    ActorHoldsCapitalOfTarget,
    /// Check if the target's owned system count is <= n.
    TargetSystemCountAtMost {
        count: u32,
    },
    /// Check if the target attacked an actor core system within the given number of hexadies.
    TargetAttackedActorCoreWithin {
        hexadies: i64,
    },
}

/// Atomic condition that checks a single game-state predicate with an optional scope.
#[derive(Clone, Debug, PartialEq, bevy::reflect::Reflect)]
pub struct ConditionAtom {
    pub kind: AtomKind,
    pub scope: ConditionScope,
}

impl ConditionAtom {
    /// Create a HasTech atom with scope=Any (backward compatible).
    pub fn has_tech(id: impl Into<String>) -> Self {
        Self {
            kind: AtomKind::HasTech(id.into()),
            scope: ConditionScope::Any,
        }
    }

    /// Create a HasModifier atom with scope=Any (backward compatible).
    pub fn has_modifier(id: impl Into<String>) -> Self {
        Self {
            kind: AtomKind::HasModifier(id.into()),
            scope: ConditionScope::Any,
        }
    }

    /// Create a HasBuilding atom with scope=Any (backward compatible).
    pub fn has_building(id: impl Into<String>) -> Self {
        Self {
            kind: AtomKind::HasBuilding(id.into()),
            scope: ConditionScope::Any,
        }
    }

    /// Create a HasFlag atom with scope=Any.
    pub fn has_flag(id: impl Into<String>) -> Self {
        Self {
            kind: AtomKind::HasFlag(id.into()),
            scope: ConditionScope::Any,
        }
    }

    /// Create an atom with a specific scope.
    pub fn scoped(kind: AtomKind, scope: ConditionScope) -> Self {
        Self { kind, scope }
    }

    /// Human-readable reason string for UI tooltip display when the atom is not satisfied.
    pub fn display_message(&self) -> String {
        match &self.kind {
            AtomKind::HasTech(id) => format!("Requires technology: {id}"),
            AtomKind::HasModifier(id) => format!("Requires modifier: {id}"),
            AtomKind::HasBuilding(id) => format!("Requires building: {id}"),
            AtomKind::HasFlag(id) => format!("Requires flag: {id}"),
            AtomKind::TargetStateIs { state } => {
                format!("Relation must be: {state}")
            }
            AtomKind::TargetStateIn { states } => {
                format!("Relation must be one of: {}", states.join(", "))
            }
            AtomKind::TargetStandingAtLeast { threshold } => {
                format!("Standing must be at least {threshold}")
            }
            AtomKind::RelativePowerAtLeast { ratio } => {
                format!("Military power ratio must be at least {ratio}")
            }
            AtomKind::TargetAllowsOption { option_id } => {
                format!("Target must allow option: {option_id}")
            }
            AtomKind::ActorHasModifier { modifier_id } => {
                format!("Requires modifier: {modifier_id}")
            }
            AtomKind::ActorHoldsCapitalOfTarget => "Must hold target's capital".to_string(),
            AtomKind::TargetSystemCountAtMost { count } => {
                format!("Target must own at most {count} systems")
            }
            AtomKind::TargetAttackedActorCoreWithin { hexadies } => {
                format!("Target must have attacked a core system within {hexadies} hexadies")
            }
        }
    }

    // Backward-compatible constructors matching old enum variant syntax:
    /// Alias for `has_tech` — matches old `ConditionAtom::HasTech(id)` usage.
    #[allow(non_snake_case)]
    pub fn HasTech(id: impl Into<String>) -> Self {
        Self::has_tech(id)
    }

    /// Alias for `has_modifier` — matches old `ConditionAtom::HasModifier(id)` usage.
    #[allow(non_snake_case)]
    pub fn HasModifier(id: impl Into<String>) -> Self {
        Self::has_modifier(id)
    }

    /// Alias for `has_building` — matches old `ConditionAtom::HasBuilding(id)` usage.
    #[allow(non_snake_case)]
    pub fn HasBuilding(id: impl Into<String>) -> Self {
        Self::has_building(id)
    }
}

/// Composable condition tree. Used by structure prerequisites, event triggers, etc.
#[derive(Clone, Debug, PartialEq, bevy::reflect::Reflect)]
pub enum Condition {
    Atom(ConditionAtom),
    /// All children must be satisfied.
    All(Vec<Condition>),
    /// At least one child must be satisfied.
    Any(Vec<Condition>),
    /// Exactly one child must be satisfied.
    OneOf(Vec<Condition>),
    /// The child must NOT be satisfied.
    /// `Box<T>` is not `Reflect` in `bevy_reflect` 0.18 even when `T:
    /// Reflect`, so the inner condition is opaque; the outer `Not`
    /// variant tag is still visible.
    Not(#[reflect(ignore, default = "default_boxed_condition")] Box<Condition>),
}

/// Default value used by `#[reflect(ignore)]` on `Condition::Not`'s
/// boxed child. The reflection layer never reads this — it exists only
/// so `FromReflect` can synthesise an enum variant.
fn default_boxed_condition() -> Box<Condition> {
    Box::new(Condition::All(Vec::new()))
}

/// Scope-specific data for condition evaluation (buildings and flags at a particular scope).
pub struct ScopeData<'a> {
    pub flags: &'a HashSet<String>,
    pub buildings: &'a HashSet<String>,
}

/// Diplomacy-specific context for evaluating diplomacy condition atoms (#322).
///
/// Populated when evaluating conditions in a diplomatic context (e.g.,
/// DiplomaticOption.available, CasusBelli.evaluate). When absent, all
/// diplomacy atoms evaluate to `false`.
pub struct DiplomacyContext<'a> {
    /// The relation state from actor to target (e.g., "war", "peace").
    pub relation_state: &'a str,
    /// The standing value from actor to target ([-100.0, +100.0]).
    pub standing: f64,
    /// The actor's active modifiers (for `actor_has_modifier`).
    pub actor_modifiers: &'a HashSet<String>,
    /// The target faction's allowed diplomatic options (for `target_allows_option`).
    pub target_allowed_options: &'a HashSet<String>,
    /// The actor's military power score (for `relative_power_at_least`).
    pub actor_military_power: f64,
    /// The target's military power score (for `relative_power_at_least`).
    pub target_military_power: f64,
    /// Whether the actor holds the target's capital system (for `actor_holds_capital_of_target`).
    pub actor_holds_target_capital: bool,
    /// The number of systems the target owns (for `target_system_count_at_most`).
    pub target_system_count: u32,
    /// The elapsed hexadies since the target last attacked an actor core system.
    /// `None` if no attack has occurred (for `target_attacked_actor_core_within`).
    pub hexadies_since_target_attacked_actor_core: Option<i64>,
}

/// Context for evaluating conditions against current game state.
pub struct EvalContext<'a> {
    pub researched_techs: &'a HashSet<String>,
    pub active_modifiers: &'a HashSet<String>,
    pub empire: Option<ScopeData<'a>>,
    pub system: Option<ScopeData<'a>>,
    pub planet: Option<ScopeData<'a>>,
    pub ship: Option<ScopeData<'a>>,
    /// Diplomacy context, present when evaluating in a diplomatic scope.
    pub diplomacy: Option<DiplomacyContext<'a>>,
}

impl<'a> EvalContext<'a> {
    /// Convenience constructor that puts all buildings and flags into a single empire scope.
    /// This provides backward compatibility for existing call sites.
    pub fn flat(
        techs: &'a HashSet<String>,
        mods: &'a HashSet<String>,
        buildings: &'a HashSet<String>,
        flags: &'a HashSet<String>,
    ) -> Self {
        Self {
            researched_techs: techs,
            active_modifiers: mods,
            empire: Some(ScopeData { flags, buildings }),
            system: None,
            planet: None,
            ship: None,
            diplomacy: None,
        }
    }
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

impl EvalContext<'_> {
    /// Check if a building exists in a specific scope.
    fn has_building_in_scope(&self, scope: &ConditionScope, id: &str) -> bool {
        match scope {
            ConditionScope::Any => {
                // Search ship -> planet -> system -> empire
                for slot in [&self.ship, &self.planet, &self.system, &self.empire] {
                    if let Some(data) = slot {
                        if data.buildings.contains(id) {
                            return true;
                        }
                    }
                }
                false
            }
            ConditionScope::Empire => self
                .empire
                .as_ref()
                .is_some_and(|d| d.buildings.contains(id)),
            ConditionScope::System => self
                .system
                .as_ref()
                .is_some_and(|d| d.buildings.contains(id)),
            ConditionScope::Planet => self
                .planet
                .as_ref()
                .is_some_and(|d| d.buildings.contains(id)),
            ConditionScope::Ship => self.ship.as_ref().is_some_and(|d| d.buildings.contains(id)),
        }
    }

    /// Check if a flag exists in a specific scope.
    fn has_flag_in_scope(&self, scope: &ConditionScope, id: &str) -> bool {
        match scope {
            ConditionScope::Any => {
                // Search ship -> planet -> system -> empire
                for slot in [&self.ship, &self.planet, &self.system, &self.empire] {
                    if let Some(data) = slot {
                        if data.flags.contains(id) {
                            return true;
                        }
                    }
                }
                false
            }
            ConditionScope::Empire => self.empire.as_ref().is_some_and(|d| d.flags.contains(id)),
            ConditionScope::System => self.system.as_ref().is_some_and(|d| d.flags.contains(id)),
            ConditionScope::Planet => self.planet.as_ref().is_some_and(|d| d.flags.contains(id)),
            ConditionScope::Ship => self.ship.as_ref().is_some_and(|d| d.flags.contains(id)),
        }
    }
}

impl Condition {
    pub fn evaluate(&self, ctx: &EvalContext) -> ConditionResult {
        match self {
            Condition::Atom(atom) => {
                let satisfied = match &atom.kind {
                    AtomKind::HasTech(id) => ctx.researched_techs.contains(id),
                    AtomKind::HasModifier(id) => ctx.active_modifiers.contains(id),
                    AtomKind::HasBuilding(id) => ctx.has_building_in_scope(&atom.scope, id),
                    AtomKind::HasFlag(id) => ctx.has_flag_in_scope(&atom.scope, id),

                    // --- Diplomacy atoms (#322) ---
                    AtomKind::TargetStateIs { state } => ctx
                        .diplomacy
                        .as_ref()
                        .is_some_and(|d| d.relation_state.eq_ignore_ascii_case(state)),
                    AtomKind::TargetStateIn { states } => ctx.diplomacy.as_ref().is_some_and(|d| {
                        states
                            .iter()
                            .any(|s| d.relation_state.eq_ignore_ascii_case(s))
                    }),
                    AtomKind::TargetStandingAtLeast { threshold } => ctx
                        .diplomacy
                        .as_ref()
                        .is_some_and(|d| d.standing >= *threshold),
                    AtomKind::RelativePowerAtLeast { ratio } => {
                        ctx.diplomacy.as_ref().is_some_and(|d| {
                            if d.target_military_power <= 0.0 {
                                // If target has no military power, any positive actor power
                                // satisfies any ratio.
                                d.actor_military_power > 0.0 || *ratio <= 0.0
                            } else {
                                d.actor_military_power / d.target_military_power >= *ratio
                            }
                        })
                    }
                    AtomKind::TargetAllowsOption { option_id } => ctx
                        .diplomacy
                        .as_ref()
                        .is_some_and(|d| d.target_allowed_options.contains(option_id)),
                    AtomKind::ActorHasModifier { modifier_id } => ctx
                        .diplomacy
                        .as_ref()
                        .is_some_and(|d| d.actor_modifiers.contains(modifier_id)),
                    AtomKind::ActorHoldsCapitalOfTarget => ctx
                        .diplomacy
                        .as_ref()
                        .is_some_and(|d| d.actor_holds_target_capital),
                    AtomKind::TargetSystemCountAtMost { count } => ctx
                        .diplomacy
                        .as_ref()
                        .is_some_and(|d| d.target_system_count <= *count),
                    AtomKind::TargetAttackedActorCoreWithin { hexadies } => {
                        ctx.diplomacy.as_ref().is_some_and(|d| {
                            d.hexadies_since_target_attacked_actor_core
                                .is_some_and(|elapsed| elapsed <= *hexadies)
                        })
                    }
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

/// Flags attached to a specific entity scope (empire, system, planet, ship).
/// Used for scoped condition evaluation.
#[derive(Component, Default, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct ScopedFlags {
    pub flags: HashSet<String>,
}

impl ScopedFlags {
    /// Set a flag.
    pub fn set(&mut self, flag: &str) {
        self.flags.insert(flag.to_string());
    }

    /// Check if a flag is set.
    pub fn check(&self, flag: &str) -> bool {
        self.flags.contains(flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_empty_set() -> HashSet<String> {
        HashSet::new()
    }

    fn make_set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn flat_ctx<'a>(
        techs: &'a HashSet<String>,
        modifiers: &'a HashSet<String>,
        buildings: &'a HashSet<String>,
    ) -> EvalContext<'a> {
        // Use a static empty set for flags in legacy tests
        static EMPTY: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);
        EvalContext::flat(techs, modifiers, buildings, &EMPTY)
    }

    #[test]
    fn test_atom_has_tech() {
        let techs = make_set(&["laser_weapons"]);
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        let cond = Condition::Atom(ConditionAtom::has_tech("laser_weapons"));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::has_tech("plasma_weapons"));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_atom_has_modifier() {
        let techs = make_empty_set();
        let mods = make_set(&["war_economy"]);
        let bldgs = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        let cond = Condition::Atom(ConditionAtom::has_modifier("war_economy"));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::has_modifier("peace_economy"));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_atom_has_building() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let bldgs = make_set(&["shipyard"]);
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        let cond = Condition::Atom(ConditionAtom::has_building("shipyard"));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::has_building("factory"));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_all_combinator() {
        let techs = make_set(&["tech_a", "tech_b"]);
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Atom(ConditionAtom::has_tech("tech_b")),
        ]);
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Atom(ConditionAtom::has_tech("tech_c")),
        ]);
        assert!(!cond.evaluate(&ctx).is_satisfied());

        // Empty All is vacuously true
        let cond = Condition::All(vec![]);
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_any_combinator() {
        let techs = make_set(&["tech_a"]);
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        let cond = Condition::Any(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Atom(ConditionAtom::has_tech("tech_b")),
        ]);
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Any(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_x")),
            Condition::Atom(ConditionAtom::has_tech("tech_y")),
        ]);
        assert!(!cond.evaluate(&ctx).is_satisfied());

        // Empty Any is false
        let cond = Condition::Any(vec![]);
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_one_of_combinator() {
        let techs = make_set(&["tech_a", "tech_b"]);
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        // Two satisfied -> not exactly one
        let cond = Condition::OneOf(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Atom(ConditionAtom::has_tech("tech_b")),
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
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Atom(ConditionAtom::has_tech("tech_c")),
        ]);
        assert!(cond.evaluate(&ctx).is_satisfied());

        // None satisfied
        let cond = Condition::OneOf(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_x")),
            Condition::Atom(ConditionAtom::has_tech("tech_y")),
        ]);
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_not_combinator() {
        let techs = make_set(&["tech_a"]);
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        let cond = Condition::Not(Box::new(Condition::Atom(ConditionAtom::has_tech("tech_a"))));
        assert!(!cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Not(Box::new(Condition::Atom(ConditionAtom::has_tech("tech_b"))));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_nested_conditions() {
        let techs = make_set(&["laser", "shields"]);
        let mods = make_set(&["war_economy"]);
        let bldgs = make_set(&["shipyard"]);
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        // All(HasTech("laser"), Any(HasModifier("war_economy"), HasBuilding("factory")))
        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("laser")),
            Condition::Any(vec![
                Condition::Atom(ConditionAtom::has_modifier("war_economy")),
                Condition::Atom(ConditionAtom::has_building("factory")),
            ]),
        ]);
        assert!(cond.evaluate(&ctx).is_satisfied());

        // Not(All(HasTech("laser"), HasTech("plasma")))
        let cond = Condition::Not(Box::new(Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("laser")),
            Condition::Atom(ConditionAtom::has_tech("plasma")),
        ])));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_condition_result_preserves_structure() {
        let techs = make_set(&["tech_a"]);
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Atom(ConditionAtom::has_tech("tech_b")),
        ]);
        let result = cond.evaluate(&ctx);

        if let ConditionResult::All {
            satisfied,
            children,
            ..
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

    // --- New tests for scoped conditions ---

    #[test]
    fn test_has_flag_satisfied() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let flags = make_set(&["my_flag"]);
        let ctx = EvalContext::flat(&techs, &mods, &bldgs, &flags);

        let cond = Condition::Atom(ConditionAtom::has_flag("my_flag"));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_has_flag_unsatisfied() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let flags = make_empty_set();
        let ctx = EvalContext::flat(&techs, &mods, &bldgs, &flags);

        let cond = Condition::Atom(ConditionAtom::has_flag("missing_flag"));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_scope_chain_any_searches_all_scopes() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let empire_bldgs = make_empty_set();
        let empire_flags = make_empty_set();
        let system_bldgs = make_empty_set();
        let system_flags = make_set(&["system_flag"]);

        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            empire: Some(ScopeData {
                flags: &empire_flags,
                buildings: &empire_bldgs,
            }),
            system: Some(ScopeData {
                flags: &system_flags,
                buildings: &system_bldgs,
            }),
            planet: None,
            ship: None,
            diplomacy: None,
        };

        // Any scope should find the flag on the system scope
        let cond = Condition::Atom(ConditionAtom::has_flag("system_flag"));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_specific_scope_does_not_find_other_scope() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let empire_bldgs = make_empty_set();
        let empire_flags = make_set(&["empire_only_flag"]);
        let system_bldgs = make_empty_set();
        let system_flags = make_empty_set();

        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            empire: Some(ScopeData {
                flags: &empire_flags,
                buildings: &empire_bldgs,
            }),
            system: Some(ScopeData {
                flags: &system_flags,
                buildings: &system_bldgs,
            }),
            planet: None,
            ship: None,
            diplomacy: None,
        };

        // Check system scope specifically — flag is on empire, so should NOT be found
        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::HasFlag("empire_only_flag".into()),
            ConditionScope::System,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());

        // Check empire scope specifically — should be found
        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::HasFlag("empire_only_flag".into()),
            ConditionScope::Empire,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_scoped_flags_component() {
        let mut flags = ScopedFlags::default();
        assert!(!flags.check("test_flag"));

        flags.set("test_flag");
        assert!(flags.check("test_flag"));
        assert!(!flags.check("other_flag"));
    }

    // --- Diplomacy atom tests (#322) ---

    fn diplomacy_ctx<'a>(
        techs: &'a HashSet<String>,
        modifiers: &'a HashSet<String>,
        diplomacy: DiplomacyContext<'a>,
    ) -> EvalContext<'a> {
        static EMPTY: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);
        EvalContext {
            researched_techs: techs,
            active_modifiers: modifiers,
            empire: Some(ScopeData {
                flags: &EMPTY,
                buildings: &EMPTY,
            }),
            system: None,
            planet: None,
            ship: None,
            diplomacy: Some(diplomacy),
        }
    }

    fn default_diplomacy_context<'a>(
        relation_state: &'a str,
        standing: f64,
        actor_mods: &'a HashSet<String>,
        target_opts: &'a HashSet<String>,
    ) -> DiplomacyContext<'a> {
        DiplomacyContext {
            relation_state,
            standing,
            actor_modifiers: actor_mods,
            target_allowed_options: target_opts,
            actor_military_power: 100.0,
            target_military_power: 100.0,
            actor_holds_target_capital: false,
            target_system_count: 5,
            hexadies_since_target_attacked_actor_core: None,
        }
    }

    #[test]
    fn test_target_state_is() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let dc = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetStateIs {
                state: "war".into(),
            },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetStateIs {
                state: "peace".into(),
            },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_target_state_in() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let dc = default_diplomacy_context("peace", 0.0, &actor_mods, &target_opts);
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetStateIn {
                states: vec!["peace".into(), "neutral".into()],
            },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetStateIn {
                states: vec!["war".into(), "alliance".into()],
            },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_target_standing_at_least() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let dc = default_diplomacy_context("peace", 50.0, &actor_mods, &target_opts);
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetStandingAtLeast { threshold: 50.0 },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetStandingAtLeast { threshold: 51.0 },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_relative_power_at_least() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let mut dc = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        dc.actor_military_power = 200.0;
        dc.target_military_power = 100.0;
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::RelativePowerAtLeast { ratio: 2.0 },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::RelativePowerAtLeast { ratio: 2.1 },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_relative_power_target_zero() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let mut dc = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        dc.actor_military_power = 10.0;
        dc.target_military_power = 0.0;
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::RelativePowerAtLeast { ratio: 999.0 },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_target_allows_option() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_set(&["generic_negotiation"]);
        let dc = default_diplomacy_context("peace", 0.0, &actor_mods, &target_opts);
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetAllowsOption {
                option_id: "generic_negotiation".into(),
            },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetAllowsOption {
                option_id: "declare_war".into(),
            },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_actor_has_modifier() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_set(&["cb_broken_treaty_recent"]);
        let target_opts = make_empty_set();
        let dc = default_diplomacy_context("peace", 0.0, &actor_mods, &target_opts);
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::ActorHasModifier {
                modifier_id: "cb_broken_treaty_recent".into(),
            },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::ActorHasModifier {
                modifier_id: "nonexistent".into(),
            },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_actor_holds_capital_of_target() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let mut dc = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        dc.actor_holds_target_capital = true;
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::ActorHoldsCapitalOfTarget,
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_target_system_count_at_most() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let mut dc = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        dc.target_system_count = 2;
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetSystemCountAtMost { count: 2 },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetSystemCountAtMost { count: 1 },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_target_attacked_actor_core_within() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let mut dc = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        dc.hexadies_since_target_attacked_actor_core = Some(50);
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetAttackedActorCoreWithin { hexadies: 100 },
            ConditionScope::Any,
        ));
        assert!(cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetAttackedActorCoreWithin { hexadies: 30 },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_target_attacked_actor_core_within_never_attacked() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let dc = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        // hexadies_since_target_attacked_actor_core is None by default
        let ctx = diplomacy_ctx(&techs, &mods, dc);

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetAttackedActorCoreWithin { hexadies: 1000 },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_diplomacy_atoms_false_without_context() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let bldgs = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &bldgs);

        // All diplomacy atoms should return false when no DiplomacyContext is present
        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::TargetStateIs {
                state: "war".into(),
            },
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());

        let cond = Condition::Atom(ConditionAtom::scoped(
            AtomKind::ActorHoldsCapitalOfTarget,
            ConditionScope::Any,
        ));
        assert!(!cond.evaluate(&ctx).is_satisfied());
    }

    #[test]
    fn test_display_message() {
        let atom = ConditionAtom::has_tech("laser");
        assert_eq!(atom.display_message(), "Requires technology: laser");

        let atom = ConditionAtom::scoped(
            AtomKind::TargetStateIs {
                state: "war".into(),
            },
            ConditionScope::Any,
        );
        assert_eq!(atom.display_message(), "Relation must be: war");

        let atom = ConditionAtom::scoped(
            AtomKind::TargetSystemCountAtMost { count: 3 },
            ConditionScope::Any,
        );
        assert_eq!(atom.display_message(), "Target must own at most 3 systems");

        let atom = ConditionAtom::scoped(AtomKind::ActorHoldsCapitalOfTarget, ConditionScope::Any);
        assert_eq!(atom.display_message(), "Must hold target's capital");
    }
}
