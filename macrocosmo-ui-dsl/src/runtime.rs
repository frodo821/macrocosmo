use std::collections::BTreeMap;

/// Stable key for a value supplied by a host context.
///
/// Examples: `empire`, `system`, `colony`, `ship`, `relation`.
pub type UiContextKey = String;

/// Opaque game-entity handle supplied by the host.
///
/// The DSL crate must not depend on the game engine's entity type. Macrocosmo's
/// Bevy side can adapt `Entity` values into this stable-ish host reference at
/// the boundary where it builds a fragment context.
pub type EntityRef = u64;

/// Opaque handle to host-provided read-only view data.
pub type UiViewRef = String;

/// Opaque handle to host-owned fragment/UI state.
pub type UiStateRef = String;

/// Stable id for a mounted fragment inside one host slot.
pub type FragmentInstanceId = String;

/// Stable key for one declared fragment-local state value.
pub type UiStateKey = String;

/// Host-supplied context value. Keep this small and serializable-looking;
/// real view expansion happens through read-only snapshots, not ECS mutation.
#[derive(Clone, Debug, PartialEq)]
pub enum UiContextValue {
    Entity(EntityRef),
    EntityList(Vec<EntityRef>),
    String(String),
    StringList(Vec<String>),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    ViewRef(UiViewRef),
    StateRef(UiStateRef),
}

impl UiContextValue {
    pub const fn value_type(&self) -> UiContextValueType {
        match self {
            UiContextValue::Entity(_) => UiContextValueType::Entity,
            UiContextValue::EntityList(_) => UiContextValueType::EntityList,
            UiContextValue::String(_) => UiContextValueType::String,
            UiContextValue::StringList(_) => UiContextValueType::StringList,
            UiContextValue::Integer(_) => UiContextValueType::Integer,
            UiContextValue::Number(_) => UiContextValueType::Number,
            UiContextValue::Boolean(_) => UiContextValueType::Boolean,
            UiContextValue::ViewRef(_) => UiContextValueType::ViewRef,
            UiContextValue::StateRef(_) => UiContextValueType::StateRef,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiContextValueType {
    Entity,
    EntityList,
    String,
    StringList,
    Integer,
    Number,
    Boolean,
    ViewRef,
    StateRef,
}

impl UiContextValueType {
    pub fn from_lua_tag(tag: &str) -> Option<Self> {
        match tag {
            "entity" => Some(Self::Entity),
            "entity_list" | "entities" => Some(Self::EntityList),
            "string" => Some(Self::String),
            "string_list" | "strings" => Some(Self::StringList),
            "integer" | "int" => Some(Self::Integer),
            "number" => Some(Self::Number),
            "boolean" | "bool" => Some(Self::Boolean),
            "view" | "view_ref" => Some(Self::ViewRef),
            "state" | "state_ref" => Some(Self::StateRef),
            _ => None,
        }
    }

    pub const fn lua_tag(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::EntityList => "entity_list",
            Self::String => "string",
            Self::StringList => "string_list",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::ViewRef => "view",
            Self::StateRef => "state",
        }
    }
}

/// Scalar fragment-local UI state.
#[derive(Clone, Debug, PartialEq)]
pub enum UiStateValue {
    Bool(bool),
    String(String),
    Number(f64),
    Enum(String),
}

impl UiStateValue {
    fn type_name(&self) -> &'static str {
        match self {
            UiStateValue::Bool(_) => "bool",
            UiStateValue::String(_) => "string",
            UiStateValue::Number(_) => "number",
            UiStateValue::Enum(_) => "enum",
        }
    }
}

/// State bucket visible only to one mounted fragment instance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiFragmentState {
    pub values: BTreeMap<UiStateKey, UiStateValue>,
}

impl UiFragmentState {
    pub fn new(values: impl IntoIterator<Item = (UiStateKey, UiStateValue)>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }

