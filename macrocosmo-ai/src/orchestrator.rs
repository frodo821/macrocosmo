//! Three-layer orchestrator — drives Long / Mid / Short agents on
//! configurable cadences, routes `IntentSpec`s through an
//! `IntentDispatcher`, manages the in-transit intent queue, the
//! deferred-spec retry queue, and the drop log.
//!
//! See `docs/ai-three-layer.md` §Orchestrator.
//!
//! The orchestrator is per-faction. A game integration that needs
//! multiple mid-term or short-term agents per faction (e.g. one Mid
//! per region, one Short per fleet) will wrap a set of orchestrators
//! in a cluster; that cluster is a future game-side layer — the
//! abstract scenario harness in `macrocosmo-ai` uses a single
//! orchestrator per faction.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::{
    CampaignOp, LongTermAgent, LongTermInput, MidTermAgent, MidTermInput, OverrideEntry,
    ShortTermAgent, ShortTermInput,
};
use crate::ai_params::AiParamsExt;
use crate::bus::AiBus;
use crate::campaign::{Campaign, CampaignState};
use crate::command::Command;
use crate::dispatcher::{DispatchResult, IntentDispatcher};
use crate::eval::EvalContext;
use crate::ids::{FactionId, IntentId, IntentKindId, IntentTargetRef, ShortContext};
use crate::intent::{Intent, IntentSpec};
use crate::time::Tick;
use crate::victory::{VictoryCondition, VictoryStatus};

/// Tunable cadence for the orchestrator's layers.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Ticks between long-term agent invocations. `1` = every tick.
    pub long_cadence: Tick,
    /// Ticks between mid-term agent invocations. `1` = every tick.
    pub mid_cadence: Tick,
    /// Intents with `effective_priority(now)` below this threshold
    /// are considered stale (not applied by the default mid-term
    /// agent). Individual agents may use their own threshold.
    pub stale_priority_threshold: f32,
    /// `ShortContext` label used when driving the short-term agent.
    /// Abstract scenarios default to `"faction"`.
    pub short_context: ShortContext,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            long_cadence: 30,
            mid_cadence: 5,
            stale_priority_threshold: 0.1,
            short_context: ShortContext::from("faction"),
        }
    }
}

/// An `IntentSpec` that could not be dispatched yet and will be
/// retried next tick.
#[derive(Debug, Clone)]
pub struct PendingSpec {
    pub spec: IntentSpec,
    pub deferred_since: Tick,
}

/// Recorded drop — intent could not be delivered at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropEntry {
    pub spec_kind: IntentKindId,
    pub target: IntentTargetRef,
    pub reason: Arc<str>,
    pub at: Tick,
}

/// Per-tick output surface. The orchestrator owns in-flight state;
/// this struct reports what happened this tick.
#[derive(Debug, Default)]
pub struct OrchestratorOutput {
    pub long_fired: bool,
    pub mid_fired: bool,
    pub short_fired: bool,
    pub commands: Vec<Command>,
    pub victory_status: Option<VictoryStatus>,
    /// Intents minted this tick (after dispatch, before arrival).
    pub intents_sent: Vec<Intent>,
    /// New override log entries this tick.
    pub override_events: Vec<OverrideEntry>,
    /// New drop log entries this tick.
    pub drop_events: Vec<DropEntry>,
    /// New campaign ops applied this tick.
    pub campaign_ops: Vec<CampaignOp>,
    /// Intents deferred this tick (dispatcher could not route them).
    pub deferred_count: usize,
}

/// Internal persistent state (outlives a single tick).
#[derive(Debug, Default)]
pub struct OrchestratorState {
    pub last_long_tick: Option<Tick>,
    pub last_mid_tick: Option<Tick>,
    pub intent_queue: Vec<Intent>,
    pub pending_specs: Vec<PendingSpec>,
    pub campaigns: Vec<Campaign>,
    pub override_log: Vec<OverrideEntry>,
    pub drop_log: Vec<DropEntry>,
    pub next_intent_seq: u64,
}

/// Drives one faction's three-layer AI loop.
pub struct Orchestrator<L: LongTermAgent, M: MidTermAgent, S: ShortTermAgent> {
    pub long: L,
    pub mid: M,
    pub short: S,
    pub config: OrchestratorConfig,
    pub state: OrchestratorState,
    pub faction: FactionId,
}

impl<L: LongTermAgent, M: MidTermAgent, S: ShortTermAgent> Orchestrator<L, M, S> {
    pub fn new(faction: FactionId, long: L, mid: M, short: S) -> Self {
        Self {
            long,
            mid,
            short,
            config: OrchestratorConfig::default(),
            state: OrchestratorState::default(),
            faction,
        }
    }

