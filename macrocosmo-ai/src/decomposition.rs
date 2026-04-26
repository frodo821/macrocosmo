//! Command decomposition rules — per-`CommandKindId` `expand`
//! functions that turn one *macro* command into a sequence of
//! primitive commands the short-term agent will issue.
//!
//! This module provides:
//! - [`DecompositionRule`] — a single rule binding a macro
//!   `CommandKindId` to a pure expansion function.
//! - [`DecompositionRegistry`] — a trait the orchestrator threads
//!   through `ShortTermInput`. The trait is intentionally narrow
//!   (one `lookup` method) so game-side and test-side registries
//!   can plug in.
//! - [`StaticDecompositionRegistry`] — a `BTreeMap`-backed default
//!   impl. Game integrations register rules at startup and pass the
//!   registry to `Orchestrator::with_decomposition`.
//! - [`EmptyRegistry`] — the unit-struct registry used when no
//!   decomposition is desired (preserves legacy short-agent
//!   behavior).
//!
//! The `expand` function signature is `fn(&Command, &PlanState,
//! Tick) -> Vec<Command>`. It is a plain `fn` pointer (not a boxed
//! closure) so rules are `Copy + Send + Sync` for free; this is also
//! why `DecompositionRule` derives `Clone` and `Copy` cleanly.
//!
//! Decomposition logic itself does not run yet — F2 only wires the
//! registry through `ShortTermInput`. F3 fills the registry with
//! game-specific rules; F4 makes a decomposition-aware short agent
//! consume them.

use std::collections::BTreeMap;

use crate::agent::PlanState;
use crate::command::Command;
use crate::ids::CommandKindId;
use crate::time::Tick;

/// Pure expansion function. Given the macro command, the current
/// `PlanState`, and the current tick, returns the primitive commands
/// the short agent should enqueue for that macro.
///
/// The function does **not** mutate `PlanState` itself — the
/// short-term agent owns the mutation and decides where to insert
/// the returned primitives. Keeping `expand` pure makes rules
/// trivially testable and avoids subtle re-entrancy bugs.
pub type ExpandFn = fn(&Command, &PlanState, Tick) -> Vec<Command>;

/// A single decomposition rule. Binds a macro `CommandKindId` to an
/// `expand` function. `Clone` is `Arc<str>`-cheap; `Copy` is not
/// available because `CommandKindId` wraps an `Arc<str>`.
#[derive(Debug, Clone)]
pub struct DecompositionRule {
    /// The macro command kind this rule applies to. Looked up in the
    /// registry by exact `CommandKindId` match.
    pub macro_kind: CommandKindId,
    /// Pure expansion: macro command + current plan state + current
    /// tick → primitive commands.
    pub expand: ExpandFn,
}

impl DecompositionRule {
    /// Convenience ctor: `DecompositionRule::new(kind, expand_fn)`.
    pub fn new(macro_kind: impl Into<CommandKindId>, expand: ExpandFn) -> Self {
        Self {
            macro_kind: macro_kind.into(),
            expand,
        }
    }
}

/// Trait every decomposition registry implements. The orchestrator
/// borrows `&dyn DecompositionRegistry` and passes it through to the
/// short agent each tick.
///
/// The trait carries `Send + Sync` so registries can be stored on an
/// `Orchestrator` that itself moves across threads (matching
/// `LongTermAgent` / `MidTermAgent` / `ShortTermAgent`).
pub trait DecompositionRegistry: Send + Sync {
    /// Look up the rule for `kind`. `None` = no decomposition for
    /// that kind; the short agent should treat the command as
    /// already-primitive.
    fn lookup(&self, kind: &CommandKindId) -> Option<&DecompositionRule>;
}

/// Default registry: a `BTreeMap` keyed by `CommandKindId`.
///
/// Game integrations build one of these at startup and register
/// every `(macro_kind, expand_fn)` pair via [`Self::register`].
/// `BTreeMap` is chosen for deterministic iteration order — useful
/// when scenarios snapshot the registry for debugging.
#[derive(Debug, Default, Clone)]
pub struct StaticDecompositionRegistry {
    rules: BTreeMap<CommandKindId, DecompositionRule>,
}

