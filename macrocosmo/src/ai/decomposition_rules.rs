//! Game-side decomposition rules for the `macrocosmo-ai` short layer.
//!
//! This module wires the abstract [`StaticDecompositionRegistry`]
//! ([`macrocosmo_ai::decomposition`]) up with concrete game macros:
//!
//! - **`colonize_system`** (macro) → `[deploy_deliverable(infra_core),
//!   colonize_planet]`. The colonization macro: drop a Core deliverable
//!   in the target system, then settle a planet.
//! - **`deploy_deliverable`** (macro) → `[build_deliverable,
//!   load_deliverable, reposition, unload_deliverable]`. The
//!   four-step deploy chain: build the deliverable at an owned system,
//!   load it onto a courier, move the courier to the target system,
//!   then unload it.
//!
//! The rules are pure functions (`fn(&Command, &PlanState, Tick) ->
//! Vec<Command>`) — they read the macro's params and synthesize the
//! primitive sequence. They do not know about Bevy `Entity`, `Resource`,
//! or anything game-engine-specific; the same params (`target_system`,
//! `target_planet`, `ship_*`, `definition_id`) flow through as opaque
//! [`CommandValue`]s.
//!
//! # Why pure functions
//!
//! Decomposition runs inside the short-term agent's `tick` and must be
//! deterministic for record/replay. By keeping rules as `fn` pointers
//! (not boxed closures with captured state), the registry stays
//! `Copy + Send + Sync`-friendly and rules trivially round-trip across
//! threads.
//!
//! # Courier sourcing
//!
//! The `deploy_deliverable` rule emits 4 primitives in fixed order. It
//! does **not** check whether a courier ship already exists — that
//! gating is the short-term agent's job (e.g. via `precondition_gate`
//! in F4) or a future `build_ship` pre-step. F3 just lays down the
//! macro shape; F4 wires the short layer to consume it.

use macrocosmo_ai::{
    Command, CommandValue, DecompositionRegistry, DecompositionRule, PlanState,
    StaticDecompositionRegistry, Tick,
};

use crate::ai::schema::ids::command as cmd_ids;

/// Build the default decomposition registry for game-side macros.
///
/// Today this registers two rules:
/// 1. `colonize_system` → [`expand_colonize_system`]
/// 2. `deploy_deliverable` → [`expand_deploy_deliverable`]
///
/// Game code wires the returned registry into `CampaignReactiveShort`
/// via the per-`ShortAgent` driver (`run_short_agents` in
/// [`super::short_agent_runtime`]). Pre-#449 PR2c the registry was
/// instead handed to `macrocosmo_ai::Orchestrator::with_decomposition`;
/// the engine-agnostic harness still exposes that API in
/// `macrocosmo-ai`'s scenario tests.
pub fn build_default_registry() -> StaticDecompositionRegistry {
    let mut reg = StaticDecompositionRegistry::new();
    reg.register(DecompositionRule::new(
        cmd_ids::colonize_system(),
        expand_colonize_system,
    ));
    reg.register(DecompositionRule::new(
        cmd_ids::deploy_deliverable(),
        expand_deploy_deliverable,
    ));
    reg
}

/// Expand a `colonize_system` macro into:
/// 1. `deploy_deliverable(infra_core, target_system, ship_*)`
/// 2. `colonize_planet(target_system, target_planet?, ship_*)`
///
/// Param transfer:
/// - `target_system` → propagated to both children.
/// - `target_planet` → forwarded to `colonize_planet` only (when present).
/// - `ship_count` / `ship_0..ship_n` → propagated to both (the short
///   layer or the consumer chooses which ship is the courier vs. the
///   colony ship; for the simple single-ship case both children see the
///   same ship list).
/// - `definition_id` is fixed to `"infra_core"` for the deploy step —
///   this matches the production Lua deliverable shipped in
///   `scripts/structures/infrastructure_core.lua`.
fn expand_colonize_system(macro_cmd: &Command, _ps: &PlanState, now: Tick) -> Vec<Command> {
    let issuer = macro_cmd.issuer;

    // 1. deploy_deliverable(infra_core, target_system, ships)
    let mut deploy = Command::new(cmd_ids::deploy_deliverable(), issuer, now)
        .with_param("definition_id", CommandValue::Str("infra_core".into()));
    deploy.priority = macro_cmd.priority;
    deploy.target = macro_cmd.target.clone();
    forward_param(&mut deploy, macro_cmd, "target_system");
    forward_ship_list(&mut deploy, macro_cmd);

    // 2. colonize_planet(target_system, target_planet?, ships)
    let mut colonize = Command::new(cmd_ids::colonize_planet(), issuer, now);
    colonize.priority = macro_cmd.priority;
    colonize.target = macro_cmd.target.clone();
    forward_param(&mut colonize, macro_cmd, "target_system");
    forward_param(&mut colonize, macro_cmd, "target_planet");
    forward_ship_list(&mut colonize, macro_cmd);

    vec![deploy, colonize]
}

