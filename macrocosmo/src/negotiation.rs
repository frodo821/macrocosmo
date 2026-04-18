//! #321: Negotiation item kind definitions.
//!
//! Each **negotiation item kind** describes a type of thing that can be exchanged
//! in a diplomatic negotiation (resources, territory, peace treaties, etc.).
//! Kinds are Lua-defined via `define_negotiation_item_kind { ... }` and carry:
//!
//! - `merge_strategy`: how to combine multiple items of the same kind in one
//!   agreement (`List`, `Sum`, `Replace`).
//! - `has_validate`: whether the kind has a Lua `validate` callback (stored in
//!   the Lua accumulator, callable at negotiation-commit time).
//! - `has_apply`: whether the kind has a Lua `apply` callback.

use std::collections::HashMap;

use bevy::prelude::*;

/// How to merge multiple items of the same kind within a single agreement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Keep all items as separate line-items (e.g. territory cessions).
    List,
    /// Sum numeric values (e.g. resource amounts).
    Sum,
    /// Last item wins — only one instance per agreement (e.g. peace).
    Replace,
}

impl MergeStrategy {
    /// Parse from the Lua string representation.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "list" => Some(Self::List),
            "sum" => Some(Self::Sum),
            "replace" => Some(Self::Replace),
            _ => None,
        }
    }
}

/// Definition of a negotiation item kind loaded from Lua.
#[derive(Debug, Clone)]
pub struct NegotiationItemKindDefinition {
    /// Unique string id (e.g. `"resources"`, `"territory"`).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// How to merge multiple items of this kind in one agreement.
    pub merge_strategy: MergeStrategy,
    /// Whether a Lua `validate` function was provided.
    pub has_validate: bool,
    /// Whether a Lua `apply` function was provided.
    pub has_apply: bool,
}

/// Registry of all negotiation item kinds, populated at startup from Lua.
#[derive(Resource, Default, Debug)]
pub struct NegotiationItemKindRegistry {
    pub kinds: HashMap<String, NegotiationItemKindDefinition>,
}