impl StaticDecompositionRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a rule. If a rule with the same `macro_kind` already
    /// exists, it is replaced and the old rule is returned.
    pub fn register(&mut self, rule: DecompositionRule) -> Option<DecompositionRule> {
        self.rules.insert(rule.macro_kind.clone(), rule)
    }

    /// Convenience: register a rule built from `(kind, expand_fn)`.
    pub fn register_kind(
        &mut self,
        macro_kind: impl Into<CommandKindId>,
        expand: ExpandFn,
    ) -> Option<DecompositionRule> {
        self.register(DecompositionRule::new(macro_kind, expand))
    }

    /// Number of registered rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// `true` if no rules are registered.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

impl DecompositionRegistry for StaticDecompositionRegistry {
    fn lookup(&self, kind: &CommandKindId) -> Option<&DecompositionRule> {
        self.rules.get(kind)
    }
}

/// Singleton-style empty registry. `EmptyRegistry::lookup` always
/// returns `None`. Use this (or simply pass `None` to
/// `Orchestrator::with_decomposition`) to preserve the legacy
/// no-decomposition behavior.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyRegistry;

impl DecompositionRegistry for EmptyRegistry {
    fn lookup(&self, _kind: &CommandKindId) -> Option<&DecompositionRule> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CommandKindId, FactionId};

    fn noop_expand(_cmd: &Command, _ps: &PlanState, _now: Tick) -> Vec<Command> {
        Vec::new()
    }

    fn build_two(cmd: &Command, _ps: &PlanState, now: Tick) -> Vec<Command> {
        vec![
            Command::new(CommandKindId::from("step_a"), cmd.issuer, now),
            Command::new(CommandKindId::from("step_b"), cmd.issuer, now),
        ]
    }

    #[test]
    fn static_registry_lookup() {
        let mut reg = StaticDecompositionRegistry::new();
        assert!(reg.is_empty());
        let prev = reg.register_kind("colonize_system", build_two);
        assert!(prev.is_none(), "no prior rule expected");
        assert_eq!(reg.len(), 1);

        let kind = CommandKindId::from("colonize_system");
        let rule = reg.lookup(&kind).expect("rule present");
        assert_eq!(rule.macro_kind, kind);

        // Calling expand on the rule returns the function's output.
        let macro_cmd = Command::new(kind.clone(), FactionId(0), 7);
        let ps = PlanState::default();
        let primitives = (rule.expand)(&macro_cmd, &ps, 7);
        assert_eq!(primitives.len(), 2);
        assert_eq!(primitives[0].kind.as_str(), "step_a");
        assert_eq!(primitives[1].kind.as_str(), "step_b");
    }

    #[test]
    fn static_registry_replace_returns_old_rule() {
        let mut reg = StaticDecompositionRegistry::new();
        reg.register_kind("k", noop_expand);
        let prev = reg.register_kind("k", build_two);
        assert!(prev.is_some(), "replacing returns old rule");
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn empty_registry_returns_none() {
        let reg = EmptyRegistry;
        let kind = CommandKindId::from("anything");
        assert!(reg.lookup(&kind).is_none());
    }

    #[test]
    fn default_static_registry_returns_none() {
        let reg = StaticDecompositionRegistry::default();
        let kind = CommandKindId::from("anything");
        assert!(reg.lookup(&kind).is_none());
    }

    #[test]
    fn registry_is_dyn_compatible() {
        // Compile-time check that `&dyn DecompositionRegistry` is
        // valid (the orchestrator threads exactly this through).
        fn _accepts_dyn(_r: &dyn DecompositionRegistry) {}
        let reg = StaticDecompositionRegistry::new();
        _accepts_dyn(&reg);
        let empty = EmptyRegistry;
        _accepts_dyn(&empty);
    }
}
