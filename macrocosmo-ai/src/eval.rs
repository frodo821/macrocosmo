//! Evaluation context passed to pure evaluators (`Condition::evaluate`,
//! `ValueExpr::evaluate`, `feasibility::evaluate`).
//!
//! Holds only references — cheap to clone and thread through recursive
//! evaluators. Atoms carry their own faction refs; `faction` here is a
//! "default observer" used when atoms leave it implicit.
//!
//! `standing_config` and `ai_params` are optional; condition atoms that need
//! them (e.g. `StandingBelow`) return `false` when unset.

use crate::ai_params::AiParamsExt;
use crate::bus::AiBus;
use crate::ids::FactionId;
use crate::standing::StandingConfig;
use crate::time::Tick;

/// Context for pure evaluators. Cheap to clone (references only). Not `Copy`
/// because the optional `&dyn AiParamsExt` trait object is not `Copy`.
#[derive(Clone)]
pub struct EvalContext<'a> {
    pub bus: &'a AiBus,
    pub now: Tick,
    pub faction: Option<FactionId>,
    pub standing_config: Option<&'a StandingConfig>,
    pub ai_params: Option<&'a (dyn AiParamsExt + 'a)>,
}

impl<'a> EvalContext<'a> {
    pub fn new(bus: &'a AiBus, now: Tick) -> Self {
        Self {
            bus,
            now,
            faction: None,
            standing_config: None,
            ai_params: None,
        }
    }

    pub fn with_faction(mut self, f: FactionId) -> Self {
        self.faction = Some(f);
        self
    }

    pub fn with_standing_config(mut self, cfg: &'a StandingConfig) -> Self {
        self.standing_config = Some(cfg);
        self
    }

    pub fn with_ai_params(mut self, p: &'a (dyn AiParamsExt + 'a)) -> Self {
        self.ai_params = Some(p);
        self
    }
}
