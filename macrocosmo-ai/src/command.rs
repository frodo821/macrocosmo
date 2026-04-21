//! Command topic value types.
//!
//! Commands are AI → game outputs: the AI decides an action and `emit_command`s
//! it; game code consumes them via `drain_commands`.
//!
//! `Command.params` is a small typed map so the AI core stays engine-agnostic
//! while preserving enough expressiveness for typical parameters
//! (faction/system/entity refs plus scalars and strings).

use std::sync::Arc;

use ahash::AHashMap;
use serde::{Deserialize, Serialize};

use crate::ids::{CommandKindId, EntityRef, FactionId, FactionRef, SystemRef};
use crate::time::Tick;

/// A single parameter value carried by a `Command`.
///
/// The enum is intentionally narrow — anything that doesn't fit is the
/// game's responsibility to encode into an `EntityRef` or a string key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandValue {
    F64(f64),
    I64(i64),
    Str(Arc<str>),
    Faction(FactionId),
    FactionRef(FactionRef),
    System(SystemRef),
    Entity(EntityRef),
    Bool(bool),
}

impl From<f64> for CommandValue {
    fn from(v: f64) -> Self {
        CommandValue::F64(v)
    }
}

impl From<i64> for CommandValue {
    fn from(v: i64) -> Self {
        CommandValue::I64(v)
    }
}

impl From<bool> for CommandValue {
    fn from(v: bool) -> Self {
        CommandValue::Bool(v)
    }
}

impl From<FactionId> for CommandValue {
    fn from(v: FactionId) -> Self {
        CommandValue::Faction(v)
    }
}

impl From<FactionRef> for CommandValue {
    fn from(v: FactionRef) -> Self {
        CommandValue::FactionRef(v)
    }
}

impl From<SystemRef> for CommandValue {
    fn from(v: SystemRef) -> Self {
        CommandValue::System(v)
    }
}

impl From<EntityRef> for CommandValue {
    fn from(v: EntityRef) -> Self {
        CommandValue::Entity(v)
    }
}

impl From<&str> for CommandValue {
    fn from(v: &str) -> Self {
        CommandValue::Str(Arc::from(v))
    }
}

impl From<String> for CommandValue {
    fn from(v: String) -> Self {
        CommandValue::Str(Arc::from(v.into_boxed_str()))
    }
}

/// Parameter map attached to a `Command`. Keys are short `Arc<str>` names.
pub type CommandParams = AHashMap<Arc<str>, CommandValue>;

/// An AI command destined for the game.
///
/// `at` is stamped by the emitter (typically `bus.now()` in the game loop).
/// `priority` is a numeric score used by downstream queues to order commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub kind: CommandKindId,
    pub issuer: FactionId,
    pub target: Option<FactionRef>,
    pub params: CommandParams,
    pub at: Tick,
    pub priority: f64,
}

impl Command {
    pub fn new(kind: CommandKindId, issuer: FactionId, at: Tick) -> Self {
        Self {
            kind,
            issuer,
            target: None,
            params: AHashMap::new(),
            at,
            priority: 0.0,
        }
    }

    pub fn with_target(mut self, target: FactionRef) -> Self {
        self.target = Some(target);
        self
    }

    pub fn with_priority(mut self, priority: f64) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_param(mut self, key: impl Into<Arc<str>>, value: impl Into<CommandValue>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }
}

/// Deterministic-order serialization wrapper for `Command`.
///
/// `Command.params` is an `AHashMap` with non-deterministic iteration order,
/// which breaks byte-identical record/replay. `SerializedCommand` mirrors
/// `Command` but stores params in a `BTreeMap`, giving a canonical encoding.
///
/// `Command` itself is left unchanged: converting to/from `SerializedCommand`
/// is cheap and keeps the hot path using `AHashMap`'s fast hashing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerializedCommand {
    pub kind: CommandKindId,
    pub issuer: FactionId,
    pub target: Option<FactionRef>,
    pub params: std::collections::BTreeMap<Arc<str>, CommandValue>,
    pub at: Tick,
    pub priority: f64,
}

impl From<Command> for SerializedCommand {
    fn from(cmd: Command) -> Self {
        let params: std::collections::BTreeMap<Arc<str>, CommandValue> =
            cmd.params.into_iter().collect();
        Self {
            kind: cmd.kind,
            issuer: cmd.issuer,
            target: cmd.target,
            params,
            at: cmd.at,
            priority: cmd.priority,
        }
    }
}

impl From<SerializedCommand> for Command {
    fn from(s: SerializedCommand) -> Self {
        let mut params: CommandParams = AHashMap::with_capacity(s.params.len());
        for (k, v) in s.params {
            params.insert(k, v);
        }
        Self {
            kind: s.kind,
            issuer: s.issuer,
            target: s.target,
            params,
            at: s.at,
            priority: s.priority,
        }
    }
}
