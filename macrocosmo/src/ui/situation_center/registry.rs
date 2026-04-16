//! Tab registry + `App` extension trait for the Empire Situation Center
//! (#344 / ESC-1).
//!
//! The registry is a Bevy `Resource` owning a `Vec<Box<dyn SituationTab>>`.
//! Call [`AppSituationExt::register_situation_tab`] or
//! [`AppSituationExt::register_ongoing_situation_tab`] from any plugin to
//! add a tab; the registry preserves insertion order within an `order`
//! key, and sorts ascending across keys at registration time so the tab
//! strip renders deterministically regardless of plugin load order.
//!
//! #346 (ESC-3) registers the four bundled ongoing tabs through this API:
//!
//! ```ignore
//! // in ESC-3's plugin build():
//! app.register_ongoing_situation_tab(ConstructionOverviewTab)
//!    .register_ongoing_situation_tab(ShipOperationsTab)
//!    .register_ongoing_situation_tab(DiplomaticStandingTab)
//!    .register_ongoing_situation_tab(ResourceTrendsTab);
//! ```

use bevy::prelude::*;

use super::tab::{OngoingTab, OngoingTabAdapter, SituationTab, TabId, TabMeta};

/// Resource owning every registered tab.
///
/// Tabs are boxed trait objects so callers can register heterogeneous
/// tab types from any plugin. The registry is append-only — tabs cannot
/// be removed in the current ESC-1 API. Hot-reload / un-registration
/// lands with the Lua tab API.
#[derive(Resource, Default)]
pub struct SituationTabRegistry {
    tabs: Vec<Box<dyn SituationTab>>,
}

impl SituationTabRegistry {
    /// Add an already-boxed tab. Most callers should prefer the
    /// [`AppSituationExt`] wrappers which handle boxing + re-sort.
    pub fn push_boxed(&mut self, tab: Box<dyn SituationTab>) {
        self.tabs.push(tab);
        // Stable sort keeps ties in insertion order (a rarely-exercised
        // corner case but it keeps test expectations predictable).
        self.tabs.sort_by_key(|t| t.meta().order);
    }

    /// Number of registered tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// `true` when no tabs have been registered.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Iterate over the tabs in render order.
    pub fn iter(&self) -> impl Iterator<Item = &dyn SituationTab> {
        self.tabs.iter().map(|boxed| boxed.as_ref())
    }

    /// Look up a tab by `TabId`. Returns `None` if no tab with that id
    /// is registered.
    pub fn get(&self, id: TabId) -> Option<&dyn SituationTab> {
        self.tabs
            .iter()
            .map(|b| b.as_ref())
            .find(|t| t.meta().id == id)
    }

    /// Return the ordered list of `(TabId, display_name, order)`
    /// triples. Used by the tab strip renderer and by tests that want
    /// to assert tab ordering without walking the boxed iter.
    pub fn meta_list(&self) -> Vec<TabMeta> {
        self.tabs.iter().map(|b| b.meta()).collect()
    }
}

/// Extension trait mirroring Bevy's `App::add_plugins` style for ESC
/// tab registration.
///
/// Plugins register tabs in their `build` method:
///
/// ```ignore
/// impl Plugin for MyEscExtensionPlugin {
///     fn build(&self, app: &mut App) {
///         app.register_ongoing_situation_tab(ConstructionOverviewTab);
///     }
/// }
/// ```
///
/// The `&mut App` return is chained so several registrations can be
/// fluently stacked.
pub trait AppSituationExt {
    /// Register an arbitrary [`SituationTab`]. Most tab types want
    /// [`Self::register_ongoing_situation_tab`] instead — this variant
    /// is reserved for bespoke tabs like `NotificationsTab` whose
    /// `render` does not fit the "Event tree" shape.
    fn register_situation_tab<T: SituationTab>(&mut self, tab: T) -> &mut Self;

    /// Register an [`OngoingTab`]. The framework wraps the tab in
    /// [`OngoingTabAdapter`] so the default Event-tree renderer is
    /// used and the tab only has to implement `collect`.
    fn register_ongoing_situation_tab<T: OngoingTab>(&mut self, tab: T) -> &mut Self;
}

