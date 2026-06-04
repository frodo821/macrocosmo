use std::{
    collections::{BTreeMap, HashSet},
    sync::LazyLock,
};

/// Scope for condition evaluation.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub enum ConditionScope {
    /// Search all scopes: ship -> planet -> system -> empire.
    Any,
    Empire,
    System,
    Planet,
    Ship,
}

/// The kind of atomic condition check.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub enum AtomKind {
    HasTech(String),
    HasModifier(String),
    HasBuilding(String),
    HasFlag(String),
    TargetStateIs { state: String },
    TargetStateIn { states: Vec<String> },
    TargetStandingAtLeast { threshold: f64 },
    RelativePowerAtLeast { ratio: f64 },
    TargetAllowsOption { option_id: String },
    ActorHasModifier { modifier_id: String },
    ActorHoldsCapitalOfTarget,
    TargetSystemCountAtMost { count: u32 },
    TargetAttackedActorCoreWithin { hexadies: i64 },
    Predicate { predicate_id: String },
}

pub type ConditionPredicateId = String;
pub type ConditionArgKey = String;

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub enum ConditionArgValue {
    String(String),
    StringList(Vec<String>),
    Number(f64),
    Integer(i64),
    Unsigned(u32),
    Boolean(bool),
}

impl ConditionArgValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_string_list(&self) -> Option<&[String]> {
        match self {
            Self::StringList(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Unsigned(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct ConditionArgs {
    pub values: BTreeMap<ConditionArgKey, ConditionArgValue>,
}

impl ConditionArgs {
    pub fn new<K: Into<String>>(values: impl IntoIterator<Item = (K, ConditionArgValue)>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&ConditionArgValue> {
        self.values.get(key)
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(ConditionArgValue::as_str)
    }

    pub fn string_list(&self, key: &str) -> Option<&[String]> {
        self.get(key).and_then(ConditionArgValue::as_string_list)
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(ConditionArgValue::as_number)
    }

    pub fn i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(ConditionArgValue::as_i64)
    }

    pub fn u32(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(ConditionArgValue::as_u32)
    }

    pub fn boolean(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(ConditionArgValue::as_bool)
    }
}

/// Atomic condition that checks one game-state predicate with an optional scope.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct ConditionAtom {
    /// Legacy/debug view of the atom. Evaluation uses `predicate_id + args`.
    pub kind: AtomKind,
    pub predicate_id: ConditionPredicateId,
    pub args: ConditionArgs,
    pub scope: ConditionScope,
    pub fallback_label: Option<String>,
}

impl ConditionAtom {
    pub fn has_tech(id: impl Into<String>) -> Self {
        Self::scoped(AtomKind::HasTech(id.into()), ConditionScope::Any)
    }

    pub fn has_modifier(id: impl Into<String>) -> Self {
        Self::scoped(AtomKind::HasModifier(id.into()), ConditionScope::Any)
    }

    pub fn has_building(id: impl Into<String>) -> Self {
        Self::scoped(AtomKind::HasBuilding(id.into()), ConditionScope::Any)
    }

    pub fn has_flag(id: impl Into<String>) -> Self {
        Self::scoped(AtomKind::HasFlag(id.into()), ConditionScope::Any)
    }

    pub fn scoped(kind: AtomKind, scope: ConditionScope) -> Self {
        let (predicate_id, args, fallback_label) = atom_kind_parts(kind.clone());
        Self {
            kind,
            predicate_id,
            args,
            scope,
            fallback_label: Some(fallback_label),
        }
    }

    pub fn predicate(
        predicate_id: impl Into<String>,
        args: ConditionArgs,
        scope: ConditionScope,
    ) -> Self {
        let predicate_id = predicate_id.into();
        Self {
            kind: AtomKind::Predicate {
                predicate_id: predicate_id.clone(),
            },
            predicate_id,
            args,
            scope,
            fallback_label: None,
        }
    }

    pub fn display_message(&self) -> String {
        default_condition_predicate_registry().label_atom(self)
    }

    #[allow(non_snake_case)]
    pub fn HasTech(id: impl Into<String>) -> Self {
        Self::has_tech(id)
    }

    #[allow(non_snake_case)]
    pub fn HasModifier(id: impl Into<String>) -> Self {
        Self::has_modifier(id)
    }

    #[allow(non_snake_case)]
    pub fn HasBuilding(id: impl Into<String>) -> Self {
        Self::has_building(id)
    }
}

fn atom_kind_parts(kind: AtomKind) -> (String, ConditionArgs, String) {
    match kind {
        AtomKind::HasTech(id) => (
            "has_tech".to_string(),
            ConditionArgs::new([("id", ConditionArgValue::String(id.clone()))]),
            format!("Requires technology: {id}"),
        ),
        AtomKind::HasModifier(id) => (
            "has_modifier".to_string(),
            ConditionArgs::new([("id", ConditionArgValue::String(id.clone()))]),
            format!("Requires modifier: {id}"),
        ),
        AtomKind::HasBuilding(id) => (
            "has_building".to_string(),
            ConditionArgs::new([("id", ConditionArgValue::String(id.clone()))]),
            format!("Requires building: {id}"),
        ),
        AtomKind::HasFlag(id) => (
            "has_flag".to_string(),
            ConditionArgs::new([("id", ConditionArgValue::String(id.clone()))]),
            format!("Requires flag: {id}"),
        ),
        AtomKind::TargetStateIs { state } => (
            "target_state_is".to_string(),
            ConditionArgs::new([("state", ConditionArgValue::String(state.clone()))]),
            format!("Relation must be: {state}"),
        ),
        AtomKind::TargetStateIn { states } => (
            "target_state_in".to_string(),
            ConditionArgs::new([("states", ConditionArgValue::StringList(states.clone()))]),
            format!("Relation must be one of: {}", states.join(", ")),
        ),
        AtomKind::TargetStandingAtLeast { threshold } => (
            "target_standing_at_least".to_string(),
            ConditionArgs::new([("threshold", ConditionArgValue::Number(threshold))]),
            format!("Standing must be at least {threshold}"),
        ),
        AtomKind::RelativePowerAtLeast { ratio } => (
            "relative_power_at_least".to_string(),
            ConditionArgs::new([("ratio", ConditionArgValue::Number(ratio))]),
            format!("Military power ratio must be at least {ratio}"),
        ),
        AtomKind::TargetAllowsOption { option_id } => (
            "target_allows_option".to_string(),
            ConditionArgs::new([("option_id", ConditionArgValue::String(option_id.clone()))]),
            format!("Target must allow option: {option_id}"),
        ),
        AtomKind::ActorHasModifier { modifier_id } => (
            "actor_has_modifier".to_string(),
            ConditionArgs::new([(
                "modifier_id",
                ConditionArgValue::String(modifier_id.clone()),
            )]),
            format!("Requires modifier: {modifier_id}"),
        ),
        AtomKind::ActorHoldsCapitalOfTarget => (
            "actor_holds_capital_of_target".to_string(),
            ConditionArgs::empty(),
            "Must hold target's capital".to_string(),
        ),
        AtomKind::TargetSystemCountAtMost { count } => (
            "target_system_count_at_most".to_string(),
            ConditionArgs::new([("count", ConditionArgValue::Unsigned(count))]),
            format!("Target must own at most {count} systems"),
        ),
        AtomKind::TargetAttackedActorCoreWithin { hexadies } => (
            "target_attacked_actor_core_within".to_string(),
            ConditionArgs::new([("hexadies", ConditionArgValue::Integer(hexadies))]),
            format!("Target must have attacked a core system within {hexadies} hexadies"),
        ),
        AtomKind::Predicate { predicate_id } => {
            (predicate_id.clone(), ConditionArgs::empty(), predicate_id)
        }
    }
}

/// Composable condition tree.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub enum Condition {
    Atom(ConditionAtom),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    OneOf(Vec<Condition>),
    Not(
        #[cfg_attr(
            feature = "reflect",
            reflect(ignore, default = "default_boxed_condition")
        )]
        Box<Condition>,
    ),
}

