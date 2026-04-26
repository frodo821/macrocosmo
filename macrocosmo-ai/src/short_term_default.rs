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

use crate::agent::{ShortTermAgent, ShortTermInput, ShortTermOutput};
use crate::command::Command;
use crate::ids::{CommandKindId, ObjectiveId};

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
#[derive(Debug, Default)]
pub struct CampaignReactiveShort {
    pub config: ShortTermDefaultConfig,
    /// Persistent per-campaign accumulators for weighted scheduling.
    accumulators: AHashMap<ObjectiveId, f64>,
}

impl CampaignReactiveShort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: ShortTermDefaultConfig) -> Self {
        self.config = config;
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
        // CampaignReactiveShort is the legacy default — it does not
        // decompose macro commands, so the new `plan_state` and
        // `decomp` borrows are intentionally discarded. F2+ adds
        // decomposition-aware agents that consume them.
        let _ = input.plan_state;
        let _ = input.decomp;

        let mut commands = Vec::new();

        if !self.config.priority_weighted {
            for campaign in input.active_campaigns {
                commands.push(self.make_command(campaign, input.faction, input.now));
            }
            return ShortTermOutput { commands };
        }

        // Weighted mode: accumulate, then drain integer-many commands
        // per campaign in declaration order until cap.
        let mut emitted = 0usize;
        for campaign in input.active_campaigns {
            let acc = self.accumulators.entry(campaign.id.clone()).or_insert(0.0);
            *acc += campaign.weight.max(0.0);
            // Compute how many to fire and decrement accumulator
            // before borrowing self for `make_command`.
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

        // Garbage-collect accumulators for campaigns no longer active
        // (avoids unbounded growth in long scenarios with churn).
        let active_ids: ahash::AHashSet<&ObjectiveId> =
            input.active_campaigns.iter().map(|c| &c.id).collect();
        self.accumulators.retain(|k, _| active_ids.contains(k));

        ShortTermOutput { commands }
    }
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
}
