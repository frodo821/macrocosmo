//! Proposal / ProposalOutcome types for the Mid-Mid arbitration
//! protocol (#467 phase 1). The actual FCFS arbiter implementation
//! lands in #467 phase 2; this PR introduces the wire types so the
//! Mid layer can start emitting Proposals through #448 PR2c+ even
//! though the single-Mid case has no real conflict yet.

use serde::{Deserialize, Serialize};

use crate::command::Command;
use crate::ids::{FactionId, RegionId, SystemRef};

// Future: dedicated MidId newtype (#449 multi-Mid split).
// Today every faction has a single empire-wide Mid agent so we
// alias to FactionId and revisit when N Mids per empire land.
pub type MidId = FactionId;

/// Where a proposed command is targeted, used by the arbiter to
/// detect cross-region encroachment and (eventually) by the
/// player-facing UI to render Mid commitments on the galaxy map.
///
/// Today only `FactionWide` and `System` are observable; `Region`
/// is reserved for the multi-Mid split in #449.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locality {
    /// No spatial constraint — applies to the empire as a whole
    /// (e.g. research_focus, retreat).
    FactionWide,
    /// A specific star system.
    System(SystemRef),
    /// Reserved for the per-region Mid split (#449). Always
    /// produces `Accepted` from the identity arbiter today.
    Region(RegionId),
}

/// Mid -> arbiter message: a tentatively-accepted command awaiting
/// FCFS arbitration. The arbiter strips the locality and either
/// accepts the inner Command, or rejects with a ConflictKind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub command: Command,
    pub locality: Locality,
}

impl Proposal {
    pub fn faction_wide(command: Command) -> Self {
        Self {
            command,
            locality: Locality::FactionWide,
        }
    }

    pub fn at_system(command: Command, system: SystemRef) -> Self {
        Self {
            command,
            locality: Locality::System(system),
        }
    }
}

/// arbiter -> Mid reply. Carried on the inter-layer comm channel
/// (#450) once it lands; today the identity arbiter calls Mid
/// in-process and the Outcome is consumed synchronously.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProposalOutcome {
    Accepted,
    Rejected { reason: ConflictKind },
}

/// Why a Proposal was rejected by the FCFS arbiter (#467 phase 2).
/// The identity arbiter never produces these today, but the type
/// is public so downstream consumers can already pattern-match
/// against them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConflictKind {
    /// Another Mid won the race for this target. `claimed_at` is
    /// the tick the winning Proposal arrived at the arbiter.
    AlreadyClaimed { by_mid: MidId, claimed_at: i64 },
    /// The proposing Mid emitted into a region it does not own.
    /// Today a Mid agent owns the empire's whole galactic
    /// footprint so this never fires; #449 promotes ownership
    /// to per-Region authority.
    OutOfRegion,
    /// A budget cap (e.g. concurrent build slots) is exhausted.
    ResourceExhausted,
    /// The target situation has already changed by the time the
    /// Proposal arrived — e.g. the system was colonized between
    /// emit and arrival.
    StaleAtArrival,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CommandKindId, FactionId, RegionId, SystemRef};

    fn sample_command() -> Command {
        Command::new(CommandKindId::from("survey_system"), FactionId(1), 100)
    }

    #[test]
    fn proposal_faction_wide_constructs_correctly() {
        let p = Proposal::faction_wide(sample_command());
        assert!(matches!(p.locality, Locality::FactionWide));
    }

    #[test]
    fn proposal_at_system_round_trips_through_serde() {
        let sys = SystemRef(42);
        let p = Proposal::at_system(sample_command(), sys);
        let json = serde_json::to_string(&p).expect("serialize Proposal");
        let back: Proposal = serde_json::from_str(&json).expect("deserialize Proposal");
        assert_eq!(p, back);
        assert!(matches!(back.locality, Locality::System(s) if s == sys));
    }

    #[test]
    fn proposal_outcome_round_trips_each_variant() {
        let outcomes = [
            ProposalOutcome::Accepted,
            ProposalOutcome::Rejected {
                reason: ConflictKind::OutOfRegion,
            },
            ProposalOutcome::Rejected {
                reason: ConflictKind::AlreadyClaimed {
                    by_mid: FactionId(2),
                    claimed_at: 200,
                },
            },
            ProposalOutcome::Rejected {
                reason: ConflictKind::ResourceExhausted,
            },
            ProposalOutcome::Rejected {
                reason: ConflictKind::StaleAtArrival,
            },
        ];
        for o in &outcomes {
            let json = serde_json::to_string(o).expect("serialize ProposalOutcome");
            let back: ProposalOutcome =
                serde_json::from_str(&json).expect("deserialize ProposalOutcome");
            assert_eq!(*o, back);
        }
    }

    #[test]
    fn locality_round_trips_through_serde() {
        let cases = [
            Locality::FactionWide,
            Locality::System(SystemRef(7)),
            Locality::Region(RegionId::from("northeast_arm")),
        ];
        for c in &cases {
            let json = serde_json::to_string(c).expect("serialize Locality");
            let back: Locality = serde_json::from_str(&json).expect("deserialize Locality");
            assert_eq!(*c, back);
        }
    }
}
