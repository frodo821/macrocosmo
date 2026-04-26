//! Default short-term agent — emits one command per active campaign.
//!
//! The default strategy is intentionally minimal: for every active
//! campaign, emit a single `Command` whose kind is derived from the
//! campaign's objective id. This gives the orchestrator's integration
//! test suite something to observe without prescribing command
//! semantics (those belong to the game layer).
//!
//! Game-side short-term agents (FleetShort / ColonyShort) replace this
//! with context-specific logic. See `docs/ai-three-layer.md`
//! §ShortTermDefault.

use ahash::AHashMap;

use crate::agent::{PlanState, ShortTermAgent, ShortTermInput, ShortTermOutput};
use crate::command::{Command, CommandValue};
use crate::decomposition::DecompositionRegistry;
use crate::ids::{CommandKindId, ObjectiveId};
use crate::time::Tick;

/// Precondition gate: returns `true` to allow draining the next
/// primitive from a `PlanState` slot, `false` to hold the slot until
/// the next tick.
///
/// Today this is a stateless `fn` pointer (not a closure) so the
/// agent stays trivially `Default`/`Send + Sync`. It receives the full
/// `PlanState` plus the current tick — enough to inspect any slot.
///
/// **F4 does not use the gate's return value**: the default is
/// [`always_allow_gate`] which returns `true` unconditionally. The
/// hook is in place so future precondition gating (e.g. "hold
/// `load_deliverable` until `build_deliverable` has actually completed
/// in the bus") can plug in without touching the orchestrator wiring.
///
/// TODO(#TBD): real precondition gating will need richer context than
/// `PlanState` alone — it likely needs the `AiBus` or a per-slot
/// "expected fact" set. The current signature is a placeholder.
pub type PreconditionGate = fn(&PlanState, Tick) -> bool;

/// Default precondition gate: always allow draining (returns `true`).
/// Use this when no gating is desired (the F4 default).
pub fn always_allow_gate(_ps: &PlanState, _now: Tick) -> bool {
    true
}

/// Config for [`CampaignReactiveShort`].
#[derive(Debug, Clone)]
pub struct ShortTermDefaultConfig {
    /// Optional prefix prepended to the synthesized command kind.
    /// Empty by default (= use objective id as-is).
    pub kind_prefix: String,
    /// When `true`, fire commands proportional to each active
    /// campaign's `weight` via fractional accumulator scheduling.
    /// When `false` (default), emit one command per active campaign
    /// per tick (legacy behavior, weight ignored).
    ///
    /// Scheme (when `true`):
    /// 1. Each tick, for every active campaign, accumulator += weight.
    /// 2. While accumulator >= 1.0, emit a command and decrement by 1.0.
    /// 3. Capped by `max_commands_per_tick` across all campaigns.
    ///
    /// Result: a campaign with weight `0.9` fires roughly 3× as often
    /// as one with weight `0.3`. Total throughput per tick equals the
    /// sum of weights (capped).
    pub priority_weighted: bool,
    /// Hard cap on total commands emitted per tick across all
    /// campaigns when `priority_weighted` is on. `usize::MAX` =
    /// unlimited. Default `usize::MAX`.
    pub max_commands_per_tick: usize,
}

impl Default for ShortTermDefaultConfig {
    fn default() -> Self {
        Self {
            kind_prefix: String::new(),
            priority_weighted: false,
            max_commands_per_tick: usize::MAX,
        }
    }
}

