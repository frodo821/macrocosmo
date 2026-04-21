//! Campaign state machine.
//!
//! A `Campaign` is an in-flight pursuit of an `Objective`. It tracks its
//! lifecycle state and records when transitions happen. Game-side code
//! drives transitions based on current feasibility, success criteria, etc.
//!
//! Legal transitions:
//!
//! ```text
//!   Proposed ─────┬──→ Active
//!                 └──→ Abandoned
//!   Active   ─────┬──→ Suspended
//!                 ├──→ Succeeded
//!                 ├──→ Failed
//!                 └──→ Abandoned
//!   Suspended ────┬──→ Active
//!                 ├──→ Failed
//!                 └──→ Abandoned
//!   Succeeded ── (terminal)
//!   Failed    ── (terminal)
//!   Abandoned ── (terminal)
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::ObjectiveId;
use crate::time::Tick;

/// Lifecycle state of a campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CampaignState {
    /// Newly created, not yet active.
    Proposed,
    /// Actively being pursued.
    Active,
    /// Temporarily paused (e.g., awaiting resources).
    Suspended,
    /// Success criteria met.
    Succeeded,
    /// Failed irrecoverably.
    Failed,
    /// Abandoned by the AI (e.g., feasibility dropped below threshold).
    Abandoned,
}

impl CampaignState {
    /// Whether this state is terminal (no outgoing transitions).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            CampaignState::Succeeded | CampaignState::Failed | CampaignState::Abandoned
        )
    }
}

/// In-flight pursuit of an `Objective`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Campaign {
    pub id: ObjectiveId,
    pub state: CampaignState,
    pub started_at: Tick,
    pub last_transition: Tick,
}

/// Errors arising from illegal state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CampaignError {
    #[error("illegal transition: {from:?} -> {to:?}")]
    IllegalTransition {
        from: CampaignState,
        to: CampaignState,
    },
}

impl Campaign {
    pub fn new(id: ObjectiveId, at: Tick) -> Self {
        Self {
            id,
            state: CampaignState::Proposed,
            started_at: at,
            last_transition: at,
        }
    }

    /// Attempt a transition to `to`. Returns `Ok(())` if legal; leaves state
    /// untouched on `Err`.
    pub fn transition(&mut self, to: CampaignState, now: Tick) -> Result<(), CampaignError> {
        if Self::is_legal(self.state, to) {
            self.state = to;
            self.last_transition = now;
            Ok(())
        } else {
            Err(CampaignError::IllegalTransition {
                from: self.state,
                to,
            })
        }
    }

    fn is_legal(from: CampaignState, to: CampaignState) -> bool {
        use CampaignState::*;
        match (from, to) {
            (Proposed, Active) | (Proposed, Abandoned) => true,
            (Active, Suspended) | (Active, Succeeded) | (Active, Failed) | (Active, Abandoned) => {
                true
            }
            (Suspended, Active) | (Suspended, Failed) | (Suspended, Abandoned) => true,
            _ => false,
        }
    }
}