    pub fn with_config(mut self, config: OrchestratorConfig) -> Self {
        self.config = config;
        self
    }

    fn mint_intent_id(&mut self) -> IntentId {
        let seq = self.state.next_intent_seq;
        self.state.next_intent_seq += 1;
        IntentId::from(format!("intent_{}_{}", self.faction.0, seq))
    }

    /// Advance one tick. Steps (see `docs/ai-three-layer.md`):
    /// 1. Retry pending specs via dispatcher.
    /// 2. Evaluate victory status.
    /// 3. Long tick (on cadence) → dispatch new specs (Sent → `intent_queue`).
    /// 4. Move arrived intents into the inbox (includes anything
    ///    dispatched this tick with zero delay).
    /// 5. Mid tick (on cadence OR intent arrived) → apply ops.
    /// 6. Short tick (every call) → emit commands.
    pub fn tick<D: IntentDispatcher + ?Sized>(
        &mut self,
        bus: &mut AiBus,
        dispatcher: &mut D,
        victory: &VictoryCondition,
        ai_params: Option<&dyn AiParamsExt>,
        now: Tick,
    ) -> OrchestratorOutput {
        let mut out = OrchestratorOutput::default();

        // 1. Retry pending specs.
        self.retry_pending(dispatcher, now, &mut out);

        // 2. Evaluate victory.
        let ctx = EvalContext::new(bus, now).with_faction(self.faction);
        let status = victory.evaluate(&ctx);
        out.victory_status = Some(status.clone());

        // 3. Long tick (cadence).
        let run_long = self
            .state
            .last_long_tick
            .map(|last| now - last >= self.config.long_cadence)
            .unwrap_or(true);
        if run_long {
            self.run_long(bus, victory, status.clone(), ai_params, dispatcher, now, &mut out);
            self.state.last_long_tick = Some(now);
        }

        // 4. Drain arrivals (after long/retry so zero-delay intents land
        //    in this same tick's inbox).
        let inbox = self.drain_arrived(now);

        // 5. Mid tick (cadence OR arrival).
        let mid_due = self
            .state
            .last_mid_tick
            .map(|last| now - last >= self.config.mid_cadence)
            .unwrap_or(true);
        let run_mid = mid_due || !inbox.is_empty();
        if run_mid {
            self.run_mid(bus, &inbox, victory, ai_params, now, &mut out);
            self.state.last_mid_tick = Some(now);
        }

        // 6. Short tick (always).
        self.run_short(bus, now, &mut out);

        out
    }