pub fn default_boxed_condition() -> Box<Condition> {
    Box::new(Condition::All(Vec::new()))
}

/// Scope-specific data for condition evaluation.
pub struct ScopeData<'a> {
    pub flags: &'a HashSet<String>,
    pub buildings: &'a HashSet<String>,
}

/// Diplomacy-specific context for evaluating diplomacy condition atoms.
pub struct DiplomacyContext<'a> {
    pub relation_state: &'a str,
    pub standing: f64,
    pub actor_modifiers: &'a HashSet<String>,
    pub target_allowed_options: &'a HashSet<String>,
    pub actor_military_power: f64,
    pub target_military_power: f64,
    pub actor_holds_target_capital: bool,
    pub target_system_count: u32,
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
    pub diplomacy: Option<DiplomacyContext<'a>>,
}

pub trait ConditionEvalContext {
    fn has_tech(&self, id: &str) -> bool;
    fn has_modifier(&self, id: &str) -> bool;
    fn has_building_in_scope(&self, scope: &ConditionScope, id: &str) -> bool;
    fn has_flag_in_scope(&self, scope: &ConditionScope, id: &str) -> bool;
    fn diplomacy(&self) -> Option<&DiplomacyContext<'_>>;

    fn evaluate_custom_predicate(
        &self,
        _predicate_id: &str,
        _args: &ConditionArgs,
        _scope: &ConditionScope,
    ) -> Option<bool> {
        None
    }
}

