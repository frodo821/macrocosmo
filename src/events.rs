use bevy::prelude::*;

use crate::time_system::GameSpeed;

#[derive(Message, Clone, Debug)]
pub struct GameEvent {
    pub timestamp: i64,
    pub kind: GameEventKind,
    pub description: String,
    pub related_system: Option<Entity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameEventKind {
    ShipArrived,
    SurveyComplete,
    SurveyDiscovery,
    ColonyEstablished,
    ShipBuilt,
    BuildingDemolished,
    CombatVictory,
    CombatDefeat,
    HostileDetected,
    ShipScrapped,
    ResourceAlert,
}

#[derive(Resource)]
pub struct EventLog {
    pub entries: Vec<GameEvent>,
    pub max_entries: usize,
}

impl Default for EventLog {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 50,
        }
    }
}

impl EventLog {
    pub fn push(&mut self, event: GameEvent) {
        self.entries.push(event);
        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }
}

pub struct EventsPlugin;

impl Plugin for EventsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<GameEvent>()
            .insert_resource(EventLog::default())
            .add_systems(Update, (collect_events, auto_pause_on_event));
    }
}

/// Collect GameEvents from the Bevy message queue into the EventLog
pub fn collect_events(
    mut reader: MessageReader<GameEvent>,
    mut log: ResMut<EventLog>,
) {
    for event in reader.read() {
        log.push(event.clone());
    }
}

/// Auto-pause when a GameEvent fires
pub fn auto_pause_on_event(
    mut reader: MessageReader<GameEvent>,
    mut speed: ResMut<GameSpeed>,
) {
    for _event in reader.read() {
        speed.pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(timestamp: i64, kind: GameEventKind, desc: &str) -> GameEvent {
        GameEvent {
            timestamp,
            kind,
            description: desc.to_string(),
            related_system: None,
        }
    }

    #[test]
    fn event_log_push_respects_max_entries() {
        let mut log = EventLog {
            entries: Vec::new(),
            max_entries: 3,
        };
        for i in 0..5 {
            log.push(make_event(i, GameEventKind::ShipArrived, &format!("event {}", i)));
        }
        assert_eq!(log.entries.len(), 3);
        // Oldest entries should have been removed
        assert_eq!(log.entries[0].timestamp, 2);
        assert_eq!(log.entries[1].timestamp, 3);
        assert_eq!(log.entries[2].timestamp, 4);
    }

    #[test]
    fn event_log_push_ordering() {
        let mut log = EventLog::default();
        log.push(make_event(10, GameEventKind::ShipArrived, "first"));
        log.push(make_event(20, GameEventKind::SurveyComplete, "second"));
        log.push(make_event(30, GameEventKind::ColonyEstablished, "third"));
        assert_eq!(log.entries.len(), 3);
        assert_eq!(log.entries[0].description, "first");
        assert_eq!(log.entries[1].description, "second");
        assert_eq!(log.entries[2].description, "third");
    }

    #[test]
    fn event_log_default_max_entries() {
        let log = EventLog::default();
        assert_eq!(log.max_entries, 50);
        assert!(log.entries.is_empty());
    }
}
