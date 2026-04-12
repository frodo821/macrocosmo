//! Evaluation context passed to pure evaluators (`Condition::evaluate`,
//! `ValueExpr::evaluate`, `feasibility::evaluate`).
//!
//! Holds only references — cheap to copy and thread through recursive
//! evaluators. Atoms carry their own faction refs; `faction` here is a
//! "default observer" used when atoms leave it implicit.

use crate::bus::AiBus;
use crate::ids::FactionId;
use crate::time::Tick;

#[derive(Clone, Copy)]
pub struct EvalContext<'a> {
    pub bus: &'a AiBus,
    pub now: Tick,
    pub faction: Option<FactionId>,
}

impl<'a> EvalContext<'a> {
    pub fn new(bus: &'a AiBus, now: Tick) -> Self {
        Self {
            bus,
            now,
            faction: None,
        }
    }

    pub fn with_faction(mut self, f: FactionId) -> Self {
        self.faction = Some(f);
        self
    }
}
