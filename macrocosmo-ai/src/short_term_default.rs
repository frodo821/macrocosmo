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

use crate::agent::{ShortTermAgent, ShortTermInput, ShortTermOutput};
use crate::command::Command;
use crate::ids::CommandKindId;

/// Config for [`CampaignReactiveShort`].
#[derive(Debug, Clone, Default)]
pub struct ShortTermDefaultConfig {
    /// Optional prefix prepended to the synthesized command kind.
    /// Empty by default (= use objective id as-is).
    pub kind_prefix: String,
}

/// Default short-term agent: one command per active campaign.
#[derive(Debug, Default)]
pub struct CampaignReactiveShort {
    pub config: ShortTermDefaultConfig,
}

impl CampaignReactiveShort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: ShortTermDefaultConfig) -> Self {
        self.config = config;
        self
    }
}

impl ShortTermAgent for CampaignReactiveShort {
    fn tick(&mut self, input: ShortTermInput<'_>) -> ShortTermOutput {
        let mut commands = Vec::new();
        for campaign in input.active_campaigns {
            let kind = if self.config.kind_prefix.is_empty() {
                CommandKindId::from(campaign.id.as_str())
            } else {
                CommandKindId::from(format!(
                    "{}:{}",
                    self.config.kind_prefix,
                    campaign.id.as_str()
                ))
            };
            let mut cmd = Command::new(kind, input.faction, input.now)
                .with_param("campaign", campaign.id.as_str());
            if let Some(src) = &campaign.source_intent {
                cmd = cmd.with_param("source_intent", src.as_str());
            }
            commands.push(cmd);
        }
        ShortTermOutput { commands }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(7),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 5,
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
        });
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &active,
            now: 1,
        });
        assert_eq!(out.commands[0].kind.as_str(), "default_short:expand");
    }

    #[test]
    fn no_commands_when_no_active_campaigns() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut agent = CampaignReactiveShort::new();
        let out = agent.tick(ShortTermInput {
            bus: &bus,
            faction: FactionId(0),
            context: ShortContext::from("faction"),
            active_campaigns: &[],
            now: 1,
        });
        assert_eq!(out.commands.len(), 0);
    }
}