/// Default short-term agent: emits commands per active campaign.
///
/// In legacy mode (`priority_weighted = false`) every active campaign
/// fires once per tick. In weighted mode (`priority_weighted = true`)
/// per-campaign accumulators allocate command budget proportional to
/// `Campaign.weight`.
///
/// # Macro decomposition (F4)
///
/// When `ShortTermInput.decomp` is `Some(_)`, every command produced
/// this tick is post-processed: macros (commands whose `kind` matches
/// a registered [`crate::decomposition::DecompositionRule`]) are
/// recursively expanded into primitives, the primitives are stored in
/// the per-`ShortContext` [`PlanState`], and the macro itself is **not
/// emitted**. Existing slots in `PlanState` are then drained one
/// primitive per tick (per slot), gated by [`Self::precondition_gate`].
///
/// When `decomp` is `None` (legacy / no-registry path), behavior is
/// unchanged — all commands flow through verbatim and `plan_state` /
/// `precondition_gate` are untouched.
#[derive(Debug)]
pub struct CampaignReactiveShort {
    pub config: ShortTermDefaultConfig,
    /// Persistent per-campaign accumulators for weighted scheduling.
    accumulators: AHashMap<ObjectiveId, f64>,
    /// Pluggable per-tick gate that decides whether `PlanState`
    /// drainage is allowed this tick. Defaults to
    /// [`always_allow_gate`] (= unconditional yes).
    ///
    /// Future precondition-gating logic (e.g. "delay `load_deliverable`
    /// until `build_deliverable` finishes") plugs in here without
    /// touching the orchestrator or game-side wiring.
    pub precondition_gate: PreconditionGate,
}

impl Default for CampaignReactiveShort {
    fn default() -> Self {
        Self {
            config: ShortTermDefaultConfig::default(),
            accumulators: AHashMap::new(),
            precondition_gate: always_allow_gate,
        }
    }
}

impl CampaignReactiveShort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: ShortTermDefaultConfig) -> Self {
        self.config = config;
        self
    }

    /// Install a custom [`PreconditionGate`]. Returns `self` for
    /// builder chaining. Pass [`always_allow_gate`] to revert.
    pub fn with_precondition_gate(mut self, gate: PreconditionGate) -> Self {
        self.precondition_gate = gate;
        self
    }

    fn make_command(
        &self,
        campaign: &crate::campaign::Campaign,
        faction: crate::ids::FactionId,
        now: crate::time::Tick,
    ) -> Command {
        let kind = if self.config.kind_prefix.is_empty() {
            CommandKindId::from(campaign.id.as_str())
        } else {
            CommandKindId::from(format!(
                "{}:{}",
                self.config.kind_prefix,
                campaign.id.as_str()
            ))
        };
        let mut cmd = Command::new(kind, faction, now).with_param("campaign", campaign.id.as_str());
        if let Some(src) = &campaign.source_intent {
            cmd = cmd.with_param("source_intent", src.as_str());
        }
        cmd
    }
}

impl ShortTermAgent for CampaignReactiveShort {
    fn tick(&mut self, input: ShortTermInput<'_>) -> ShortTermOutput {
        // Phase 1: build the raw command list using the existing
        // campaign-reactive logic. This is the legacy path — every
        // active campaign emits one (or many, in weighted mode)
        // commands. Decomposition does not change this stage; it
        // only post-processes the result.
        let raw = self.build_raw_commands(&input);

        // Phase 2: post-process. If a decomposition registry is
        // attached, every macro in `raw` is intercepted and replaced
        // by its primitive sequence (queued in `plan_state`). The
        // macro itself is dropped. Primitives flow through verbatim.
        // Then drained heads (one per slot, gated) are appended.
        let commands = match input.decomp {
            Some(reg) => self.intercept_and_drain(reg, input.plan_state, raw, input.now),
            None => raw,
        };

        ShortTermOutput { commands }
    }
}