/// Expand a `deploy_deliverable` macro into the 4-step deliverable
/// deploy chain:
/// 1. `build_deliverable(definition_id, target_system?)`
/// 2. `load_deliverable(target_system, ship_*)`
/// 3. `reposition(target_system, ship_*)` — courier moves to drop point
/// 4. `unload_deliverable(ship_*)` — emit at drop point
///
/// Param transfer:
/// - `definition_id` is required; absent → empty expansion (logged by
///   the short layer).
/// - `target_system` propagates to build/load/reposition (build picks
///   any owned system if absent; load needs the system holding the
///   stockpile).
/// - `ship_*` propagates to load/reposition/unload (build runs at a
///   shipyard, no courier needed).
///
/// # Courier sourcing (TODO)
///
/// This rule does **not** prepend a `build_ship` step when no courier
/// exists. That is a future enhancement — the short layer's
/// precondition gate (F4) is the right hook to delay
/// `load_deliverable` until a courier is available. For now F3 emits
/// the 4-step chain unconditionally, matching the contract the test
/// suite asserts.
fn expand_deploy_deliverable(macro_cmd: &Command, _ps: &PlanState, now: Tick) -> Vec<Command> {
    // Skip expansion if `definition_id` is missing — without it,
    // build/load/unload have no item to act on. Returning an empty
    // Vec leaves the short layer's macro slot empty so it cleans up
    // on the next tick.
    if macro_cmd.params.get("definition_id").is_none() {
        return Vec::new();
    }

    let issuer = macro_cmd.issuer;

    // 1. build_deliverable(definition_id, target_system?)
    let mut build = Command::new(cmd_ids::build_deliverable(), issuer, now);
    build.priority = macro_cmd.priority;
    build.target = macro_cmd.target.clone();
    forward_param(&mut build, macro_cmd, "definition_id");
    forward_param(&mut build, macro_cmd, "target_system");

    // 2. load_deliverable(target_system, ships)
    let mut load = Command::new(cmd_ids::load_deliverable(), issuer, now);
    load.priority = macro_cmd.priority;
    load.target = macro_cmd.target.clone();
    forward_param(&mut load, macro_cmd, "target_system");
    forward_param(&mut load, macro_cmd, "definition_id");
    forward_ship_list(&mut load, macro_cmd);

    // 3. reposition(target_system, ships) — courier moves to drop point
    let mut mv = Command::new(cmd_ids::reposition(), issuer, now);
    mv.priority = macro_cmd.priority;
    mv.target = macro_cmd.target.clone();
    forward_param(&mut mv, macro_cmd, "target_system");
    forward_ship_list(&mut mv, macro_cmd);

    // 4. unload_deliverable(ships)
    let mut unload = Command::new(cmd_ids::unload_deliverable(), issuer, now);
    unload.priority = macro_cmd.priority;
    unload.target = macro_cmd.target.clone();
    forward_param(&mut unload, macro_cmd, "definition_id");
    forward_ship_list(&mut unload, macro_cmd);

    vec![build, load, mv, unload]
}

/// Copy a single named param from the macro to the child if present.
fn forward_param(child: &mut Command, macro_cmd: &Command, key: &str) {
    if let Some(v) = macro_cmd.params.get(key) {
        child.params.insert(key.into(), v.clone());
    }
}