impl<'a> EvalContext<'a> {
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

impl ConditionEvalContext for EvalContext<'_> {
    fn has_tech(&self, id: &str) -> bool {
        self.researched_techs.contains(id)
    }

    fn has_modifier(&self, id: &str) -> bool {
        self.active_modifiers.contains(id)
    }

    fn has_building_in_scope(&self, scope: &ConditionScope, id: &str) -> bool {
        match scope {
            ConditionScope::Any => {
                for slot in [&self.ship, &self.planet, &self.system, &self.empire] {
                    if let Some(data) = slot
                        && data.buildings.contains(id)
                    {
                        return true;
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

    fn has_flag_in_scope(&self, scope: &ConditionScope, id: &str) -> bool {
        match scope {
            ConditionScope::Any => {
                for slot in [&self.ship, &self.planet, &self.system, &self.empire] {
                    if let Some(data) = slot
                        && data.flags.contains(id)
                    {
                        return true;
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

    fn diplomacy(&self) -> Option<&DiplomacyContext<'_>> {
        self.diplomacy.as_ref()
    }
}

pub trait ConditionPredicate: Send + Sync + 'static {
    fn id(&self) -> &str;
    fn label(&self, args: &ConditionArgs) -> String;
    fn evaluate(
        &self,
        args: &ConditionArgs,
        scope: &ConditionScope,
        ctx: &dyn ConditionEvalContext,
    ) -> bool;
}

#[derive(Default)]
pub struct ConditionPredicateRegistry {
    predicates: BTreeMap<ConditionPredicateId, Box<dyn ConditionPredicate>>,
}

impl ConditionPredicateRegistry {
    pub fn register(&mut self, predicate: impl ConditionPredicate) {
        self.predicates
            .insert(predicate.id().to_string(), Box::new(predicate));
    }

    pub fn get(&self, id: &str) -> Option<&dyn ConditionPredicate> {
        self.predicates.get(id).map(|predicate| predicate.as_ref())
    }

    pub fn evaluate_atom(&self, atom: &ConditionAtom, ctx: &dyn ConditionEvalContext) -> bool {
        self.get(&atom.predicate_id)
            .map(|predicate| predicate.evaluate(&atom.args, &atom.scope, ctx))
            .or_else(|| ctx.evaluate_custom_predicate(&atom.predicate_id, &atom.args, &atom.scope))
            .unwrap_or(false)
    }

    pub fn label_atom(&self, atom: &ConditionAtom) -> String {
        self.get(&atom.predicate_id)
            .map(|predicate| predicate.label(&atom.args))
            .or_else(|| atom.fallback_label.clone())
            .unwrap_or_else(|| atom.predicate_id.clone())
    }
}

pub fn default_condition_predicate_registry() -> &'static ConditionPredicateRegistry {
    static REGISTRY: LazyLock<ConditionPredicateRegistry> = LazyLock::new(|| {
        let mut registry = ConditionPredicateRegistry::default();
        registry.register(BuiltinConditionPredicate {
            id: "has_tech",
            label: |args| format!("Requires technology: {}", args.string("id").unwrap_or("?")),
            evaluate: |args, _, ctx| args.string("id").is_some_and(|id| ctx.has_tech(id)),
        });
        registry.register(BuiltinConditionPredicate {
            id: "has_modifier",
            label: |args| format!("Requires modifier: {}", args.string("id").unwrap_or("?")),
            evaluate: |args, _, ctx| args.string("id").is_some_and(|id| ctx.has_modifier(id)),
        });
        registry.register(BuiltinConditionPredicate {
            id: "has_building",
            label: |args| format!("Requires building: {}", args.string("id").unwrap_or("?")),
            evaluate: |args, scope, ctx| {
                args.string("id")
                    .is_some_and(|id| ctx.has_building_in_scope(scope, id))
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "has_flag",
            label: |args| format!("Requires flag: {}", args.string("id").unwrap_or("?")),
            evaluate: |args, scope, ctx| {
                args.string("id")
                    .is_some_and(|id| ctx.has_flag_in_scope(scope, id))
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "target_state_is",
            label: |args| format!("Relation must be: {}", args.string("state").unwrap_or("?")),
            evaluate: |args, _, ctx| {
                let Some(state) = args.string("state") else {
                    return false;
                };
                ctx.diplomacy()
                    .is_some_and(|d| d.relation_state.eq_ignore_ascii_case(state))
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "target_state_in",
            label: |args| {
                let states = args
                    .string_list("states")
                    .map(|states| states.join(", "))
                    .unwrap_or_else(|| "?".to_string());
                format!("Relation must be one of: {states}")
            },
            evaluate: |args, _, ctx| {
                let Some(states) = args.string_list("states") else {
                    return false;
                };
                ctx.diplomacy().is_some_and(|d| {
                    states
                        .iter()
                        .any(|state| d.relation_state.eq_ignore_ascii_case(state))
                })
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "target_standing_at_least",
            label: |args| {
                format!(
                    "Standing must be at least {}",
                    args.number("threshold").unwrap_or_default()
                )
            },
            evaluate: |args, _, ctx| {
                let Some(threshold) = args.number("threshold") else {
                    return false;
                };
                ctx.diplomacy().is_some_and(|d| d.standing >= threshold)
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "relative_power_at_least",
            label: |args| {
                format!(
                    "Military power ratio must be at least {}",
                    args.number("ratio").unwrap_or_default()
                )
            },
            evaluate: |args, _, ctx| {
                let Some(ratio) = args.number("ratio") else {
                    return false;
                };
                ctx.diplomacy().is_some_and(|d| {
                    if d.target_military_power <= 0.0 {
                        d.actor_military_power > 0.0 || ratio <= 0.0
                    } else {
                        d.actor_military_power / d.target_military_power >= ratio
                    }
                })
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "target_allows_option",
            label: |args| {
                format!(
                    "Target must allow option: {}",
                    args.string("option_id").unwrap_or("?")
                )
            },
            evaluate: |args, _, ctx| {
                let Some(option_id) = args.string("option_id") else {
                    return false;
                };
                ctx.diplomacy()
                    .is_some_and(|d| d.target_allowed_options.contains(option_id))
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "actor_has_modifier",
            label: |args| {
                format!(
                    "Requires modifier: {}",
                    args.string("modifier_id").unwrap_or("?")
                )
            },
            evaluate: |args, _, ctx| {
                let Some(modifier_id) = args.string("modifier_id") else {
                    return false;
                };
                ctx.diplomacy()
                    .is_some_and(|d| d.actor_modifiers.contains(modifier_id))
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "actor_holds_capital_of_target",
            label: |_| "Must hold target's capital".to_string(),
            evaluate: |_, _, ctx| {
                ctx.diplomacy()
                    .is_some_and(|d| d.actor_holds_target_capital)
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "target_system_count_at_most",
            label: |args| {
                format!(
                    "Target must own at most {} systems",
                    args.u32("count").unwrap_or_default()
                )
            },
            evaluate: |args, _, ctx| {
                let Some(count) = args.u32("count") else {
                    return false;
                };
                ctx.diplomacy()
                    .is_some_and(|d| d.target_system_count <= count)
            },
        });
        registry.register(BuiltinConditionPredicate {
            id: "target_attacked_actor_core_within",
            label: |args| {
                format!(
                    "Target must have attacked a core system within {} hexadies",
                    args.i64("hexadies").unwrap_or_default()
                )
            },
            evaluate: |args, _, ctx| {
                let Some(hexadies) = args.i64("hexadies") else {
                    return false;
                };
                ctx.diplomacy().is_some_and(|d| {
                    d.hexadies_since_target_attacked_actor_core
                        .is_some_and(|elapsed| elapsed <= hexadies)
                })
            },
        });
        registry
    });
    &REGISTRY
}

struct BuiltinConditionPredicate {
    id: &'static str,
    label: fn(&ConditionArgs) -> String,
    evaluate: fn(&ConditionArgs, &ConditionScope, &dyn ConditionEvalContext) -> bool,
}

impl ConditionPredicate for BuiltinConditionPredicate {
    fn id(&self) -> &str {
        self.id
    }

    fn label(&self, args: &ConditionArgs) -> String {
        (self.label)(args)
    }

    fn evaluate(
        &self,
        args: &ConditionArgs,
        scope: &ConditionScope,
        ctx: &dyn ConditionEvalContext,
    ) -> bool {
        (self.evaluate)(args, scope, ctx)
    }
}

impl Condition {
    pub fn evaluate(&self, ctx: &EvalContext) -> ConditionResult {
        self.evaluate_with_registry(ctx, default_condition_predicate_registry())
    }

    pub fn evaluate_with_registry(
        &self,
        ctx: &dyn ConditionEvalContext,
        registry: &ConditionPredicateRegistry,
    ) -> ConditionResult {
        match self {
            Condition::Atom(atom) => {
                let satisfied = registry.evaluate_atom(atom, ctx);
                ConditionResult::Atom {
                    atom: atom.clone(),
                    satisfied,
                }
            }
            Condition::All(children) => {
                let children: Vec<_> = children
                    .iter()
                    .map(|c| c.evaluate_with_registry(ctx, registry))
                    .collect();
                let satisfied = children.iter().all(|r| r.is_satisfied());
                ConditionResult::All {
                    satisfied,
                    children,
                }
            }
            Condition::Any(children) => {
                let children: Vec<_> = children
                    .iter()
                    .map(|c| c.evaluate_with_registry(ctx, registry))
                    .collect();
                let satisfied = children.iter().any(|r| r.is_satisfied());
                ConditionResult::Any {
                    satisfied,
                    children,
                }
            }
            Condition::OneOf(children) => {
                let children: Vec<_> = children
                    .iter()
                    .map(|c| c.evaluate_with_registry(ctx, registry))
                    .collect();
                let satisfied_count = children.iter().filter(|r| r.is_satisfied()).count();
                ConditionResult::OneOf {
                    satisfied: satisfied_count == 1,
                    satisfied_count,
                    children,
                }
            }
            Condition::Not(child) => {
                let child = child.evaluate_with_registry(ctx, registry);
                let satisfied = !child.is_satisfied();
                ConditionResult::Not {
                    satisfied,
                    child: Box::new(child),
                }
            }
        }
    }
}

impl ConditionResult {
    pub fn is_satisfied(&self) -> bool {
        match self {
            ConditionResult::Atom { satisfied, .. }
            | ConditionResult::All { satisfied, .. }
            | ConditionResult::Any { satisfied, .. }
            | ConditionResult::OneOf { satisfied, .. }
            | ConditionResult::Not { satisfied, .. } => *satisfied,
        }
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
        static EMPTY: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);
        EvalContext::flat(techs, modifiers, buildings, &EMPTY)
    }

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

    struct PopulationContext {
        target_population: i64,
    }

    impl ConditionEvalContext for PopulationContext {
        fn has_tech(&self, _id: &str) -> bool {
            false
        }

        fn has_modifier(&self, _id: &str) -> bool {
            false
        }

        fn has_building_in_scope(&self, _scope: &ConditionScope, _id: &str) -> bool {
            false
        }

        fn has_flag_in_scope(&self, _scope: &ConditionScope, _id: &str) -> bool {
            false
        }

        fn diplomacy(&self) -> Option<&DiplomacyContext<'_>> {
            None
        }

        fn evaluate_custom_predicate(
            &self,
            predicate_id: &str,
            args: &ConditionArgs,
            _scope: &ConditionScope,
        ) -> Option<bool> {
            match predicate_id {
                "target_population_at_least" => Some(
                    args.i64("threshold")
                        .is_some_and(|n| self.target_population >= n),
                ),
                _ => None,
            }
        }
    }

    struct PopulationPredicate;

    impl ConditionPredicate for PopulationPredicate {
        fn id(&self) -> &str {
            "target_population_at_least"
        }

        fn label(&self, args: &ConditionArgs) -> String {
            format!(
                "Target population must be at least {}",
                args.i64("threshold").unwrap_or_default()
            )
        }

        fn evaluate(
            &self,
            args: &ConditionArgs,
            scope: &ConditionScope,
            ctx: &dyn ConditionEvalContext,
        ) -> bool {
            ctx.evaluate_custom_predicate(self.id(), args, scope)
                .unwrap_or(false)
        }
    }

    #[test]
    fn atoms_evaluate_against_flat_context() {
        let techs = make_set(&["laser_weapons"]);
        let mods = make_set(&["war_economy"]);
        let buildings = make_set(&["shipyard"]);
        let ctx = flat_ctx(&techs, &mods, &buildings);

        assert!(
            Condition::Atom(ConditionAtom::has_tech("laser_weapons"))
                .evaluate(&ctx)
                .is_satisfied()
        );
        assert!(
            !Condition::Atom(ConditionAtom::has_tech("plasma_weapons"))
                .evaluate(&ctx)
                .is_satisfied()
        );
        assert!(
            Condition::Atom(ConditionAtom::has_modifier("war_economy"))
                .evaluate(&ctx)
                .is_satisfied()
        );
        assert!(
            !Condition::Atom(ConditionAtom::has_modifier("peace_economy"))
                .evaluate(&ctx)
                .is_satisfied()
        );
        assert!(
            Condition::Atom(ConditionAtom::has_building("shipyard"))
                .evaluate(&ctx)
                .is_satisfied()
        );
        assert!(
            !Condition::Atom(ConditionAtom::has_building("factory"))
                .evaluate(&ctx)
                .is_satisfied()
        );
    }

    #[test]
    fn combinators_evaluate_and_preserve_structure() {
        let techs = make_set(&["tech_a", "tech_b"]);
        let mods = make_empty_set();
        let buildings = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &buildings);

        assert!(
            Condition::All(vec![
                Condition::Atom(ConditionAtom::has_tech("tech_a")),
                Condition::Atom(ConditionAtom::has_tech("tech_b")),
            ])
            .evaluate(&ctx)
            .is_satisfied()
        );
        assert!(Condition::All(vec![]).evaluate(&ctx).is_satisfied());

        assert!(
            Condition::Any(vec![
                Condition::Atom(ConditionAtom::has_tech("tech_a")),
                Condition::Atom(ConditionAtom::has_tech("tech_x")),
            ])
            .evaluate(&ctx)
            .is_satisfied()
        );
        assert!(!Condition::Any(vec![]).evaluate(&ctx).is_satisfied());

        let result = Condition::OneOf(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Atom(ConditionAtom::has_tech("tech_b")),
        ])
        .evaluate(&ctx);
        assert!(!result.is_satisfied());
        assert!(matches!(
            result,
            ConditionResult::OneOf {
                satisfied_count: 2,
                ..
            }
        ));

        assert!(
            Condition::OneOf(vec![
                Condition::Atom(ConditionAtom::has_tech("tech_a")),
                Condition::Atom(ConditionAtom::has_tech("tech_x")),
            ])
            .evaluate(&ctx)
            .is_satisfied()
        );
        assert!(
            Condition::Not(Box::new(Condition::Atom(ConditionAtom::has_tech("tech_x"))))
                .evaluate(&ctx)
                .is_satisfied()
        );

        let result = Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Atom(ConditionAtom::has_tech("tech_x")),
        ])
        .evaluate(&ctx);
        let ConditionResult::All {
            satisfied,
            children,
        } = result
        else {
            panic!("expected All result");
        };
        assert!(!satisfied);
        assert_eq!(children.len(), 2);
        assert!(children[0].is_satisfied());
        assert!(!children[1].is_satisfied());
    }

    #[test]
    fn nested_conditions_evaluate() {
        let techs = make_set(&["laser", "shields"]);
        let mods = make_set(&["war_economy"]);
        let buildings = make_set(&["shipyard"]);
        let ctx = flat_ctx(&techs, &mods, &buildings);

        assert!(
            Condition::All(vec![
                Condition::Atom(ConditionAtom::has_tech("laser")),
                Condition::Any(vec![
                    Condition::Atom(ConditionAtom::has_modifier("war_economy")),
                    Condition::Atom(ConditionAtom::has_building("factory")),
                ]),
            ])
            .evaluate(&ctx)
            .is_satisfied()
        );

        assert!(
            Condition::Not(Box::new(Condition::All(vec![
                Condition::Atom(ConditionAtom::has_tech("laser")),
                Condition::Atom(ConditionAtom::has_tech("plasma")),
            ])))
            .evaluate(&ctx)
            .is_satisfied()
        );
    }

    #[test]
    fn scoped_flags_search_expected_scopes() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let empty = make_empty_set();
        let empire_flags = make_set(&["empire_only_flag"]);
        let system_flags = make_set(&["system_flag"]);

        let ctx = EvalContext {
            researched_techs: &techs,
            active_modifiers: &mods,
            empire: Some(ScopeData {
                flags: &empire_flags,
                buildings: &empty,
            }),
            system: Some(ScopeData {
                flags: &system_flags,
                buildings: &empty,
            }),
            planet: None,
            ship: None,
            diplomacy: None,
        };

        assert!(
            Condition::Atom(ConditionAtom::has_flag("system_flag"))
                .evaluate(&ctx)
                .is_satisfied()
        );
        assert!(
            !Condition::Atom(ConditionAtom::scoped(
                AtomKind::HasFlag("empire_only_flag".into()),
                ConditionScope::System,
            ))
            .evaluate(&ctx)
            .is_satisfied()
        );
        assert!(
            Condition::Atom(ConditionAtom::scoped(
                AtomKind::HasFlag("empire_only_flag".into()),
                ConditionScope::Empire,
            ))
            .evaluate(&ctx)
            .is_satisfied()
        );
    }

    #[test]
    fn diplomacy_atoms_evaluate_context() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_set(&["cb_broken_treaty_recent"]);
        let target_opts = make_set(&["generic_negotiation"]);
        let mut diplomacy = default_diplomacy_context("war", 50.0, &actor_mods, &target_opts);
        diplomacy.actor_military_power = 200.0;
        diplomacy.target_military_power = 100.0;
        diplomacy.actor_holds_target_capital = true;
        diplomacy.target_system_count = 2;
        diplomacy.hexadies_since_target_attacked_actor_core = Some(50);
        let ctx = diplomacy_ctx(&techs, &mods, diplomacy);

        for atom in [
            AtomKind::TargetStateIs {
                state: "war".into(),
            },
            AtomKind::TargetStateIn {
                states: vec!["peace".into(), "war".into()],
            },
            AtomKind::TargetStandingAtLeast { threshold: 50.0 },
            AtomKind::RelativePowerAtLeast { ratio: 2.0 },
            AtomKind::TargetAllowsOption {
                option_id: "generic_negotiation".into(),
            },
            AtomKind::ActorHasModifier {
                modifier_id: "cb_broken_treaty_recent".into(),
            },
            AtomKind::ActorHoldsCapitalOfTarget,
            AtomKind::TargetSystemCountAtMost { count: 2 },
            AtomKind::TargetAttackedActorCoreWithin { hexadies: 100 },
        ] {
            assert!(
                Condition::Atom(ConditionAtom::scoped(atom, ConditionScope::Any))
                    .evaluate(&ctx)
                    .is_satisfied()
            );
        }
    }

    #[test]
    fn diplomacy_atoms_report_false_for_unsatisfied_cases() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let mut diplomacy = default_diplomacy_context("peace", 50.0, &actor_mods, &target_opts);
        diplomacy.actor_military_power = 200.0;
        diplomacy.target_military_power = 100.0;
        diplomacy.target_system_count = 2;
        diplomacy.hexadies_since_target_attacked_actor_core = Some(50);
        let ctx = diplomacy_ctx(&techs, &mods, diplomacy);

        for atom in [
            AtomKind::TargetStateIs {
                state: "war".into(),
            },
            AtomKind::TargetStateIn {
                states: vec!["war".into(), "alliance".into()],
            },
            AtomKind::TargetStandingAtLeast { threshold: 51.0 },
            AtomKind::RelativePowerAtLeast { ratio: 2.1 },
            AtomKind::TargetAllowsOption {
                option_id: "declare_war".into(),
            },
            AtomKind::ActorHasModifier {
                modifier_id: "nonexistent".into(),
            },
            AtomKind::TargetSystemCountAtMost { count: 1 },
            AtomKind::TargetAttackedActorCoreWithin { hexadies: 30 },
        ] {
            assert!(
                !Condition::Atom(ConditionAtom::scoped(atom, ConditionScope::Any))
                    .evaluate(&ctx)
                    .is_satisfied()
            );
        }
    }

    #[test]
    fn relative_power_handles_zero_target_power() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let mut diplomacy = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        diplomacy.actor_military_power = 10.0;
        diplomacy.target_military_power = 0.0;
        let ctx = diplomacy_ctx(&techs, &mods, diplomacy);

        assert!(
            Condition::Atom(ConditionAtom::scoped(
                AtomKind::RelativePowerAtLeast { ratio: 999.0 },
                ConditionScope::Any,
            ))
            .evaluate(&ctx)
            .is_satisfied()
        );
    }

    #[test]
    fn diplomacy_atoms_are_false_without_context() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let buildings = make_empty_set();
        let ctx = flat_ctx(&techs, &mods, &buildings);

        assert!(
            !Condition::Atom(ConditionAtom::scoped(
                AtomKind::TargetStateIs {
                    state: "war".into(),
                },
                ConditionScope::Any,
            ))
            .evaluate(&ctx)
            .is_satisfied()
        );
        assert!(
            !Condition::Atom(ConditionAtom::scoped(
                AtomKind::ActorHoldsCapitalOfTarget,
                ConditionScope::Any,
            ))
            .evaluate(&ctx)
            .is_satisfied()
        );
    }

    #[test]
    fn target_attacked_actor_core_within_is_false_when_never_attacked() {
        let techs = make_empty_set();
        let mods = make_empty_set();
        let actor_mods = make_empty_set();
        let target_opts = make_empty_set();
        let diplomacy = default_diplomacy_context("war", 0.0, &actor_mods, &target_opts);
        let ctx = diplomacy_ctx(&techs, &mods, diplomacy);

        assert!(
            !Condition::Atom(ConditionAtom::scoped(
                AtomKind::TargetAttackedActorCoreWithin { hexadies: 1000 },
                ConditionScope::Any,
            ))
            .evaluate(&ctx)
            .is_satisfied()
        );
    }

    #[test]
    fn display_message_covers_common_atoms() {
        assert_eq!(
            ConditionAtom::has_tech("laser").display_message(),
            "Requires technology: laser"
        );
        assert_eq!(
            ConditionAtom::scoped(
                AtomKind::TargetStateIs {
                    state: "war".into(),
                },
                ConditionScope::Any,
            )
            .display_message(),
            "Relation must be: war"
        );
        assert_eq!(
            ConditionAtom::scoped(
                AtomKind::TargetSystemCountAtMost { count: 3 },
                ConditionScope::Any,
            )
            .display_message(),
            "Target must own at most 3 systems"
        );
        assert_eq!(
            ConditionAtom::scoped(AtomKind::ActorHoldsCapitalOfTarget, ConditionScope::Any)
                .display_message(),
            "Must hold target's capital"
        );
    }

    #[test]
    fn custom_predicates_can_be_registered_and_evaluated() {
        let mut registry = ConditionPredicateRegistry::default();
        registry.register(PopulationPredicate);
        let ctx = PopulationContext {
            target_population: 800,
        };
        let atom = ConditionAtom::predicate(
            "target_population_at_least",
            ConditionArgs::new([("threshold", ConditionArgValue::Integer(500))]),
            ConditionScope::Planet,
        );

        assert_eq!(
            registry.label_atom(&atom),
            "Target population must be at least 500"
        );
        assert!(
            Condition::Atom(atom.clone())
                .evaluate_with_registry(&ctx, &registry)
                .is_satisfied()
        );

        let failing = ConditionAtom::predicate(
            "target_population_at_least",
            ConditionArgs::new([("threshold", ConditionArgValue::Integer(900))]),
            ConditionScope::Planet,
        );
        assert!(
            !Condition::Atom(failing)
                .evaluate_with_registry(&ctx, &registry)
                .is_satisfied()
        );
    }

    #[test]
    fn unregistered_predicates_can_defer_to_context_or_fallback_false() {
        let ctx = PopulationContext {
            target_population: 800,
        };
        let registry = ConditionPredicateRegistry::default();

        let context_owned = ConditionAtom::predicate(
            "target_population_at_least",
            ConditionArgs::new([("threshold", ConditionArgValue::Integer(500))]),
            ConditionScope::Planet,
        );
        assert!(
            Condition::Atom(context_owned)
                .evaluate_with_registry(&ctx, &registry)
                .is_satisfied()
        );

        let mut unknown = ConditionAtom::predicate(
            "unknown_game_predicate",
            ConditionArgs::empty(),
            ConditionScope::Any,
        );
        unknown.fallback_label = Some("Unknown game predicate".to_string());
        assert_eq!(registry.label_atom(&unknown), "Unknown game predicate");
        assert!(
            !Condition::Atom(unknown)
                .evaluate_with_registry(&ctx, &registry)
                .is_satisfied()
        );
    }
}