    pub fn set(&mut self, key: &str, value: UiStateValue) -> Result<bool, UiStateUpdateError> {
        let Some(current) = self.values.get_mut(key) else {
            return Err(UiStateUpdateError::UnknownKey(key.to_string()));
        };

        if current.type_name() != value.type_name() {
            return Err(UiStateUpdateError::TypeMismatch {
                key: key.to_string(),
                expected: current.type_name(),
                actual: value.type_name(),
            });
        }

        if current == &value {
            return Ok(false);
        }

        *current = value;
        Ok(true)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiStateUpdateError {
    UnknownKey(String),
    TypeMismatch {
        key: String,
        expected: &'static str,
        actual: &'static str,
    },
}

/// Apply state updates in caller-supplied order.
///
/// The first invalid update aborts the batch before later updates are applied.
/// This keeps partial application deterministic and easy to diagnose.
pub fn apply_state_updates(
    mounted: &mut MountedFragment,
    updates: impl IntoIterator<Item = (UiStateKey, UiStateValue)>,
) -> Result<bool, UiStateUpdateError> {
    let mut next_state = mounted.state.clone();
    let mut changed = false;

    for (key, value) in updates {
        changed |= next_state.set(&key, value)?;
    }

    if changed {
        mounted.state = next_state;
        mounted.dirty.descriptor = true;
    }

    Ok(changed)
}

/// Concrete context passed to fragment matching/inflation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiFragmentContext {
    pub values: BTreeMap<UiContextKey, UiContextValue>,
}

/// Dirty flags for cached descriptor invalidation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiFragmentDirtyFlags {
    pub descriptor: bool,
}

impl UiFragmentDirtyFlags {
    pub const fn clean() -> Self {
        Self { descriptor: false }
    }

    pub const fn descriptor() -> Self {
        Self { descriptor: true }
    }
}

/// Host-owned mounted fragment state. Rendering can use `cached_descriptor`
/// without re-entering Lua until `dirty.descriptor` becomes true.
#[derive(Clone, Debug, PartialEq)]
pub struct MountedFragment {
    pub instance_id: FragmentInstanceId,
    pub fragment_id: String,
    pub context: UiFragmentContext,
    pub state: UiFragmentState,
    pub cached_descriptor: Option<UiNode>,
    pub dirty: UiFragmentDirtyFlags,
}

impl MountedFragment {
    pub fn apply_state_update(
        &mut self,
        key: &str,
        value: UiStateValue,
    ) -> Result<bool, UiStateUpdateError> {
        let changed = self.state.set(key, value)?;
        if changed {
            self.dirty.descriptor = true;
        }
        Ok(changed)
    }
}

/// Desired fragment instance produced by a host's fragment query for one slot.
#[derive(Clone, Debug, PartialEq)]
pub struct DesiredFragment {
    pub instance_id: FragmentInstanceId,
    pub fragment_id: String,
    pub context: UiFragmentContext,
    pub default_state: UiFragmentState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiFragmentReconcileError {
    DuplicateInstanceId(FragmentInstanceId),
}

/// Reconcile one host slot's desired fragment list with its mounted instances.
///
/// State is preserved only when both instance id and fragment id match. This is
/// the same boundary React uses for keyed children: a stable key preserves local
/// state across reorders, while changing the component type for a key resets it.
pub fn reconcile_fragment_slot(
    previous: Vec<MountedFragment>,
    desired: Vec<DesiredFragment>,
) -> Result<Vec<MountedFragment>, UiFragmentReconcileError> {
    let mut previous_by_instance = BTreeMap::new();
    for mounted in previous {
        previous_by_instance.insert(mounted.instance_id.clone(), mounted);
    }

    let mut seen_desired = BTreeMap::new();
    let mut reconciled = Vec::with_capacity(desired.len());

    for desired in desired {
        if seen_desired
            .insert(desired.instance_id.clone(), ())
            .is_some()
        {
            return Err(UiFragmentReconcileError::DuplicateInstanceId(
                desired.instance_id,
            ));
        }

        match previous_by_instance.remove(&desired.instance_id) {
            Some(mut mounted) if mounted.fragment_id == desired.fragment_id => {
                if mounted.context != desired.context {
                    mounted.context = desired.context;
                    mounted.cached_descriptor = None;
                    mounted.dirty.descriptor = true;
                }
                reconciled.push(mounted);
            }
            _ => reconciled.push(MountedFragment {
                instance_id: desired.instance_id,
                fragment_id: desired.fragment_id,
                context: desired.context,
                state: desired.default_state,
                cached_descriptor: None,
                dirty: UiFragmentDirtyFlags::descriptor(),
            }),
        }
    }

    Ok(reconciled)
}

/// Declared context requirements for a fragment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiFragmentContextSpec {
    pub requires: Vec<UiContextBinding>,
    pub optional: Vec<UiContextBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiContextBinding {
    pub key: UiContextKey,
    pub value_type: Option<UiContextValueType>,
}

impl UiContextBinding {
    pub fn untyped(key: impl Into<UiContextKey>) -> Self {
        Self {
            key: key.into(),
            value_type: None,
        }
    }