/// Copy the `ship_count` + `ship_0..ship_n` indexed list (the
/// convention used by `attack_target`/`survey_system`/`colonize_system`
/// emitters) from the macro to the child.
fn forward_ship_list(child: &mut Command, macro_cmd: &Command) {
    let count = match macro_cmd.params.get("ship_count") {
        Some(CommandValue::I64(n)) => *n,
        _ => return,
    };
    child
        .params
        .insert("ship_count".into(), CommandValue::I64(count));
    for i in 0..count.max(0) {
        let key = format!("ship_{i}");
        if let Some(v) = macro_cmd.params.get(key.as_str()) {
            child.params.insert(key.into(), v.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macrocosmo_ai::{CommandKindId, EntityRef, FactionId, SystemRef};

    fn faction() -> FactionId {
        FactionId(7)
    }

    fn make_colonize_system_macro() -> Command {
        Command::new(cmd_ids::colonize_system(), faction(), 100)
            .with_param("target_system", CommandValue::System(SystemRef(42)))
            .with_param("target_planet", CommandValue::Entity(EntityRef(99)))
            .with_param("ship_count", CommandValue::I64(1))
            .with_param("ship_0", CommandValue::Entity(EntityRef(7)))
    }

    fn make_deploy_deliverable_macro() -> Command {
        Command::new(cmd_ids::deploy_deliverable(), faction(), 200)
            .with_param("definition_id", CommandValue::Str("infra_core".into()))
            .with_param("target_system", CommandValue::System(SystemRef(42)))
            .with_param("ship_count", CommandValue::I64(1))
            .with_param("ship_0", CommandValue::Entity(EntityRef(7)))
    }

    #[test]
    fn colonize_system_expands_to_deploy_then_colonize_planet() {
        let macro_cmd = make_colonize_system_macro();
        let ps = PlanState::default();
        let primitives = expand_colonize_system(&macro_cmd, &ps, 100);

        // The macro expands to two commands; `deploy_deliverable` is
        // itself a macro that the registry will expand again when the
        // short layer pops it (multi-level decomposition).
        assert_eq!(primitives.len(), 2, "expected 2 child commands");
        assert_eq!(
            primitives[0].kind,
            cmd_ids::deploy_deliverable(),
            "first child must be deploy_deliverable"
        );
        assert_eq!(
            primitives[1].kind,
            cmd_ids::colonize_planet(),
            "second child must be colonize_planet"
        );
    }

    #[test]
    fn deploy_deliverable_expands_to_four_primitives_in_order() {
        let macro_cmd = make_deploy_deliverable_macro();
        let ps = PlanState::default();
        let primitives = expand_deploy_deliverable(&macro_cmd, &ps, 200);

        assert_eq!(primitives.len(), 4, "expected 4 primitive commands");
        assert_eq!(primitives[0].kind, cmd_ids::build_deliverable());
        assert_eq!(primitives[1].kind, cmd_ids::load_deliverable());
        assert_eq!(primitives[2].kind, cmd_ids::reposition());
        assert_eq!(primitives[3].kind, cmd_ids::unload_deliverable());
    }

    #[test]
    fn deploy_deliverable_skips_when_definition_id_missing() {
        // No `definition_id` — expansion must bail out so the short
        // layer doesn't queue a chain that can't possibly succeed.
        let macro_cmd = Command::new(cmd_ids::deploy_deliverable(), faction(), 1);
        let ps = PlanState::default();
        let primitives = expand_deploy_deliverable(&macro_cmd, &ps, 1);
        assert!(primitives.is_empty());
    }

    #[test]
    fn unknown_macro_returns_empty_via_registry() {
        // A `CommandKindId` that has no rule registered: looking it
        // up should return None, mirroring the "no decomposition"
        // contract the short layer relies on.
        let reg = build_default_registry();
        let kind = CommandKindId::from("not_a_real_macro");
        assert!(reg.lookup(&kind).is_none());
    }

    #[test]
    fn param_transfer_target_system_to_both_children() {
        let macro_cmd = make_colonize_system_macro();
        let ps = PlanState::default();
        let primitives = expand_colonize_system(&macro_cmd, &ps, 100);

        for (i, child) in primitives.iter().enumerate() {
            match child.params.get("target_system") {
                Some(CommandValue::System(s)) => {
                    assert_eq!(*s, SystemRef(42), "child {i} target_system mismatch");
                }
                _ => panic!("child {i} missing target_system"),
            }
        }
    }

    #[test]
    fn param_transfer_target_planet_only_to_colonize_planet() {
        let macro_cmd = make_colonize_system_macro();
        let ps = PlanState::default();
        let primitives = expand_colonize_system(&macro_cmd, &ps, 100);

        // deploy_deliverable should not carry target_planet (it doesn't
        // need it — the planet step is the next sibling).
        assert!(
            primitives[0].params.get("target_planet").is_none(),
            "deploy_deliverable child should not carry target_planet"
        );
        // colonize_planet must carry target_planet.
        match primitives[1].params.get("target_planet") {
            Some(CommandValue::Entity(e)) => assert_eq!(*e, EntityRef(99)),
            _ => panic!("colonize_planet missing target_planet"),
        }
    }

    #[test]
    fn param_transfer_ship_list_propagates() {
        let macro_cmd = make_colonize_system_macro();
        let ps = PlanState::default();
        let primitives = expand_colonize_system(&macro_cmd, &ps, 100);

        for (i, child) in primitives.iter().enumerate() {
            match child.params.get("ship_count") {
                Some(CommandValue::I64(n)) => assert_eq!(*n, 1, "child {i} ship_count"),
                _ => panic!("child {i} missing ship_count"),
            }
            match child.params.get("ship_0") {
                Some(CommandValue::Entity(e)) => assert_eq!(*e, EntityRef(7), "child {i} ship_0"),
                _ => panic!("child {i} missing ship_0"),
            }
        }
    }

    #[test]
    fn deploy_expansion_propagates_definition_id_to_build_load_unload() {
        let macro_cmd = make_deploy_deliverable_macro();
        let ps = PlanState::default();
        let primitives = expand_deploy_deliverable(&macro_cmd, &ps, 200);

        for child in [&primitives[0], &primitives[1], &primitives[3]] {
            match child.params.get("definition_id") {
                Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "infra_core"),
                _ => panic!("child {:?} missing definition_id", child.kind),
            }
        }
        // `reposition` doesn't need definition_id — make sure we
        // didn't accidentally tag it.
        assert!(primitives[2].params.get("definition_id").is_none());
    }

    #[test]
    fn deploy_expansion_propagates_target_system_to_build_load_reposition() {
        let macro_cmd = make_deploy_deliverable_macro();
        let ps = PlanState::default();
        let primitives = expand_deploy_deliverable(&macro_cmd, &ps, 200);

        for child in [&primitives[0], &primitives[1], &primitives[2]] {
            match child.params.get("target_system") {
                Some(CommandValue::System(s)) => assert_eq!(*s, SystemRef(42)),
                _ => panic!("child {:?} missing target_system", child.kind),
            }
        }
    }

    #[test]
    fn default_registry_has_both_rules() {
        let reg = build_default_registry();
        assert_eq!(reg.len(), 2, "expected 2 default rules");
        assert!(
            reg.lookup(&cmd_ids::colonize_system()).is_some(),
            "colonize_system rule not registered"
        );
        assert!(
            reg.lookup(&cmd_ids::deploy_deliverable()).is_some(),
            "deploy_deliverable rule not registered"
        );
    }

    /// End-to-end gameplay-value smoke test (F4): drive the
    /// `colonize_system` macro through `CampaignReactiveShort` against
    /// the production registry and observe the primitives that come
    /// out, tick by tick. Covers the full chain
    /// build → load → reposition → unload → colonize_planet.
    #[test]
    fn colonize_system_macro_decomposes_full_chain_via_short_agent() {
        use macrocosmo_ai::{
            AiBus, CampaignReactiveShort, ShortContext, ShortTermAgent, ShortTermInput, WarningMode,
        };

        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let registry = build_default_registry();
        let mut agent = CampaignReactiveShort::new();
        let mut plan = macrocosmo_ai::PlanState::default();

        // Active campaign whose ObjectiveId == "colonize_system" so
        // `make_command` synthesizes a macro the registry intercepts.
        // Note: `Campaign::new` defaults to `Pending`, but the short
        // agent only iterates `active_campaigns` — we pass the
        // already-active list directly so we don't need a state
        // transition here.
        let mut camp = macrocosmo_ai::campaign::Campaign::new(
            macrocosmo_ai::ObjectiveId::from("colonize_system"),
            0,
        );
        camp.state = macrocosmo_ai::campaign::CampaignState::Active;
        let active = [&camp];

        let mut emitted = Vec::new();
        for tick in 0..5 {
            let out = agent.tick(ShortTermInput {
                bus: &bus,
                faction: faction(),
                context: ShortContext::from("faction"),
                active_campaigns: &active,
                now: tick,
                plan_state: &mut plan,
                decomp: Some(&registry),
            });
            for cmd in out.commands {
                emitted.push(cmd.kind.clone());
            }
        }

        // The macro chain must surface as 5 primitives in this order:
        // build → load → reposition → unload → colonize_planet.
        // Note `make_command` doesn't carry `target_system`, so the
        // expansion runs on a bare macro — the kinds are still the
        // contract we test against (param transfer is covered by the
        // unit tests above).
        let expected = [
            cmd_ids::build_deliverable(),
            cmd_ids::load_deliverable(),
            cmd_ids::reposition(),
            cmd_ids::unload_deliverable(),
            cmd_ids::colonize_planet(),
        ];
        assert_eq!(emitted.len(), 5, "expected 5 primitives, got {emitted:?}");
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(&emitted[i], want, "primitive #{i} mismatch");
        }
    }

    #[test]
    fn colonize_system_children_inherit_issuer_priority_target() {
        let mut macro_cmd = make_colonize_system_macro();
        macro_cmd.priority = 0.85;

        let ps = PlanState::default();
        let primitives = expand_colonize_system(&macro_cmd, &ps, 100);

        for child in &primitives {
            assert_eq!(child.issuer, macro_cmd.issuer);
            assert_eq!(child.priority, 0.85);
            assert_eq!(child.target, macro_cmd.target);
            assert_eq!(child.at, 100);
        }
    }
}
