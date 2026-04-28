//! #350: `KindRegistry` resource + `KnowledgeKindId` parser.
//!
//! Part of the #349 ScriptableKnowledge epic (K-1). Defines the core types
//! for Lua-defined knowledge kinds:
//!
//! * [`KnowledgeKindId`] — a parsed `<namespace>:<name>` identifier used to
//!   key kinds in the registry. `<id>@<lifecycle>` strings are **not** ids
//!   and are rejected by the parser.
//! * [`KnowledgeKindDef`] — a single kind's definition (id + payload schema
//!   + origin tag).
//! * [`PayloadSchema`] / [`PayloadFieldType`] — v1 "loose" schema: top-level
//!   field names mapped to Lua type tags. Nested / function / userdata
//!   values are rejected.
//! * [`KindOrigin`] — tags Rust-side (`core:*`) vs Lua-side (`define_knowledge`)
//!   kinds so we can forbid Lua from redefining `core:*`.
//! * [`KindRegistry`] — Bevy resource holding `id -> KnowledgeKindDef`.
//!
//! K-2 (#351) will consume `validate_payload` for `record_knowledge`; K-5
//! (#354) preloads `core:*` kinds here.
//!
//! Spec: see `docs/plan-349-scriptable-knowledge.md` §2.1, §2.3, §3.1, §6.

use bevy::prelude::*;
use std::collections::HashMap;

/// Separator between namespace and name in a `KnowledgeKindId`.
pub const NAMESPACE_SEPARATOR: char = ':';

/// Lifecycle separator in event ids (`<kind>@<lifecycle>`). Ids containing
/// `@` are **invalid kind ids** — they look like event ids and would collide
/// with the automatic `<id>@recorded` / `<id>@observed` wiring.
pub const LIFECYCLE_SEPARATOR: char = '@';

/// Reserved namespace for Rust-side built-in kinds (`core:hostile_detected`,
/// etc.). Lua definitions in this namespace are rejected at load time (plan
/// §0.5 9.6, §2.3).
pub const CORE_NAMESPACE: &str = "core";

/// Errors surfaced by the kind registry / id parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KindRegistryError {
    /// The raw id string contains `@` (reserved for lifecycle events).
    IdContainsLifecycleSeparator(String),
    /// The raw id string is empty.
    EmptyId,
    /// Attempted to (re)define a `core:*` kind from Lua, or insert twice.
    CoreNamespaceReserved(String),
    /// A kind with this id already exists in the registry.
    DuplicateKind(String),
}

impl std::fmt::Display for KindRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KindRegistryError::IdContainsLifecycleSeparator(id) => write!(
                f,
                "knowledge kind id '{id}' contains '@' which is reserved for lifecycle events (<id>@recorded / <id>@observed)"
            ),
            KindRegistryError::EmptyId => write!(f, "knowledge kind id must be non-empty"),
            KindRegistryError::CoreNamespaceReserved(id) => write!(
                f,
                "knowledge kind id '{id}' uses the reserved 'core:' namespace (Rust-side only)"
            ),
            KindRegistryError::DuplicateKind(id) => {
                write!(f, "knowledge kind id '{id}' is already registered")
            }
        }
    }
}

impl std::error::Error for KindRegistryError {}

/// A parsed knowledge kind identifier.
///
/// Invariants (plan §0.5 9.2):
/// * `raw` never contains `@`
/// * `raw` is non-empty
///
/// `<namespace>:<name>` is the **recommended** form but not enforced at parse
/// time — plan §0.5 9.6 says namespace-less ids are `warn only`. The warn
/// path is emitted by [`parse_id_with_warn`] for the Lua load callsite.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bevy::reflect::Reflect)]
pub struct KnowledgeKindId {
    raw: String,
}