impl CampaignReactiveShort {
    /// Build the raw, undecomposed command list for one tick. This is
    /// the legacy emit logic, factored out so the decomposition path
    /// can post-process its output without changing what gets built.
    fn build_raw_commands(&mut self, input: &ShortTermInput<'_>) -> Vec<Command> {
        let mut commands = Vec::new();

        if !self.config.priority_weighted {
            for campaign in input.active_campaigns {
                commands.push(self.make_command(campaign, input.faction, input.now));
            }
        } else {
            // Weighted mode: accumulate, then drain integer-many
            // commands per campaign in declaration order until cap.
            let mut emitted = 0usize;
            for campaign in input.active_campaigns {
                let acc = self.accumulators.entry(campaign.id.clone()).or_insert(0.0);
                *acc += campaign.weight.max(0.0);
                let mut fire = 0usize;
                while *acc >= 1.0 && emitted + fire < self.config.max_commands_per_tick {
                    *acc -= 1.0;
                    fire += 1;
                }
                for _ in 0..fire {
                    commands.push(self.make_command(campaign, input.faction, input.now));
                }
                emitted += fire;
            }

            // Garbage-collect accumulators for campaigns no longer
            // active (avoids unbounded growth in long scenarios).
            let active_ids: ahash::AHashSet<&ObjectiveId> =
                input.active_campaigns.iter().map(|c| &c.id).collect();
            self.accumulators.retain(|k, _| active_ids.contains(k));
        }

        commands
    }

    /// Post-process the raw command list against a decomposition
    /// registry:
    ///
    /// 1. For each command in `raw`:
    ///    - If its kind has a registered macro rule **and** there is
    ///      no in-flight slot already queued for that
    ///      `(kind, objective)`, recursively expand the macro into
    ///      primitives (eager flatten — every macro encountered along
    ///      the way is itself looked up and expanded) and push the
    ///      flat primitive sequence into `plan_state`. The macro
    ///      itself is **not** emitted.
    ///    - If its kind has a registered macro rule but its slot is
    ///      already queued, drop it (the existing slot will continue
    ///      draining).
    ///    - If its kind is not a macro, emit it verbatim.
    ///
    /// 2. After processing all raw commands, drain at most one head
    ///    primitive per non-empty `plan_state` slot, **gated by**
    ///    `self.precondition_gate(&plan_state, now)`. When the gate
    ///    returns `false`, the entire drainage step is skipped this
    ///    tick.
    ///
    /// 3. Empty slots are removed from `plan_state` so the
    ///    serialization snapshot stays compact and so future macro
    ///    re-emissions for the same `(kind, objective)` can re-seed
    ///    the slot.
    fn intercept_and_drain(
        &self,
        reg: &dyn DecompositionRegistry,
        plan_state: &mut PlanState,
        raw: Vec<Command>,
        now: Tick,
    ) -> Vec<Command> {
        let mut output = Vec::with_capacity(raw.len());

        // Step 1: dispatch each raw command — primitive vs macro.
        for cmd in raw {
            if reg.lookup(&cmd.kind).is_some() {
                let objective = objective_id_for(&cmd);
                let key = (cmd.kind.clone(), objective);

                // If we already have a slot for this macro/objective,
                // skip re-expansion — let the existing slot drain
                // first. This avoids re-pushing the entire chain
                // every tick the campaign re-emits the macro.
                if plan_state
                    .pending
                    .get(&key)
                    .is_some_and(|v| !v.is_empty())
                {
                    continue;
                }

                let mut primitives = Vec::new();
                expand_recursive(reg, &cmd, plan_state, now, &mut primitives);
                if !primitives.is_empty() {
                    plan_state.pending.insert(key, primitives);
                }
            } else {
                // Primitive — emit verbatim.
                output.push(cmd);
            }
        }

        // Step 2: drain one head per non-empty slot, gated.
        let allow = (self.precondition_gate)(plan_state, now);
        if allow {
            // Collect keys first so we don't borrow `plan_state`
            // while mutating it.
            let keys: Vec<_> = plan_state.pending.keys().cloned().collect();
            for key in keys {
                if let Some(slot) = plan_state.pending.get_mut(&key)
                    && !slot.is_empty()
                {
                    let head = slot.remove(0);
                    output.push(head);
                }
            }
        }

        // Step 3: GC empty slots so PlanState stays compact and
        // future macro re-emissions can re-seed.
        plan_state.pending.retain(|_, v| !v.is_empty());

        output
    }
}