impl AppSituationExt for App {
    fn register_situation_tab<T: SituationTab>(&mut self, tab: T) -> &mut Self {
        let registry = self
            .world_mut()
            .get_resource_or_insert_with::<SituationTabRegistry>(Default::default);
        let mut registry = registry;
        registry.push_boxed(Box::new(tab));
        self
    }

    fn register_ongoing_situation_tab<T: OngoingTab>(&mut self, tab: T) -> &mut Self {
        let registry = self
            .world_mut()
            .get_resource_or_insert_with::<SituationTabRegistry>(Default::default);
        let mut registry = registry;
        registry.push_boxed(Box::new(OngoingTabAdapter(tab)));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::situation_center::state::TabState;
    use crate::ui::situation_center::tab::TabBadge;
    use crate::ui::situation_center::types::{Event as EscEvent, Severity};
    use std::any::Any;

    struct StubTab {
        id: TabId,
        display_name: &'static str,
        order: i32,
    }

    impl SituationTab for StubTab {
        fn meta(&self) -> TabMeta {
            TabMeta {
                id: self.id,
                display_name: self.display_name,
                order: self.order,
            }
        }

        fn badge(&self, _world: &World) -> Option<TabBadge> {
            None
        }

        fn render(&self, _ui: &mut bevy_egui::egui::Ui, _world: &World, _state: &mut TabState) {}

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct StubOngoingTab {
        id: TabId,
        order: i32,
    }

    impl OngoingTab for StubOngoingTab {
        fn meta(&self) -> TabMeta {
            TabMeta {
                id: self.id,
                display_name: "Ongoing",
                order: self.order,
            }
        }

        fn collect(&self, _world: &World) -> Vec<EscEvent> {
            Vec::new()
        }
    }

    #[test]
    fn register_and_retrieve_roundtrip() {
        let mut app = App::new();
        app.register_situation_tab(StubTab {
            id: "alpha",
            display_name: "Alpha",
            order: 10,
        });

        let registry = app.world().resource::<SituationTabRegistry>();
        assert_eq!(registry.len(), 1);
        let tab = registry.get("alpha").expect("registered tab retrievable");
        assert_eq!(tab.meta().display_name, "Alpha");
        assert!(registry.get("missing").is_none());
    }

    #[test]
    fn registry_sorts_by_order_key() {
        let mut app = App::new();
        app.register_situation_tab(StubTab {
            id: "c",
            display_name: "C",
            order: 30,
        })
        .register_situation_tab(StubTab {
            id: "a",
            display_name: "A",
            order: 10,
        })
        .register_situation_tab(StubTab {
            id: "b",
            display_name: "B",
            order: 20,
        });

        let ids: Vec<TabId> = app
            .world()
            .resource::<SituationTabRegistry>()
            .meta_list()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn ongoing_tab_registration_wraps_in_adapter() {
        let mut app = App::new();
        app.register_ongoing_situation_tab(StubOngoingTab {
            id: "ongoing",
            order: 5,
        });

        let world = app.world();
        let registry = world.resource::<SituationTabRegistry>();
        assert_eq!(registry.len(), 1);

        // `collect` through the adapter must not panic on an empty world.
        let tab = registry.get("ongoing").expect("ongoing tab retrievable");
        let badge = tab.badge(world);
        // Empty collect ⇒ no badge.
        assert!(badge.is_none());
        let _ = Severity::Info; // silence unused import warning on some toolchains
    }

    #[test]
    fn stable_sort_preserves_insertion_order_within_same_key() {
        let mut app = App::new();
        app.register_situation_tab(StubTab {
            id: "first",
            display_name: "First",
            order: 0,
        })
        .register_situation_tab(StubTab {
            id: "second",
            display_name: "Second",
            order: 0,
        });

        let ids: Vec<TabId> = app
            .world()
            .resource::<SituationTabRegistry>()
            .meta_list()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, vec!["first", "second"]);
    }
}