    pub fn typed(key: impl Into<UiContextKey>, value_type: UiContextValueType) -> Self {
        Self {
            key: key.into(),
            value_type: Some(value_type),
        }
    }

    pub fn matches_value(&self, value: &UiContextValue) -> bool {
        self.value_type
            .is_none_or(|expected| value.value_type() == expected)
    }
}

/// Query issued by a UI host when it wants matching fragments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiFragmentQuery {
    pub required_context: Vec<UiContextKey>,
    pub labels_any: Vec<String>,
    pub labels_all: Vec<String>,
    pub forbidden_labels: Vec<String>,
    pub max_actions: Option<usize>,
}

/// Metadata common to Lua and Rust fragments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiFragmentMeta {
    pub id: String,
    pub labels: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub order: i32,
    pub context: UiFragmentContextSpec,
    pub source: Option<UiFragmentSource>,
}

/// Best-effort source metadata captured when a fragment is registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiFragmentSource {
    pub source: Option<String>,
    pub short_src: Option<String>,
    pub line: Option<usize>,
    pub registration_order: usize,
}

/// Minimal descriptor tree understood by the future renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum UiNode {
    Section {
        title: Option<String>,
        children: Vec<UiNode>,
    },
    VStack {
        align_items: UiAlignItems,
        justify_content: UiJustifyContent,
        children: Vec<UiNode>,
    },
    HStack {
        align_items: UiAlignItems,
        justify_content: UiJustifyContent,
        children: Vec<UiNode>,
    },
    Grid {
        columns: usize,
        children: Vec<UiNode>,
    },
    Row {
        align_items: UiAlignItems,
        justify_content: UiJustifyContent,
        children: Vec<UiNode>,
    },
    Text {
        value: String,
    },
    Progress {
        value: f32,
    },
    Tabs {
        tabs: Vec<UiTabItem>,
    },
    Tooltip {
        content: Box<UiNode>,
        tooltip: Vec<UiNode>,
    },
    ModifiedValue {
        label: String,
        base: String,
        final_value: String,
        modifiers: Vec<UiModifierDisplayLine>,
    },
    Button {
        label: String,
        command: Option<String>,
        secondary_command: Option<String>,
        secondary_shift_command: Option<String>,
        full_width: bool,
        disabled: bool,
        disabled_when: Option<UiConditionDisplay>,
    },
    Action {
        label: String,
        command: String,
        secondary_command: Option<String>,
        secondary_shift_command: Option<String>,
        full_width: bool,
        disabled: bool,
        disabled_when: Option<UiConditionDisplay>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiAlignItems {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiJustifyContent {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiTabItem {
    pub label: String,
    pub command: String,
    pub selected: bool,
    pub disabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiModifierDisplayLine {
    pub label: String,
    pub parts: Vec<String>,
    pub remaining_duration: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiConditionDisplay {
    pub label: String,
    pub satisfied: bool,
    pub operator: UiConditionOperator,
    pub children: Vec<UiConditionDisplay>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiConditionOperator {
    #[default]
    Leaf,
    All,
    Any,
    Not,
    Group,
}

/// Rust-side contract for UI fragments.
pub trait UiFragment: Send + Sync + 'static {
    fn meta(&self) -> &UiFragmentMeta;

    fn matches(&self, query: &UiFragmentQuery, context: &UiFragmentContext) -> bool {
        fragment_meta_matches(self.meta(), query, context)
    }

    fn inflate(&self, context: &UiFragmentContext) -> UiNode;
}

/// Shared metadata/context matching used by fragment implementations.
///
/// Capability and action-count checks need declared `needs` metadata before
/// they can be enforced here. Until then, hosts must validate those after
/// inflation if they opt into action-bearing fragments.
pub fn fragment_meta_matches(
    meta: &UiFragmentMeta,
    query: &UiFragmentQuery,
    context: &UiFragmentContext,
) -> bool {
    if query
        .required_context
        .iter()
        .any(|key| !context.values.contains_key(key))
    {
        return false;
    }

    for binding in &meta.context.requires {
        let Some(value) = context.values.get(&binding.key) else {
            return false;
        };
        if !binding.matches_value(value) {
            return false;
        }
    }

    for binding in &meta.context.optional {
        if let Some(value) = context.values.get(&binding.key)
            && !binding.matches_value(value)
        {
            return false;
        }
    }

    if !query.labels_any.is_empty()
        && !query.labels_any.iter().any(|label| {
            meta.labels
                .iter()
                .any(|fragment_label| fragment_label == label)
        })
    {
        return false;
    }

    if query.labels_all.iter().any(|label| {
        !meta
            .labels
            .iter()
            .any(|fragment_label| fragment_label == label)
    }) {
        return false;
    }

    if query.forbidden_labels.iter().any(|label| {
        meta.labels
            .iter()
            .any(|fragment_label| fragment_label == label)
    }) {
        return false;
    }

    true
}

/// Registry skeleton for fragment discovery.
#[derive(Default)]
pub struct UiFragmentRegistry {
    fragments: Vec<Box<dyn UiFragment>>,
}

impl UiFragmentRegistry {
    pub fn push(&mut self, fragment: Box<dyn UiFragment>) {
        self.fragments.push(fragment);
        self.fragments.sort_by(|a, b| {
            a.meta()
                .order
                .cmp(&b.meta().order)
                .then_with(|| a.meta().id.cmp(&b.meta().id))
        });
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn UiFragment> {
        self.fragments.iter().map(|f| f.as_ref())
    }

    pub fn matching<'a>(
        &'a self,
        query: &'a UiFragmentQuery,
        context: &'a UiFragmentContext,
    ) -> Vec<&'a dyn UiFragment> {
        self.fragments
            .iter()
            .map(|f| f.as_ref())
            .filter(|fragment| fragment.matches(query, context))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestFragment {
        meta: UiFragmentMeta,
    }

    impl TestFragment {
        fn new(id: &str, labels: &[&str], order: i32, requires: &[&str]) -> Self {
            Self {
                meta: UiFragmentMeta {
                    id: id.to_string(),
                    labels: labels.iter().map(|label| label.to_string()).collect(),
                    tags: BTreeMap::new(),
                    order,
                    context: UiFragmentContextSpec {
                        requires: requires
                            .iter()
                            .map(|key| UiContextBinding::untyped(*key))
                            .collect(),
                        optional: Vec::new(),
                    },
                    source: None,
                },
            }
        }
    }

    impl UiFragment for TestFragment {
        fn meta(&self) -> &UiFragmentMeta {
            &self.meta
        }

        fn inflate(&self, _context: &UiFragmentContext) -> UiNode {
            UiNode::Text {
                value: self.meta.id.clone(),
            }
        }
    }

    #[test]
    fn matching_filters_by_context_and_labels_in_registry_order() {
        let mut registry = UiFragmentRegistry::default();
        registry.push(Box::new(TestFragment::new(
            "late",
            &["esc", "ship_ops"],
            20,
            &["empire"],
        )));
        registry.push(Box::new(TestFragment::new(
            "early",
            &["esc", "construction"],
            10,
            &["empire"],
        )));
        registry.push(Box::new(TestFragment::new(
            "missing-context",
            &["esc", "construction"],
            0,
            &["colony"],
        )));

        let context = UiFragmentContext {
            values: BTreeMap::from([(
                "empire".to_string(),
                UiContextValue::String("player".to_string()),
            )]),
        };
        let query = UiFragmentQuery {
            labels_all: vec!["esc".to_string()],
            labels_any: vec!["construction".to_string(), "ship_ops".to_string()],
            ..Default::default()
        };

        let ids: Vec<&str> = registry
            .matching(&query, &context)
            .iter()
            .map(|fragment| fragment.meta().id.as_str())
            .collect();

        assert_eq!(ids, vec!["early", "late"]);
    }

    #[test]
    fn matching_rejects_forbidden_labels() {
        let meta = TestFragment::new("debug", &["debug", "file_io"], 0, &[]).meta;
        let context = UiFragmentContext::default();
        let query = UiFragmentQuery {
            forbidden_labels: vec!["file_io".to_string()],
            ..Default::default()
        };

        assert!(!fragment_meta_matches(&meta, &query, &context));
    }

    #[test]
    fn registry_orders_equal_order_by_id_independent_of_push_order() {
        let mut registry = UiFragmentRegistry::default();
        registry.push(Box::new(TestFragment::new("c", &["esc"], 10, &[])));
        registry.push(Box::new(TestFragment::new("a", &["esc"], 10, &[])));
        registry.push(Box::new(TestFragment::new("b", &["esc"], 10, &[])));

        let ids: Vec<&str> = registry
            .iter()
            .map(|fragment| fragment.meta().id.as_str())
            .collect();

        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn matching_rejects_typed_context_mismatch() {
        let meta = UiFragmentMeta {
            id: "ship-list".to_string(),
            labels: vec!["ship_ops".to_string()],
            tags: BTreeMap::new(),
            order: 0,
            context: UiFragmentContextSpec {
                requires: vec![UiContextBinding::typed(
                    "ships",
                    UiContextValueType::EntityList,
                )],
                optional: vec![UiContextBinding::typed(
                    "filter",
                    UiContextValueType::String,
                )],
            },
            source: None,
        };

        let wrong_required = context(&[("ships", UiContextValue::Entity(1))]);
        assert!(!fragment_meta_matches(
            &meta,
            &UiFragmentQuery::default(),
            &wrong_required
        ));

        let wrong_optional = context(&[
            ("ships", UiContextValue::EntityList(vec![1, 2])),
            ("filter", UiContextValue::Boolean(true)),
        ]);
        assert!(!fragment_meta_matches(
            &meta,
            &UiFragmentQuery::default(),
            &wrong_optional
        ));

        let matching = context(&[
            ("ships", UiContextValue::EntityList(vec![1, 2])),
            ("filter", UiContextValue::String("idle".to_string())),
        ]);
        assert!(fragment_meta_matches(
            &meta,
            &UiFragmentQuery::default(),
            &matching
        ));
    }

    #[test]
    fn matching_requires_all_labels_one_any_and_no_forbidden() {
        let meta = TestFragment::new("resources", &["esc", "resources", "charts"], 0, &[]).meta;
        let context = UiFragmentContext::default();
        let query = UiFragmentQuery {
            labels_all: vec!["esc".to_string(), "resources".to_string()],
            labels_any: vec!["charts".to_string(), "table".to_string()],
            forbidden_labels: vec!["debug".to_string()],
            ..Default::default()
        };

        assert!(fragment_meta_matches(&meta, &query, &context));

        let missing_all = UiFragmentMeta {
            labels: vec!["esc".to_string(), "charts".to_string()],
            ..meta.clone()
        };
        assert!(!fragment_meta_matches(&missing_all, &query, &context));

        let missing_any = UiFragmentMeta {
            labels: vec!["esc".to_string(), "resources".to_string()],
            ..meta.clone()
        };
        assert!(!fragment_meta_matches(&missing_any, &query, &context));

        let forbidden = UiFragmentMeta {
            labels: vec![
                "esc".to_string(),
                "resources".to_string(),
                "charts".to_string(),
                "debug".to_string(),
            ],
            ..meta
        };
        assert!(!fragment_meta_matches(&forbidden, &query, &context));
    }

    fn context(entries: &[(&str, UiContextValue)]) -> UiFragmentContext {
        UiFragmentContext {
            values: entries
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect(),
        }
    }

    fn state(entries: &[(&str, UiStateValue)]) -> UiFragmentState {
        UiFragmentState::new(
            entries
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone())),
        )
    }

    fn desired(instance_id: &str, fragment_id: &str, label: &str) -> DesiredFragment {
        DesiredFragment {
            instance_id: instance_id.to_string(),
            fragment_id: fragment_id.to_string(),
            context: context(&[("label", UiContextValue::String(label.to_string()))]),
            default_state: state(&[("filter", UiStateValue::String(String::new()))]),
        }
    }

    fn mounted(instance_id: &str, fragment_id: &str, label: &str, filter: &str) -> MountedFragment {
        MountedFragment {
            instance_id: instance_id.to_string(),
            fragment_id: fragment_id.to_string(),
            context: context(&[("label", UiContextValue::String(label.to_string()))]),
            state: state(&[("filter", UiStateValue::String(filter.to_string()))]),
            cached_descriptor: Some(UiNode::Text {
                value: format!("cached:{instance_id}"),
            }),
            dirty: UiFragmentDirtyFlags::clean(),
        }
    }

    #[test]
    fn reconcile_builds_new_mounted_tree_with_default_state_dirty_descriptor() {
        let mounted =
            reconcile_fragment_slot(Vec::new(), vec![desired("ship-1", "ship.detail", "Scout")])
                .expect("reconcile");

        assert_eq!(mounted.len(), 1);
        assert_eq!(mounted[0].instance_id, "ship-1");
        assert_eq!(mounted[0].fragment_id, "ship.detail");
        assert_eq!(
            mounted[0].state.values.get("filter"),
            Some(&UiStateValue::String(String::new()))
        );
        assert_eq!(mounted[0].cached_descriptor, None);
        assert_eq!(mounted[0].dirty, UiFragmentDirtyFlags::descriptor());
    }

    #[test]
    fn reconcile_preserves_state_and_cache_for_same_instance_and_fragment() {
        let previous = vec![mounted("ship-1", "ship.detail", "Scout", "weapons")];

        let next = reconcile_fragment_slot(
            previous,
            vec![DesiredFragment {
                instance_id: "ship-1".to_string(),
                fragment_id: "ship.detail".to_string(),
                context: context(&[("label", UiContextValue::String("Scout".to_string()))]),
                default_state: state(&[("filter", UiStateValue::String(String::new()))]),
            }],
        )
        .expect("reconcile");

        assert_eq!(
            next[0].state.values.get("filter"),
            Some(&UiStateValue::String("weapons".to_string()))
        );
        assert_eq!(
            next[0].cached_descriptor,
            Some(UiNode::Text {
                value: "cached:ship-1".to_string()
            })
        );
        assert_eq!(next[0].dirty, UiFragmentDirtyFlags::clean());
    }

    #[test]
    fn reconcile_preserves_keyed_state_across_reorder() {
        let previous = vec![
            mounted("ship-1", "ship.detail", "Scout", "survey"),
            mounted("ship-2", "ship.detail", "Frigate", "combat"),
        ];

        let next = reconcile_fragment_slot(
            previous,
            vec![
                desired("ship-2", "ship.detail", "Frigate"),
                desired("ship-1", "ship.detail", "Scout"),
            ],
        )
        .expect("reconcile");

        assert_eq!(
            next.iter()
                .map(|mounted| mounted.instance_id.as_str())
                .collect::<Vec<_>>(),
            vec!["ship-2", "ship-1"]
        );
        assert_eq!(
            next[0].state.values.get("filter"),
            Some(&UiStateValue::String("combat".to_string()))
        );
        assert_eq!(
            next[1].state.values.get("filter"),
            Some(&UiStateValue::String("survey".to_string()))
        );
    }

    #[test]
    fn reconcile_resets_state_when_fragment_type_changes_for_same_key() {
        let previous = vec![mounted("ship-1", "ship.detail", "Scout", "weapons")];

        let next =
            reconcile_fragment_slot(previous, vec![desired("ship-1", "ship.refit", "Scout")])
                .expect("reconcile");

        assert_eq!(next[0].fragment_id, "ship.refit");
        assert_eq!(
            next[0].state.values.get("filter"),
            Some(&UiStateValue::String(String::new()))
        );
        assert_eq!(next[0].cached_descriptor, None);
        assert_eq!(next[0].dirty, UiFragmentDirtyFlags::descriptor());
    }

    #[test]
    fn reconcile_context_change_preserves_state_but_invalidates_descriptor() {
        let previous = vec![mounted("ship-1", "ship.detail", "Scout", "weapons")];

        let next = reconcile_fragment_slot(
            previous,
            vec![desired("ship-1", "ship.detail", "Scout Mk II")],
        )
        .expect("reconcile");

        assert_eq!(
            next[0].state.values.get("filter"),
            Some(&UiStateValue::String("weapons".to_string()))
        );
        assert_eq!(next[0].cached_descriptor, None);
        assert_eq!(next[0].dirty, UiFragmentDirtyFlags::descriptor());
    }

    #[test]
    fn reconcile_rejects_duplicate_desired_instance_ids() {
        let err = reconcile_fragment_slot(
            Vec::new(),
            vec![
                desired("ship-1", "ship.detail", "Scout"),
                desired("ship-1", "ship.detail", "Scout duplicate"),
            ],
        )
        .expect_err("duplicate key should fail");

        assert_eq!(
            err,
            UiFragmentReconcileError::DuplicateInstanceId("ship-1".to_string())
        );
    }

    #[test]
    fn state_update_marks_descriptor_dirty_only_when_value_changes() {
        let mut mounted = mounted("ship-1", "ship.detail", "Scout", "survey");

        assert_eq!(
            mounted.apply_state_update("filter", UiStateValue::String("survey".to_string())),
            Ok(false)
        );
        assert_eq!(mounted.dirty, UiFragmentDirtyFlags::clean());

        assert_eq!(
            mounted.apply_state_update("filter", UiStateValue::String("combat".to_string())),
            Ok(true)
        );
        assert_eq!(mounted.dirty, UiFragmentDirtyFlags::descriptor());
        assert_eq!(
            mounted.state.values.get("filter"),
            Some(&UiStateValue::String("combat".to_string()))
        );
    }

    #[test]
    fn state_update_rejects_unknown_keys_and_type_changes() {
        let mut mounted = mounted("ship-1", "ship.detail", "Scout", "survey");

        assert_eq!(
            mounted.apply_state_update("missing", UiStateValue::String("x".to_string())),
            Err(UiStateUpdateError::UnknownKey("missing".to_string()))
        );
        assert_eq!(
            mounted.apply_state_update("filter", UiStateValue::Bool(true)),
            Err(UiStateUpdateError::TypeMismatch {
                key: "filter".to_string(),
                expected: "string",
                actual: "bool",
            })
        );
        assert_eq!(
            mounted.state.values.get("filter"),
            Some(&UiStateValue::String("survey".to_string()))
        );
        assert_eq!(mounted.dirty, UiFragmentDirtyFlags::clean());
    }

    #[test]
    fn batched_state_updates_apply_in_order_and_noop_batches_stay_clean() {
        let mut mounted = mounted("ship-1", "ship.detail", "Scout", "survey");

        assert_eq!(
            apply_state_updates(
                &mut mounted,
                vec![
                    (
                        "filter".to_string(),
                        UiStateValue::String("combat".to_string())
                    ),
                    (
                        "filter".to_string(),
                        UiStateValue::String("combat-ready".to_string())
                    ),
                ],
            ),
            Ok(true)
        );
        assert_eq!(
            mounted.state.values.get("filter"),
            Some(&UiStateValue::String("combat-ready".to_string()))
        );
        assert_eq!(mounted.dirty, UiFragmentDirtyFlags::descriptor());

        mounted.dirty = UiFragmentDirtyFlags::clean();
        assert_eq!(
            apply_state_updates(
                &mut mounted,
                vec![(
                    "filter".to_string(),
                    UiStateValue::String("combat-ready".to_string())
                )],
            ),
            Ok(false)
        );
        assert_eq!(mounted.dirty, UiFragmentDirtyFlags::clean());
    }

    #[test]
    fn invalid_batched_state_update_leaves_state_and_dirty_flags_unchanged() {
        let mut mounted = mounted("ship-1", "ship.detail", "Scout", "survey");

        assert_eq!(
            apply_state_updates(
                &mut mounted,
                vec![
                    (
                        "filter".to_string(),
                        UiStateValue::String("combat".to_string())
                    ),
                    ("missing".to_string(), UiStateValue::String("x".to_string())),
                ],
            ),
            Err(UiStateUpdateError::UnknownKey("missing".to_string()))
        );
        assert_eq!(
            mounted.state.values.get("filter"),
            Some(&UiStateValue::String("survey".to_string()))
        );
        assert_eq!(mounted.dirty, UiFragmentDirtyFlags::clean());
    }
}