impl KnowledgeKindId {
    /// Parse a raw string into a [`KnowledgeKindId`], validating that it is
    /// non-empty and does not contain `@`. Does NOT warn on missing
    /// namespace (caller should use [`parse_id_with_warn`] for that).
    pub fn parse(raw: &str) -> Result<Self, KindRegistryError> {
        if raw.is_empty() {
            return Err(KindRegistryError::EmptyId);
        }
        if raw.contains(LIFECYCLE_SEPARATOR) {
            return Err(KindRegistryError::IdContainsLifecycleSeparator(
                raw.to_string(),
            ));
        }
        Ok(Self {
            raw: raw.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Returns `true` if this id is in the reserved `core:` namespace.
    pub fn is_core(&self) -> bool {
        matches!(self.namespace(), Some(CORE_NAMESPACE))
    }

    /// Returns the `<namespace>` portion (before the first `:`), if present.
    pub fn namespace(&self) -> Option<&str> {
        self.raw.split_once(NAMESPACE_SEPARATOR).map(|(ns, _)| ns)
    }

    /// Returns the `<name>` portion (after the first `:`), or the whole id
    /// if no `:` is present.
    pub fn name(&self) -> &str {
        self.raw
            .split_once(NAMESPACE_SEPARATOR)
            .map(|(_, n)| n)
            .unwrap_or(&self.raw)
    }

    /// Build the canonical `<id>@recorded` lifecycle event id.
    pub fn recorded_event_id(&self) -> String {
        format!("{}{LIFECYCLE_SEPARATOR}recorded", self.raw)
    }

    /// Build the canonical `<id>@observed` lifecycle event id.
    pub fn observed_event_id(&self) -> String {
        format!("{}{LIFECYCLE_SEPARATOR}observed", self.raw)
    }
}

impl std::fmt::Display for KnowledgeKindId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Parse a raw id and emit a `warn!` log if it lacks a `<namespace>:` prefix
/// (plan §0.5 9.6). Errors are propagated unchanged.
pub fn parse_id_with_warn(raw: &str) -> Result<KnowledgeKindId, KindRegistryError> {
    let id = KnowledgeKindId::parse(raw)?;
    if id.namespace().is_none() {
        warn!(
            "knowledge kind id '{raw}' has no namespace prefix; recommended form is '<namespace>:<name>'"
        );
    }
    Ok(id)
}

/// An event id parsed from a string like `"vesk:famine_outbreak@recorded"`.
///
/// Used by K-1 at load time to reject malformed patterns, and by K-3 at
/// `on(...)` registration time to route between `_knowledge_subscribers`
/// and `_event_handlers` (plan §2.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEventId<'a> {
    pub kind_part: &'a str,
    pub lifecycle: &'a str,
}

/// Split an event id string on the **rightmost** `@`. Returns `None` if the
/// string has no `@`. This is intentionally lenient — callers decide what to
/// do with edge cases (plan §10.2 spike).
///
/// Examples:
/// * `"vesk:famine_outbreak@recorded"` → `Some(("vesk:famine_outbreak", "recorded"))`
/// * `"*@observed"` → `Some(("*", "observed"))`
/// * `"harvest_ended"` → `None`
/// * `"foo@"` → `Some(("foo", ""))`
/// * `"@recorded"` → `Some(("", "recorded"))`
/// * `"foo@bar@recorded"` → `Some(("foo@bar", "recorded"))` — caller must
///   further validate that `kind_part` contains no `@`.
pub fn split_event_id(raw: &str) -> Option<ParsedEventId<'_>> {
    raw.rsplit_once(LIFECYCLE_SEPARATOR)
        .map(|(kind_part, lifecycle)| ParsedEventId {
            kind_part,
            lifecycle,
        })
}

/// Loose v1 payload schema: top-level field names mapped to type tags.
/// Nested tables as schema values are rejected by [`parse_payload_schema`]
/// (nested schemas are v2).
#[derive(Debug, Clone, Default, PartialEq, Eq, bevy::reflect::Reflect)]
pub struct PayloadSchema {
    pub fields: HashMap<String, PayloadFieldType>,
}

impl PayloadSchema {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Type tag for a payload field. Matches the Lua-facing strings `"number"`,
/// `"string"`, `"boolean"`, `"table"`, `"entity"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, bevy::reflect::Reflect)]
pub enum PayloadFieldType {
    Number,
    String,
    Boolean,
    Table,
    Entity,
}

impl PayloadFieldType {
    /// Parse the Lua-side type tag string. Returns `None` for unknown tags.
    pub fn parse(tag: &str) -> Option<Self> {
        match tag {
            "number" => Some(Self::Number),
            "string" => Some(Self::String),
            "boolean" | "bool" => Some(Self::Boolean),
            "table" => Some(Self::Table),
            "entity" => Some(Self::Entity),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Table => "table",
            Self::Entity => "entity",
        }
    }
}

/// Whether a kind was defined by Rust (`core:*`) or by Lua (`define_knowledge`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, bevy::reflect::Reflect)]
pub enum KindOrigin {
    Core,
    Lua,
}

/// A single knowledge kind definition.
#[derive(Debug, Clone, bevy::reflect::Reflect)]
pub struct KnowledgeKindDef {
    pub id: KnowledgeKindId,
    pub payload_schema: PayloadSchema,
    pub origin: KindOrigin,
}

/// Bevy resource keyed by the raw id string (equal to `KnowledgeKindId::as_str`).
#[derive(Resource, Default, Debug, Reflect)]
#[reflect(Resource)]
pub struct KindRegistry {
    pub kinds: HashMap<String, KnowledgeKindDef>,
}

impl KindRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&KnowledgeKindDef> {
        self.kinds.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.kinds.contains_key(id)
    }