/// Recursively expand `cmd` against `reg`. Macros are looked up and
/// their `expand` results are walked again; primitives are appended
/// to `out`. A small depth guard (16) prevents accidental cycles in
/// pathological registries.
fn expand_recursive(
    reg: &dyn DecompositionRegistry,
    cmd: &Command,
    ps: &PlanState,
    now: Tick,
    out: &mut Vec<Command>,
) {
    fn inner(
        reg: &dyn DecompositionRegistry,
        cmd: &Command,
        ps: &PlanState,
        now: Tick,
        out: &mut Vec<Command>,
        depth: usize,
    ) {
        if depth >= 16 {
            // Unreachable in practice (no real macro chain is this
            // deep) but a hard cap prevents pathological registries
            // from looping forever.
            return;
        }
        match reg.lookup(&cmd.kind) {
            Some(rule) => {
                let children = (rule.expand)(cmd, ps, now);
                for child in children {
                    inner(reg, &child, ps, now, out, depth + 1);
                }
            }
            None => {
                out.push(cmd.clone());
            }
        }
    }
    inner(reg, cmd, ps, now, out, 0);
}

/// Derive the `ObjectiveId` for a macro's `PlanState` slot:
///
/// 1. Prefer the `campaign` param (set by [`CampaignReactiveShort::make_command`]
///    when the macro originated from a campaign).
/// 2. Otherwise fall back to the macro's `kind` as the objective —
///    this keeps slot keying deterministic for hand-emitted macros
///    that were not produced by a campaign (e.g. game-side
///    `SimpleNpcPolicy`).
fn objective_id_for(cmd: &Command) -> ObjectiveId {
    if let Some(CommandValue::Str(s)) = cmd.params.get("campaign") {
        return ObjectiveId::from(s.as_ref());
    }
    ObjectiveId::from(cmd.kind.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::PlanState;
    use crate::bus::AiBus;
    use crate::campaign::{Campaign, CampaignState};
    use crate::ids::{FactionId, ObjectiveId, ShortContext};
    use crate::warning::WarningMode;

    #[test]
    fn emits_one_command_per_active_campaign() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut c1 = Campaign::new(ObjectiveId::from("pursue_metric:econ"), 0);
        c1.state = CampaignState::Active;
        let mut c2 = Campaign::new(ObjectiveId::from("preserve_metric:stockpile"), 0);
        c2.state = CampaignState::Active;
        let active = [&c1, &c2];
        let mut agent = CampaignReactiveShort::new();
        let mut plan = PlanState::default();
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(7),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 5,
            plan_state: &mut plan,
            decomp: None,
        });
        assert_eq!(out.commands.len(), 2);
        assert_eq!(out.commands[0].issuer, FactionId(7));
        assert_eq!(out.commands[0].kind.as_str(), "pursue_metric:econ");
        assert_eq!(out.commands[1].kind.as_str(), "preserve_metric:stockpile");
    }

    #[test]
    fn kind_prefix_applied() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut c = Campaign::new(ObjectiveId::from("expand"), 0);
        c.state = CampaignState::Active;
        let active = [&c];
        let mut agent = CampaignReactiveShort::new().with_config(ShortTermDefaultConfig {
            kind_prefix: "default_short".to_string(),
            ..ShortTermDefaultConfig::default()
        });
        let mut plan = PlanState::default();
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 1,
            plan_state: &mut plan,
            decomp: None,
        });
        assert_eq!(out.commands[0].kind.as_str(), "default_short:expand");
    }

    #[test]
    fn no_commands_when_no_active_campaigns() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut agent = CampaignReactiveShort::new();
        let mut plan = PlanState::default();
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &[],
            now: 1,
            plan_state: &mut plan,
            decomp: None,
        });
        assert_eq!(out.commands.len(), 0);
    }

    // ---------------------------------------------------------------
    // F4: macro decomposition + plan-state drainage
    // ---------------------------------------------------------------

    use crate::decomposition::{DecompositionRule, StaticDecompositionRegistry};
    use crate::ids::CommandKindId;

    /// Test rule: `m_outer` macro → [m_inner, p_z]. `m_inner` is itself
    /// a macro: → [p_a, p_b]. So a fully-flattened expansion of
    /// `m_outer` is `[p_a, p_b, p_z]` (three primitives).
    fn make_two_level_registry() -> StaticDecompositionRegistry {
        fn outer_expand(cmd: &Command, _ps: &PlanState, now: Tick) -> Vec<Command> {
            vec![
                Command::new(CommandKindId::from("m_inner"), cmd.issuer, now),
                Command::new(CommandKindId::from("p_z"), cmd.issuer, now),
            ]
        }
        fn inner_expand(cmd: &Command, _ps: &PlanState, now: Tick) -> Vec<Command> {
            vec![
                Command::new(CommandKindId::from("p_a"), cmd.issuer, now),
                Command::new(CommandKindId::from("p_b"), cmd.issuer, now),
            ]
        }

        let mut reg = StaticDecompositionRegistry::new();
        reg.register(DecompositionRule::new(
            CommandKindId::from("m_outer"),
            outer_expand,
        ));
        reg.register(DecompositionRule::new(
            CommandKindId::from("m_inner"),
            inner_expand,
        ));
        reg
    }

    fn macro_campaign(name: &str) -> Campaign {
        let mut c = Campaign::new(ObjectiveId::from(name), 0);
        c.state = CampaignState::Active;
        c
    }

    #[test]
    fn macro_command_decomposes_to_primitives_in_order() {
        // The campaign is named after the macro — `make_command`
        // synthesizes a command whose `kind` matches the campaign id,
        // so the post-process intercept will pick it up.
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let c = macro_campaign("m_outer");
        let active = [&c];
        let reg = make_two_level_registry();
        let mut agent = CampaignReactiveShort::new();
        let mut plan = PlanState::default();

        // Tick 0: macro is intercepted, expanded recursively into
        // [p_a, p_b, p_z], slot is created, head `p_a` drains.
        let out0 = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 0,
            plan_state: &mut plan,
            decomp: Some(&reg),
        });
        assert_eq!(out0.commands.len(), 1, "tick 0 should emit 1 primitive");
        assert_eq!(out0.commands[0].kind.as_str(), "p_a");

        // Tick 1: macro re-emitted, but slot is non-empty so we skip
        // re-expansion. Head `p_b` drains.
        let out1 = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 1,
            plan_state: &mut plan,
            decomp: Some(&reg),
        });
        assert_eq!(out1.commands.len(), 1);
        assert_eq!(out1.commands[0].kind.as_str(), "p_b");

        // Tick 2: head `p_z` drains. Slot becomes empty after this.
        let out2 = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 2,
            plan_state: &mut plan,
            decomp: Some(&reg),
        });
        assert_eq!(out2.commands.len(), 1);
        assert_eq!(out2.commands[0].kind.as_str(), "p_z");
    }

    #[test]
    fn plan_state_drains_to_empty_after_completion() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let c = macro_campaign("m_outer");
        let active = [&c];
        let reg = make_two_level_registry();
        let mut agent = CampaignReactiveShort::new();
        let mut plan = PlanState::default();

        // Pump enough ticks that the slot fully drains. Three
        // primitives → three drain ticks. After that the campaign
        // re-emits the macro, and the slot is re-seeded again — but
        // we stop before that to assert the GC pass clears empty
        // slots between fills.
        for tick in 0..3 {
            let _ = agent.tick(ShortTermInput {
                bus: &bus,
                faction: FactionId(0),
                context: ShortContext::from("faction"),
                active_campaigns: &active,
                now: tick,
                plan_state: &mut plan,
                decomp: Some(&reg),
            });
        }

        // After the third drain, the slot should have been GC'd
        // (its vec is empty, retain drops it). At the start of the
        // next tick the macro will re-seed; until then `plan_state`
        // is fully empty.
        assert!(
            plan.is_empty(),
            "plan_state should be empty after slot fully drained, got {:?}",
            plan.pending,
        );
    }

    #[test]
    fn primitives_pass_through_unchanged() {
        // A campaign whose id is a primitive (no rule registered).
        // Decomposition shouldn't touch it — same behavior as the
        // legacy path.
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let c = macro_campaign("p_a");
        let active = [&c];
        let reg = make_two_level_registry();
        let mut agent = CampaignReactiveShort::new();
        let mut plan = PlanState::default();
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 0,
            plan_state: &mut plan,
            decomp: Some(&reg),
        });
        assert_eq!(out.commands.len(), 1);
        assert_eq!(out.commands[0].kind.as_str(), "p_a");
        assert!(plan.is_empty(), "plan_state untouched for primitives");
    }

    #[test]
    fn legacy_path_unchanged_when_decomp_is_none() {
        // Same setup as `macro_command_decomposes_to_primitives` but
        // with `decomp: None`. Output must match the legacy contract:
        // one command per active campaign, kind = campaign id.
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let c = macro_campaign("m_outer");
        let active = [&c];
        let mut agent = CampaignReactiveShort::new();
        let mut plan = PlanState::default();
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 0,
            plan_state: &mut plan,
            decomp: None,
        });
        assert_eq!(out.commands.len(), 1);
        assert_eq!(out.commands[0].kind.as_str(), "m_outer");
        assert!(plan.is_empty());
    }

    #[test]
    fn precondition_gate_default_allows_progress() {
        // Default gate (`always_allow_gate`) should permit drainage
        // every tick — equivalent to the test above but the
        // assertion explicitly inspects the gate.
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let c = macro_campaign("m_outer");
        let active = [&c];
        let reg = make_two_level_registry();
        let mut agent = CampaignReactiveShort::new();
        let mut plan = PlanState::default();
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 0,
            plan_state: &mut plan,
            decomp: Some(&reg),
        });
        assert_eq!(out.commands.len(), 1, "default gate allows drainage");
        assert_eq!(out.commands[0].kind.as_str(), "p_a");
    }

    #[test]
    fn precondition_gate_blocks_when_false_returns() {
        // Custom gate that refuses drainage. The macro is still
        // expanded into PlanState (Step 1), but no head is popped
        // (Step 2 skipped). PlanState retains all three primitives.
        fn refuse(_ps: &PlanState, _now: Tick) -> bool {
            false
        }
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let c = macro_campaign("m_outer");
        let active = [&c];
        let reg = make_two_level_registry();
        let mut agent = CampaignReactiveShort::new().with_precondition_gate(refuse);
        let mut plan = PlanState::default();
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 0,
            plan_state: &mut plan,
            decomp: Some(&reg),
        });
        assert_eq!(out.commands.len(), 0, "gate blocks drainage entirely");
        // PlanState retains all three primitives.
        assert_eq!(plan.total_len(), 3, "primitives still queued");
    }

    #[test]
    fn macro_re_emission_does_not_double_seed_slot() {
        // Tick 0: macro emitted, slot seeded with 3 primitives, 1
        // drains → 2 left.
        // Tick 1: macro emitted again. Slot is non-empty so we skip
        // re-expansion. 1 drains → 1 left.
        // PlanState size after tick 1 must be 1, not 4 (which would
        // happen if we re-expanded).
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let c = macro_campaign("m_outer");
        let active = [&c];
        let reg = make_two_level_registry();
        let mut agent = CampaignReactiveShort::new();
        let mut plan = PlanState::default();

        let _ = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 0,
            plan_state: &mut plan,
            decomp: Some(&reg),
        });
        assert_eq!(plan.total_len(), 2, "after tick 0: 3 expanded, 1 drained");

        let _ = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 1,
            plan_state: &mut plan,
            decomp: Some(&reg),
        });
        assert_eq!(
            plan.total_len(),
            1,
            "after tick 1: skip re-expansion, 1 more drained",
        );
    }
}