    fn retry_pending<D: IntentDispatcher + ?Sized>(
        &mut self,
        dispatcher: &mut D,
        now: Tick,
        out: &mut OrchestratorOutput,
    ) {
        if self.state.pending_specs.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.state.pending_specs);
        for p in pending {
            let id = self.mint_intent_id();
            match dispatcher.dispatch(id.clone(), p.spec.clone(), now, self.faction) {
                DispatchResult::Sent(intent) => {
                    out.intents_sent.push(intent.clone());
                    self.state.intent_queue.push(intent);
                }
                DispatchResult::Deferred => {
                    out.deferred_count += 1;
                    self.state.pending_specs.push(PendingSpec {
                        spec: p.spec,
                        deferred_since: p.deferred_since,
                    });
                }
                DispatchResult::Dropped { reason } => {
                    let entry = DropEntry {
                        spec_kind: p.spec.kind.clone(),
                        target: p.spec.target.clone(),
                        reason,
                        at: now,
                    };
                    self.state.drop_log.push(entry.clone());
                    out.drop_events.push(entry);
                }
            }
        }
    }

    /// Pull all intents with `arrives_at <= now` into a Vec, dropping
    /// expired ones along the way. Remaining in-transit intents stay
    /// on `intent_queue`.
    fn drain_arrived(&mut self, now: Tick) -> Vec<Intent> {
        let mut inbox = Vec::new();
        let mut kept = Vec::with_capacity(self.state.intent_queue.len());
        for intent in std::mem::take(&mut self.state.intent_queue) {
            if intent.is_expired(now) {
                continue; // silently drop expired
            }
            if intent.has_arrived(now) {
                inbox.push(intent);
            } else {
                kept.push(intent);
            }
        }
        self.state.intent_queue = kept;
        // stable order: oldest issued_at first
        inbox.sort_by_key(|i| (i.issued_at, i.id.as_str().to_string()));
        inbox
    }

    fn run_long<D: IntentDispatcher + ?Sized>(
        &mut self,
        bus: &AiBus,
        victory: &VictoryCondition,
        status: VictoryStatus,
        ai_params: Option<&dyn AiParamsExt>,
        dispatcher: &mut D,
        now: Tick,
        out: &mut OrchestratorOutput,
    ) {
        let campaigns: Vec<&Campaign> = self.state.campaigns.iter().collect();
        let input = LongTermInput {
            bus,
            faction: self.faction,
            victory,
            victory_status: status,
            active_campaigns: &campaigns,
            now,
            params: ai_params,
        };
        let long_out = self.long.tick(input);
        out.long_fired = true;

        for spec in long_out.intents {
            let id = self.mint_intent_id();
            match dispatcher.dispatch(id.clone(), spec.clone(), now, self.faction) {
                DispatchResult::Sent(intent) => {
                    out.intents_sent.push(intent.clone());
                    self.state.intent_queue.push(intent);
                }
                DispatchResult::Deferred => {
                    out.deferred_count += 1;
                    self.state.pending_specs.push(PendingSpec {
                        spec,
                        deferred_since: now,
                    });
                }
                DispatchResult::Dropped { reason } => {
                    let entry = DropEntry {
                        spec_kind: spec.kind.clone(),
                        target: spec.target.clone(),
                        reason,
                        at: now,
                    };
                    self.state.drop_log.push(entry.clone());
                    out.drop_events.push(entry);
                }
            }
        }
    }

    fn run_mid(
        &mut self,
        bus: &AiBus,
        inbox: &[Intent],
        victory: &VictoryCondition,
        ai_params: Option<&dyn AiParamsExt>,
        now: Tick,
        out: &mut OrchestratorOutput,
    ) {
        let input = MidTermInput {
            bus,
            faction: self.faction,
            inbox,
            campaigns: &self.state.campaigns,
            now,
            params: ai_params,
            victory,
        };
        let mid_out = self.mid.tick(input);
        out.mid_fired = true;

        for op in &mid_out.campaign_ops {
            self.apply_campaign_op(op, now);
        }
        out.campaign_ops = mid_out.campaign_ops;

        for entry in mid_out.override_log {
            self.state.override_log.push(entry.clone());
            out.override_events.push(entry);
        }
    }

    fn apply_campaign_op(&mut self, op: &CampaignOp, _now: Tick) {
        match op {
            CampaignOp::Start {
                objective_id,
                source_intent,
                at,
            } => {
                let mut campaign = Campaign::new(objective_id.clone(), *at);
                campaign.source_intent = source_intent.clone();
                // Idempotent: skip if a campaign with same id already exists.
                if !self.state.campaigns.iter().any(|c| &c.id == objective_id) {
                    self.state.campaigns.push(campaign);
                }
            }
            CampaignOp::Transition {
                campaign_id,
                to,
                at,
            } => {
                if let Some(c) = self.state.campaigns.iter_mut().find(|c| &c.id == campaign_id) {
                    let _ = c.transition(*to, *at); // swallow illegal-transition errors
                }
            }
            CampaignOp::AttachIntent {
                campaign_id,
                intent_id,
            } => {
                if let Some(c) = self.state.campaigns.iter_mut().find(|c| &c.id == campaign_id) {
                    c.source_intent = Some(intent_id.clone());
                }
            }
            CampaignOp::SetWeight {
                campaign_id,
                weight,
            } => {
                if let Some(c) = self.state.campaigns.iter_mut().find(|c| &c.id == campaign_id) {
                    c.weight = *weight;
                }
            }
        }
    }

    fn run_short(&mut self, bus: &AiBus, now: Tick, out: &mut OrchestratorOutput) {
        let active: Vec<&Campaign> = self
            .state
            .campaigns
            .iter()
            .filter(|c| c.state == CampaignState::Active)
            .collect();
        let input = ShortTermInput {
            bus,
            faction: self.faction,
            context: self.config.short_context.clone(),
            active_campaigns: &active,
            now,
        };
        let short_out = self.short.tick(input);
        out.short_fired = true;
        out.commands = short_out.commands;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{LongTermOutput, MidTermOutput, ShortTermOutput};
    use crate::condition::Condition;
    use crate::dispatcher::FixedDelayDispatcher;
    use crate::warning::WarningMode;

    // --- Stub agents ---

    struct NoopLong;
    impl LongTermAgent for NoopLong {
        fn tick(&mut self, _input: LongTermInput<'_>) -> LongTermOutput {
            LongTermOutput::default()
        }
    }

    struct EmitOneIntent {
        emitted: bool,
    }
    impl LongTermAgent for EmitOneIntent {
        fn tick(&mut self, _input: LongTermInput<'_>) -> LongTermOutput {
            if self.emitted {
                return LongTermOutput::default();
            }
            self.emitted = true;
            LongTermOutput {
                intents: vec![IntentSpec::new("pursue", "faction")],
            }
        }
    }

    struct StartCampaignOnIntent;
    impl MidTermAgent for StartCampaignOnIntent {
        fn tick(&mut self, input: MidTermInput<'_>) -> MidTermOutput {
            let mut ops = Vec::new();
            for intent in input.inbox {
                ops.push(CampaignOp::Start {
                    objective_id: crate::ids::ObjectiveId::from("expand"),
                    source_intent: Some(intent.id.clone()),
                    at: input.now,
                });
                ops.push(CampaignOp::Transition {
                    campaign_id: crate::ids::ObjectiveId::from("expand"),
                    to: CampaignState::Active,
                    at: input.now,
                });
            }
            MidTermOutput {
                campaign_ops: ops,
                override_log: vec![],
            }
        }
    }

    struct NoopShort;
    impl ShortTermAgent for NoopShort {
        fn tick(&mut self, _input: ShortTermInput<'_>) -> ShortTermOutput {
            ShortTermOutput::default()
        }
    }

    fn always_ongoing_victory() -> VictoryCondition {
        VictoryCondition::simple(Condition::Never, Condition::Always)
    }

    fn orchestrator_with_delay(
        delay: Tick,
    ) -> (
        Orchestrator<EmitOneIntent, StartCampaignOnIntent, NoopShort>,
        FixedDelayDispatcher,
    ) {
        let mut orch = Orchestrator::new(
            FactionId(0),
            EmitOneIntent { emitted: false },
            StartCampaignOnIntent,
            NoopShort,
        );
        orch.config.long_cadence = 1;
        orch.config.mid_cadence = 1;
        (orch, FixedDelayDispatcher::new(delay))
    }

    #[test]
    fn orchestrator_runs_all_three_layers() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let victory = always_ongoing_victory();
        let mut orch = Orchestrator::new(FactionId(0), NoopLong, StartCampaignOnIntent, NoopShort);
        orch.config.long_cadence = 1;
        orch.config.mid_cadence = 1;
        let mut d = FixedDelayDispatcher::zero_delay();

        let out = orch.tick(&mut bus, &mut d, &victory, None, 1);
        assert!(out.long_fired);
        assert!(out.mid_fired);
        assert!(out.short_fired);
    }

    #[test]
    fn intent_delay_gates_mid_tick_application() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let victory = always_ongoing_victory();
        let (mut orch, mut disp) = orchestrator_with_delay(10);

        // tick 0: long emits, dispatcher stamps arrives_at=10
        let out_0 = orch.tick(&mut bus, &mut disp, &victory, None, 0);
        assert_eq!(out_0.intents_sent.len(), 1);
        assert_eq!(out_0.intents_sent[0].arrives_at, 10);
        assert!(out_0.campaign_ops.is_empty(), "mid sees empty inbox");

        // tick 5: still in-transit
        let out_5 = orch.tick(&mut bus, &mut disp, &victory, None, 5);
        assert!(out_5.campaign_ops.is_empty());
        assert_eq!(orch.state.intent_queue.len(), 1);

        // tick 10: arrives, mid starts a campaign
        let out_10 = orch.tick(&mut bus, &mut disp, &victory, None, 10);
        assert_eq!(out_10.campaign_ops.len(), 2); // Start + Transition
        assert_eq!(orch.state.intent_queue.len(), 0);
        assert_eq!(orch.state.campaigns.len(), 1);
        assert_eq!(orch.state.campaigns[0].state, CampaignState::Active);
    }

    #[test]
    fn campaign_op_start_is_idempotent() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let victory = always_ongoing_victory();
        let (mut orch, mut disp) = orchestrator_with_delay(0);

        // tick 0: long emits, dispatcher delivers immediately, mid starts campaign
        let _ = orch.tick(&mut bus, &mut disp, &victory, None, 0);
        assert_eq!(orch.state.campaigns.len(), 1);

        // tick 1: long does not re-emit (EmitOneIntent). Mid has no new inbox.
        let _ = orch.tick(&mut bus, &mut disp, &victory, None, 1);
        assert_eq!(orch.state.campaigns.len(), 1);
    }
}
