//! Perception mode facade for knowledge reads.
//!
//! Slice 1 of the knowledge redesign (`docs/knowledge-redesign-implementation-plan.md`).
//!
//! This module introduces [`PerceptionMode`] and [`Perception`] as types only.
//! Slice 1 does not change any UI or visualization call site; those migrations
//! land in Slice 5 (`refactor(ui): replace knowledge option fallback with
//! perception mode`).
//!
//! The intent is that normal consumers should branch on [`PerceptionMode`]
//! explicitly rather than treating `Option<&KnowledgeStore>` as a fallback
//! signal that quietly degrades to realtime ECS state. Once Slice 5 is in,
//! `Subject` with missing knowledge means *unknown* and `Omniscient` is the
//! only sanctioned ground-truth read path.

use super::KnowledgeStore;
use super::subject::KnowledgeSubject;

/// How a consumer is allowed to read world state.
///
/// * [`PerceptionMode::Subject`] — read through a specific subject's knowledge.
///   Missing knowledge must surface as *unknown*, never as silent realtime
///   fallback.
/// * [`PerceptionMode::Omniscient`] — explicit ground-truth read. Reserved for
///   simulation producers, observer-mode visualization, and debug views. AI
///   must never receive this mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PerceptionMode {
    Subject(KnowledgeSubject),
    Omniscient,
}

impl PerceptionMode {
    pub fn is_omniscient(self) -> bool {
        matches!(self, PerceptionMode::Omniscient)
    }

    pub fn subject(self) -> Option<KnowledgeSubject> {
        match self {
            PerceptionMode::Subject(s) => Some(s),
            PerceptionMode::Omniscient => None,
        }
    }
}

/// A perception view = the mode plus the knowledge store the subject can read.
///
/// `knowledge` is `None` when the subject has no [`KnowledgeStore`] attached
/// yet (e.g. a ship or fleet subject in pre-Slice-1 layouts) OR when the mode
/// is `Omniscient` (omniscient reads must not consult subject knowledge).
pub struct Perception<'a> {
    pub mode: PerceptionMode,
    pub knowledge: Option<&'a KnowledgeStore>,
}

impl<'a> Perception<'a> {
    pub fn subject(subject: KnowledgeSubject, knowledge: Option<&'a KnowledgeStore>) -> Self {
        Self {
            mode: PerceptionMode::Subject(subject),
            knowledge,
        }
    }

    pub fn omniscient() -> Self {
        Self {
            mode: PerceptionMode::Omniscient,
            knowledge: None,
        }
    }

    pub fn is_omniscient(&self) -> bool {
        self.mode.is_omniscient()
    }
}
