//! Command topic internal storage.
//!
//! Commands are AI → game outputs. The store holds:
//! - `specs`: declared command kinds. Emitting to an undeclared kind warns + no-ops.
//! - `pending`: FIFO queue drained by the game consumer.

use ahash::AHashMap;

use crate::command::Command;
use crate::ids::CommandKindId;
use crate::spec::CommandSpec;

#[derive(Debug, Default)]
pub(crate) struct CommandStore {
    pub(crate) specs: AHashMap<CommandKindId, CommandSpec>,
    pub(crate) pending: Vec<Command>,
}

impl CommandStore {
    pub(crate) fn drain(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.pending)
    }
}