    /// Insert a kind definition. Fails if:
    /// * A Lua-origin definition tries to use the `core:` namespace.
    /// * The id is already registered (regardless of origin).
    ///
    /// Rust-side callers that preload `core:*` should still see duplicate
    /// protection — the plan (§2.3) treats the registry as the single
    /// source-of-truth.
    pub fn insert(&mut self, def: KnowledgeKindDef) -> Result<(), KindRegistryError> {
        if def.origin == KindOrigin::Lua && def.id.is_core() {
            return Err(KindRegistryError::CoreNamespaceReserved(
                def.id.as_str().to_string(),
            ));
        }
        if self.kinds.contains_key(def.id.as_str()) {
            return Err(KindRegistryError::DuplicateKind(
                def.id.as_str().to_string(),
            ));
        }
        self.kinds.insert(def.id.as_str().to_string(), def);
        Ok(())
    }

    /// #354 (K-5): Pre-populate the registry with the Rust-side built-in
    /// kinds (`core:*`). Each kind mirrors one of the `KnowledgeFact`
    /// variants listed in plan-349 §1.1 with a payload schema that
    /// matches the field set emitted by the core→scripted converter in
    /// [`crate::knowledge::facts`].
    ///
    /// The registry returned here is inserted into the world **before**
    /// Lua `define_knowledge { id = "core:..." }` can run — the
    /// subsequent Lua drain will find each `core:*` id already present
    /// and raise `KindRegistryError::DuplicateKind` (which the loader
    /// surfaces as a `warn!`; plan §0.5 9.6 "`core:` 上書きは常に error").
    ///
    /// The set of kinds and their field mapping **must** stay in sync
    /// with the core→payload converter. [`CORE_KIND_IDS`] and
    /// [`core_kind_catalog`] enumerate the id constants in one place so
    /// callers can iterate without duplicating the string list.
    pub fn preload_core() -> Self {
        let mut r = Self::default();
        for (id, fields) in core_kind_catalog() {
            let schema = PayloadSchema {
                fields: fields
                    .iter()
                    .map(|(k, ty)| ((*k).to_string(), *ty))
                    .collect(),
            };
            let parsed = KnowledgeKindId::parse(id)
                .expect("core kind ids must parse (unit-tested in registry tests)");
            r.insert(KnowledgeKindDef {
                id: parsed,
                payload_schema: schema,
                origin: KindOrigin::Core,
            })
            .expect("core kind catalog has no duplicates (unit-tested in registry tests)");
        }
        r
    }
}

/// `core:*` kind ids (plan-349 §3.5). Exposed so callers that need a
/// stable list (tests, notification bridges) don't have to re-derive it.
pub const CORE_KIND_IDS: &[&str] = &[
    "core:hostile_detected",
    "core:combat_outcome",
    "core:survey_complete",
    "core:anomaly_discovered",
    "core:survey_discovery",
    "core:structure_built",
    "core:colony_established",
    "core:colony_failed",
    "core:ship_arrived",
    "core:core_conquered",
    "core:ship_destroyed",
    "core:ship_missing",
];

/// Full payload schema catalog for `core:*` kinds. The schema mirrors the
/// field set that the core→payload converter emits for each variant —
/// keep them synchronised.
///
/// Field type rationale:
/// * `event_id`: **not** included — it is a Rust-internal dedup handle
///   (see `NotifiedEventIds`), not part of the observable payload.
/// * `Entity` values are exposed as `entity` (serialised as `u64`).
/// * `[f64; 3]` positions are flattened into `target_pos_{x,y,z}`.
/// * `CombatVictor` is flattened into `victor: string` ("player" /
///   "hostile") to keep payloads purely scalar/string/entity typed.
/// * `destroyed: bool` is exposed as `boolean`.
pub fn core_kind_catalog() -> &'static [(&'static str, &'static [(&'static str, PayloadFieldType)])]
{
    &[
        (
            "core:hostile_detected",
            &[
                ("target", PayloadFieldType::Entity),
                ("detector", PayloadFieldType::Entity),
                ("target_pos_x", PayloadFieldType::Number),
                ("target_pos_y", PayloadFieldType::Number),
                ("target_pos_z", PayloadFieldType::Number),
                ("description", PayloadFieldType::String),
            ],
        ),
        (
            "core:combat_outcome",
            &[
                ("system", PayloadFieldType::Entity),
                ("victor", PayloadFieldType::String),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            "core:survey_complete",
            &[
                ("system", PayloadFieldType::Entity),
                ("system_name", PayloadFieldType::String),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            "core:anomaly_discovered",
            &[
                ("system", PayloadFieldType::Entity),
                ("anomaly_id", PayloadFieldType::String),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            "core:survey_discovery",
            &[
                ("system", PayloadFieldType::Entity),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            "core:structure_built",
            &[
                // `system` is Option<Entity>: the converter only inserts
                // the field when the original variant had Some(_), so the
                // schema marks it as Entity without imposing required-ness.
                ("system", PayloadFieldType::Entity),
                ("kind", PayloadFieldType::String),
                ("name", PayloadFieldType::String),
                ("destroyed", PayloadFieldType::Boolean),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            "core:colony_established",
            &[
                ("system", PayloadFieldType::Entity),
                ("planet", PayloadFieldType::Entity),
                ("name", PayloadFieldType::String),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            "core:colony_failed",
            &[
                ("system", PayloadFieldType::Entity),
                ("name", PayloadFieldType::String),
                ("reason", PayloadFieldType::String),
            ],
        ),
        (
            "core:ship_arrived",
            &[
                // Same Option<Entity> note as `core:structure_built`.
                ("system", PayloadFieldType::Entity),
                ("name", PayloadFieldType::String),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            "core:core_conquered",
            &[
                ("system", PayloadFieldType::Entity),
                ("conquered_by", PayloadFieldType::Entity),
                ("original_owner", PayloadFieldType::Entity),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            // #472: per-faction observation of a ship destruction. `system`
            // is `Option<Entity>` (the converter only inserts when Some),
            // mirroring `core:structure_built` / `core:ship_arrived`.
            "core:ship_destroyed",
            &[
                ("system", PayloadFieldType::Entity),
                ("ship_name", PayloadFieldType::String),
                ("destroyed_at", PayloadFieldType::Number),
                ("detail", PayloadFieldType::String),
            ],
        ),
        (
            // #472: per-empire epistemic state when a ship has not returned
            // by the grace window. No `GameEvent` counterpart — emitted
            // straight from the per-faction observation pipeline.
            "core:ship_missing",
            &[
                ("system", PayloadFieldType::Entity),
                ("ship_name", PayloadFieldType::String),
                ("detail", PayloadFieldType::String),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- KnowledgeKindId parse ---

    #[test]
    fn parse_simple_namespaced_id() {
        let id = KnowledgeKindId::parse("vesk:famine_outbreak").unwrap();
        assert_eq!(id.as_str(), "vesk:famine_outbreak");
        assert_eq!(id.namespace(), Some("vesk"));
        assert_eq!(id.name(), "famine_outbreak");
        assert!(!id.is_core());
    }

    #[test]
    fn parse_namespaceless_id_is_allowed() {
        // Warn-only per plan §0.5 9.6; the parser itself accepts it.
        let id = KnowledgeKindId::parse("famine_outbreak").unwrap();
        assert_eq!(id.namespace(), None);
        assert_eq!(id.name(), "famine_outbreak");
    }

    #[test]
    fn parse_core_namespace_is_accepted_at_parse_level() {
        // Parsing itself allows "core:*"; only the registry / Lua API
        // rejects Lua-origin core namespace use.
        let id = KnowledgeKindId::parse("core:hostile_detected").unwrap();
        assert!(id.is_core());
        assert_eq!(id.namespace(), Some("core"));
    }

    #[test]
    fn parse_empty_id_errors() {
        assert_eq!(KnowledgeKindId::parse(""), Err(KindRegistryError::EmptyId));
    }

    // --- Spike 10.2: event id edge cases ---

    #[test]
    fn parse_rejects_id_with_at_symbol() {
        // `define_knowledge { id = "foo@bar" }` is load-time error.
        let err = KnowledgeKindId::parse("foo@bar").unwrap_err();
        assert_eq!(
            err,
            KindRegistryError::IdContainsLifecycleSeparator("foo@bar".into())
        );
    }

    #[test]
    fn parse_rejects_embedded_lifecycle_style_id() {
        // `"foo@recorded"` looks like an event id, not a kind id.
        let err = KnowledgeKindId::parse("vesk:famine@recorded").unwrap_err();
        assert!(matches!(
            err,
            KindRegistryError::IdContainsLifecycleSeparator(_)
        ));
    }

    #[test]
    fn parse_rejects_trailing_at() {
        assert!(KnowledgeKindId::parse("foo@").is_err());
    }

    #[test]
    fn parse_rejects_leading_at() {
        assert!(KnowledgeKindId::parse("@recorded").is_err());
    }

    #[test]
    fn parse_rejects_double_at() {
        // Spike 10.2: pathological `foo@bar@recorded` — parse_id rejects
        // any `@`, regardless of count.
        assert!(KnowledgeKindId::parse("foo@bar@recorded").is_err());
    }

    // --- split_event_id (rsplit_once semantics, plan §2.9) ---

    #[test]
    fn split_event_id_simple_recorded() {
        let p = split_event_id("vesk:famine_outbreak@recorded").unwrap();
        assert_eq!(p.kind_part, "vesk:famine_outbreak");
        assert_eq!(p.lifecycle, "recorded");
    }

    #[test]
    fn split_event_id_wildcard_observed() {
        let p = split_event_id("*@observed").unwrap();
        assert_eq!(p.kind_part, "*");
        assert_eq!(p.lifecycle, "observed");
    }

    #[test]
    fn split_event_id_no_at_is_none() {
        assert_eq!(split_event_id("harvest_ended"), None);
    }

    #[test]
    fn split_event_id_trailing_at() {
        let p = split_event_id("foo@").unwrap();
        assert_eq!(p.kind_part, "foo");
        assert_eq!(p.lifecycle, "");
    }

    #[test]
    fn split_event_id_leading_at() {
        let p = split_event_id("@recorded").unwrap();
        assert_eq!(p.kind_part, "");
        assert_eq!(p.lifecycle, "recorded");
    }

    #[test]
    fn split_event_id_rsplit_semantics() {
        // rsplit_once splits on the *rightmost* `@`, so `foo@bar@recorded`
        // becomes ("foo@bar", "recorded"). Kind-side validation must then
        // reject the remaining `@` in the kind part. Documented here so
        // subsequent suffix-matcher work doesn't accidentally switch to
        // `split_once`.
        let p = split_event_id("foo@bar@recorded").unwrap();
        assert_eq!(p.kind_part, "foo@bar");
        assert_eq!(p.lifecycle, "recorded");
    }

    // --- KnowledgeKindId lifecycle helpers ---

    #[test]
    fn lifecycle_event_ids_are_formatted() {
        let id = KnowledgeKindId::parse("vesk:famine_outbreak").unwrap();
        assert_eq!(id.recorded_event_id(), "vesk:famine_outbreak@recorded");
        assert_eq!(id.observed_event_id(), "vesk:famine_outbreak@observed");
    }

    // --- PayloadFieldType ---

    #[test]
    fn payload_field_type_parse_known_tags() {
        assert_eq!(
            PayloadFieldType::parse("number"),
            Some(PayloadFieldType::Number)
        );
        assert_eq!(
            PayloadFieldType::parse("string"),
            Some(PayloadFieldType::String)
        );
        assert_eq!(
            PayloadFieldType::parse("boolean"),
            Some(PayloadFieldType::Boolean)
        );
        assert_eq!(
            PayloadFieldType::parse("bool"),
            Some(PayloadFieldType::Boolean)
        );
        assert_eq!(
            PayloadFieldType::parse("table"),
            Some(PayloadFieldType::Table)
        );
        assert_eq!(
            PayloadFieldType::parse("entity"),
            Some(PayloadFieldType::Entity)
        );
    }

    #[test]
    fn payload_field_type_parse_unknown_is_none() {
        assert_eq!(PayloadFieldType::parse("cucumber"), None);
        assert_eq!(PayloadFieldType::parse(""), None);
        assert_eq!(PayloadFieldType::parse("Number"), None); // case-sensitive
    }

    // --- KindRegistry insert ---

    fn make_def(raw_id: &str, origin: KindOrigin) -> KnowledgeKindDef {
        KnowledgeKindDef {
            id: KnowledgeKindId::parse(raw_id).unwrap(),
            payload_schema: PayloadSchema::default(),
            origin,
        }
    }

    #[test]
    fn registry_insert_accepts_lua_kind() {
        let mut reg = KindRegistry::default();
        reg.insert(make_def("vesk:famine_outbreak", KindOrigin::Lua))
            .unwrap();
        assert!(reg.contains("vesk:famine_outbreak"));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_insert_accepts_core_kind_from_rust() {
        let mut reg = KindRegistry::default();
        reg.insert(make_def("core:hostile_detected", KindOrigin::Core))
            .unwrap();
        assert!(reg.contains("core:hostile_detected"));
    }

    #[test]
    fn registry_rejects_lua_defining_core_namespace() {
        let mut reg = KindRegistry::default();
        let err = reg
            .insert(make_def("core:hostile_detected", KindOrigin::Lua))
            .unwrap_err();
        assert_eq!(
            err,
            KindRegistryError::CoreNamespaceReserved("core:hostile_detected".into())
        );
        assert!(reg.is_empty());
    }

    #[test]
    fn registry_rejects_duplicate_id() {
        let mut reg = KindRegistry::default();
        reg.insert(make_def("vesk:famine", KindOrigin::Lua))
            .unwrap();
        let err = reg
            .insert(make_def("vesk:famine", KindOrigin::Lua))
            .unwrap_err();
        assert_eq!(err, KindRegistryError::DuplicateKind("vesk:famine".into()));
    }

    #[test]
    fn registry_rejects_duplicate_across_origins() {
        // Rust preloaded first, then Lua tries the same id (even outside
        // core:*, e.g. a test fixture that predeclares a kind).
        let mut reg = KindRegistry::default();
        reg.insert(make_def("mod:test", KindOrigin::Core)).unwrap();
        assert!(matches!(
            reg.insert(make_def("mod:test", KindOrigin::Lua)),
            Err(KindRegistryError::DuplicateKind(_))
        ));
    }

    // --- #354 K-5: preload_core ---

    #[test]
    fn preload_core_registers_all_expected_ids() {
        let reg = KindRegistry::preload_core();
        // The catalog must match CORE_KIND_IDS exactly.
        for id in CORE_KIND_IDS {
            assert!(
                reg.contains(id),
                "preload_core() missing core kind id '{id}'"
            );
        }
        assert_eq!(
            reg.len(),
            CORE_KIND_IDS.len(),
            "preload_core() len mismatch — CORE_KIND_IDS and core_kind_catalog() drifted apart"
        );
    }

    #[test]
    fn preload_core_marks_every_kind_as_core_origin() {
        let reg = KindRegistry::preload_core();
        for id in CORE_KIND_IDS {
            let def = reg.get(id).expect("core id preloaded");
            assert_eq!(
                def.origin,
                KindOrigin::Core,
                "core:* kind '{id}' must carry KindOrigin::Core"
            );
        }
    }

    #[test]
    fn preload_core_then_lua_redefinition_is_duplicate_error() {
        // plan §0.5 9.6 / §2.3: Lua cannot redefine core:* kinds. The
        // first guard (`CoreNamespaceReserved`) also triggers, so we
        // assert both protections are in effect.
        let mut reg = KindRegistry::preload_core();
        let err = reg
            .insert(make_def("core:hostile_detected", KindOrigin::Lua))
            .unwrap_err();
        // Lua-origin hitting core namespace trips the CoreNamespaceReserved
        // error *before* the duplicate check.
        assert!(matches!(err, KindRegistryError::CoreNamespaceReserved(_)));

        // Even if somehow we got past namespace (Rust Core inserting
        // duplicate), duplicate check still holds.
        let err2 = reg
            .insert(make_def("core:hostile_detected", KindOrigin::Core))
            .unwrap_err();
        assert!(matches!(err2, KindRegistryError::DuplicateKind(_)));
    }

    #[test]
    fn core_kind_catalog_matches_core_kind_ids_list() {
        let catalog_ids: std::collections::HashSet<&str> =
            core_kind_catalog().iter().map(|(id, _)| *id).collect();
        let list_ids: std::collections::HashSet<&str> = CORE_KIND_IDS.iter().copied().collect();
        assert_eq!(
            catalog_ids, list_ids,
            "CORE_KIND_IDS and core_kind_catalog() must enumerate the same set"
        );
    }

    #[test]
    fn preload_core_schemas_are_non_empty() {
        // Every core kind should ship at least one schema field so
        // record-time validation has something to check against.
        let reg = KindRegistry::preload_core();
        for id in CORE_KIND_IDS {
            let def = reg.get(id).expect("core id preloaded");
            assert!(
                !def.payload_schema.fields.is_empty(),
                "core:* kind '{id}' must ship a payload schema"
            );
        }
    }
}
