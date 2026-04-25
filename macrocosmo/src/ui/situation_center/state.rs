//! Panel / per-tab state for the Empire Situation Center (#344 / ESC-1).
//!
//! Tab implementations are stateless on purpose — any scratch state a
//! tab wants to persist across frames (scroll offset, severity filter,
//! search string) lives in [`TabState`] keyed by [`TabId`] on the
//! shared [`SituationCenterState`] resource. This keeps trait objects
//! `Send + Sync` and makes persistence / save-load trivial.

use std::collections::HashMap;

use bevy::prelude::{ReflectResource, Resource};
use bevy::reflect::Reflect;

use super::tab::TabId;
use super::types::Severity;

/// Per-tab UI scratch state.
///
/// `Default` is intentionally trivial — new tabs don't have to opt in to
/// every field. Fields are `pub` so tab renderers can mutate directly.
#[derive(Clone, Debug, Default, bevy::reflect::Reflect)]
pub struct TabState {
    /// Free-form filter string (e.g. "minerals" / "Corvette").
    pub filter: String,
    /// Severity cutoff — tabs that apply filtering hide entries below
    /// this threshold. `None` ⇒ show all.
    pub severity_floor: Option<Severity>,
    /// Scroll offset retained across tab switches (renderers may ignore).
    pub scroll_offset: f32,
}

/// Global ESC panel state.
///
/// Owned as a Bevy `Resource`; registered by [`super::plugin::SituationCenterPlugin`].
/// Contains three concerns bundled together because every ESC system
/// touches all of them:
/// 1. panel open / closed,
/// 2. which tab is active,
/// 3. per-tab scratch state keyed by `TabId`.
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource)]
pub struct SituationCenterState {
    /// Whether the panel is visible this frame.
    pub open: bool,
    /// Currently active tab id, or `None` when no tab is registered.
    pub active_tab: Option<TabId>,
    /// Per-tab scratch state keyed by `TabId`.
    ///
    /// Entries are lazily inserted on first render; we never garbage
    /// collect so a tab can pick up exactly where it left off if it
    /// un-registers + re-registers (mostly a hot-reload consideration).
    pub tab_states: HashMap<TabId, TabState>,
}

impl SituationCenterState {
    /// Return the `TabState` for `id`, inserting a default if missing.
    pub fn tab_state_mut(&mut self, id: TabId) -> &mut TabState {
        self.tab_states.entry(id).or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_state_mut_inserts_default_on_first_access() {
        let mut state = SituationCenterState::default();
        assert!(state.tab_states.is_empty());

        let slot = state.tab_state_mut("construction");
        slot.filter = "minerals".into();

        // Second access returns the same slot with preserved mutation.
        let slot = state.tab_state_mut("construction");
        assert_eq!(slot.filter, "minerals");
        assert_eq!(state.tab_states.len(), 1);
    }

    #[test]
    fn default_state_is_closed_with_no_active_tab() {
        let state = SituationCenterState::default();
        assert!(!state.open);
        assert!(state.active_tab.is_none());
    }
}
