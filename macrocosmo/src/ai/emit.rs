//! `SystemParam` ergonomic wrappers around [`AiBusResource`].
//!
//! Game systems interact with the AI bus through three flavours of param:
//!
//! - [`AiBusWriter`] — mutable access to the bus + `GameClock`, used by
//!   systems under [`AiTickSet::MetricProduce`](super::AiTickSet::MetricProduce)
//!   or any system that needs to emit metrics / commands / evidence with
//!   automatic tick stamping.
//! - [`AiBusReader`] — read-only access, used by reasoning systems under
//!   [`AiTickSet::Reason`](super::AiTickSet::Reason).
//! - [`AiBusDrainer`] — mutable access that only drains the pending
//!   command queue, used by systems under
//!   [`AiTickSet::CommandDrain`](super::AiTickSet::CommandDrain) that
//!   convert AI commands back into ECS actions.
//!
//! Each helper stamps `at = clock.elapsed` when the caller does not
//! specify an explicit tick; this keeps emit sites concise.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use macrocosmo_ai::{
    AiBus, Command, FactionId, MetricId, StandingEvidence, Tick, TimestampedValue,
};

use crate::ai::plugin::AiBusResource;
use crate::time_system::GameClock;

/// Mutable bus access with automatic tick stamping.
#[derive(SystemParam)]
pub struct AiBusWriter<'w> {
    bus: ResMut<'w, AiBusResource>,
    clock: Res<'w, GameClock>,
}

impl<'w> AiBusWriter<'w> {
    /// Current tick (hexadies) as seen by the bus emitter.
    pub fn now(&self) -> Tick {
        self.clock.elapsed
    }

    /// Emit a metric sample stamped at [`Self::now`].
    pub fn emit(&mut self, id: &MetricId, value: f64) {
        let at = self.now();
        self.bus.0.emit(id, value, at);
    }

    /// Emit a metric sample at an explicit tick. Prefer [`Self::emit`]
    /// unless stamping historical data (e.g. during loading).
    pub fn emit_at(&mut self, id: &MetricId, value: f64, at: Tick) {
        self.bus.0.emit(id, value, at);
    }

    /// Enqueue a command on the bus. The caller controls the command's
    /// `at` field; callers that want automatic stamping should set
    /// `cmd.at = writer.now()` before calling.
    pub fn emit_command(&mut self, cmd: Command) {
        self.bus.0.emit_command(cmd);
    }

    /// Emit standing evidence. If `ev.at == 0`, the writer overwrites it
    /// with [`Self::now`] so opt-in auto-stamping works by leaving the
    /// default `at`.
    pub fn emit_evidence(&mut self, mut ev: StandingEvidence) {
        if ev.at == 0 {
            ev.at = self.now();
        }
        self.bus.0.emit_evidence(ev);
    }

    /// Direct mutable access to the underlying `AiBus` for operations
    /// not covered by the helpers above (e.g. schema declarations from
    /// late-loaded content).
    pub fn bus(&mut self) -> &mut AiBus {
        &mut self.bus.0
    }
}

/// Read-only bus access.
#[derive(SystemParam)]
pub struct AiBusReader<'w> {
    bus: Res<'w, AiBusResource>,
    clock: Res<'w, GameClock>,
}

impl<'w> AiBusReader<'w> {
    /// Current tick (hexadies).
    pub fn now(&self) -> Tick {
        self.clock.elapsed
    }

    /// Latest value for a metric, or `None` if undeclared / never emitted.
    pub fn current(&self, id: &MetricId) -> Option<f64> {
        self.bus.0.current(id)
    }

    /// Exact-timestamp metric lookup.
    pub fn at(&self, id: &MetricId, t: Tick) -> Option<f64> {
        self.bus.0.at(id, t)
    }

    /// Latest-at-or-before-`t` metric lookup.
    pub fn at_or_before(&self, id: &MetricId, t: Tick) -> Option<f64> {
        self.bus.0.at_or_before(id, t)
    }

    /// Metric window `[now - duration, now]`, collected into a `Vec` for
    /// convenience. Callers that iterate repeatedly should prefer
    /// [`Self::bus`] for zero-allocation access.
    pub fn window(&self, id: &MetricId, duration: Tick) -> Vec<TimestampedValue> {
        self.bus
            .0
            .window(id, self.now(), duration)
            .copied()
            .collect()
    }

    /// Evidence for a given observer over `[now - duration, now]`.
    pub fn evidence_for(&self, observer: FactionId, duration: Tick) -> Vec<StandingEvidence> {
        self.bus
            .0
            .evidence_for(observer, self.now(), duration)
            .cloned()
            .collect()
    }

    /// Non-draining peek at the pending command queue.
    pub fn pending_commands(&self) -> &[Command] {
        self.bus.0.pending_commands()
    }

    /// Direct read access to the underlying bus.
    pub fn bus(&self) -> &AiBus {
        &self.bus.0
    }
}

/// Mutable bus access specialised to draining the pending command queue.
///
/// Used by systems under
/// [`AiTickSet::CommandDrain`](super::AiTickSet::CommandDrain) that
/// convert AI-produced commands into ECS mutations. Does **not** carry a
/// `GameClock` because draining does not need to stamp.
#[derive(SystemParam)]
pub struct AiBusDrainer<'w> {
    bus: ResMut<'w, AiBusResource>,
}

impl<'w> AiBusDrainer<'w> {
    /// Drain and return every pending command.
    pub fn drain_commands(&mut self) -> Vec<Command> {
        self.bus.0.drain_commands()
    }
}
