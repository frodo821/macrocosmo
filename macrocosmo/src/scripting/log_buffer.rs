use bevy::prelude::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Source context for a log entry.
#[derive(Clone, Debug, PartialEq, Eq, bevy::reflect::Reflect)]
#[allow(dead_code)]
pub enum LogSource {
    /// User-typed console input (echo).
    Console,
    /// Output from evaluating a console expression.
    ConsoleResult,
    /// Print output from an event callback.
    Event(String),
    /// Print output from a lifecycle hook.
    Lifecycle(String),
    /// Print output from define_xxx calls.
    Define,
    /// Error message.
    Error,
    /// Generic print output (source not yet determined).
    Print,
}

/// A single log entry.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct LogEntry {
    pub text: String,
    pub source: LogSource,
    pub timestamp: i64,
}

/// Shared print buffer between Lua closures (which run outside ECS) and the
/// Bevy resource world. Lua's `print` override pushes entries here; a Bevy
/// system drains them into `LogBuffer` each frame.
pub type SharedPrintBuffer = Arc<Mutex<Vec<LogEntry>>>;

/// Bevy resource that accumulates Lua log output for display in the console.
#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct LogBuffer {
    pub entries: VecDeque<LogEntry>,
    pub capacity: usize,
    /// Shared buffer that Lua's print function writes into.
    /// `Mutex<Vec<LogEntry>>` is not `Reflect` (interior mutability).
    #[reflect(ignore)]
    pub shared: SharedPrintBuffer,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: 1000,
            shared: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl LogBuffer {
    /// Create a new LogBuffer with a pre-existing shared buffer handle.
    pub fn with_shared(shared: SharedPrintBuffer) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: 1000,
            shared,
        }
    }

    /// Push a log entry, dropping the oldest if over capacity.
    pub fn push(&mut self, text: String, source: LogSource, tick: i64) {
        self.entries.push_back(LogEntry {
            text,
            source,
            timestamp: tick,
        });
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    /// Convenience for pushing an error entry.
    pub fn push_error(&mut self, text: String, tick: i64) {
        self.push(text, LogSource::Error, tick);
    }
}

/// Bevy system that drains the shared print buffer into the LogBuffer resource.
/// Runs every frame in Update.
pub fn drain_print_buffer(
    mut log_buffer: ResMut<LogBuffer>,
    clock: Res<crate::time_system::GameClock>,
) {
    // Drain the shared buffer into a local vec to avoid borrow conflicts.
    let drained: Vec<LogEntry> = {
        let mut shared = log_buffer.shared.lock().unwrap();
        shared.drain(..).collect()
    };

    for entry in drained {
        let tick = if entry.timestamp == 0 {
            clock.elapsed
        } else {
            entry.timestamp
        };
        log_buffer.entries.push_back(LogEntry {
            text: entry.text,
            source: entry.source,
            timestamp: tick,
        });
    }
    while log_buffer.entries.len() > log_buffer.capacity {
        log_buffer.entries.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_buffer_push_respects_capacity() {
        let mut buf = LogBuffer::default();
        buf.capacity = 3;
        for i in 0..5 {
            buf.push(format!("entry {}", i), LogSource::Print, i);
        }
        assert_eq!(buf.entries.len(), 3);
        assert_eq!(buf.entries[0].text, "entry 2");
        assert_eq!(buf.entries[1].text, "entry 3");
        assert_eq!(buf.entries[2].text, "entry 4");
    }

    #[test]
    fn log_buffer_push_error() {
        let mut buf = LogBuffer::default();
        buf.push_error("bad thing".to_string(), 42);
        assert_eq!(buf.entries.len(), 1);
        assert_eq!(buf.entries[0].source, LogSource::Error);
        assert_eq!(buf.entries[0].timestamp, 42);
    }

    #[test]
    fn shared_buffer_drains_correctly() {
        let shared = Arc::new(Mutex::new(Vec::new()));
        let mut buf = LogBuffer::with_shared(shared.clone());

        // Simulate Lua pushing entries into the shared buffer.
        {
            let mut s = shared.lock().unwrap();
            s.push(LogEntry {
                text: "hello from lua".to_string(),
                source: LogSource::Print,
                timestamp: 0,
            });
        }

        // Manually drain (simulating what the system would do).
        let mut locked = buf.shared.lock().unwrap();
        for entry in locked.drain(..) {
            buf.entries.push_back(entry);
        }
        drop(locked);

        assert_eq!(buf.entries.len(), 1);
        assert_eq!(buf.entries[0].text, "hello from lua");
    }
}
