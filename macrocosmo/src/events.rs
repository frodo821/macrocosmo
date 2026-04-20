use bevy::prelude::*;

use crate::knowledge::{EventId, NextEventId};
use crate::time_system::GameSpeed;

#[derive(Message, Clone, Debug)]
pub struct GameEvent {
    /// #249: Unique id used to dedupe notification banners that arrive through
    /// both the legacy event log and the `KnowledgeFact` pipeline for the same
    /// underlying world happening. Allocated from [`NextEventId`] by the
    /// emitting system; `EventId::default()` (== `EventId(0)`) marks an event
    /// that predates the id-dedupe migration.
    pub id: EventId,
    pub timestamp: i64,
    pub kind: GameEventKind,
    pub description: String,
    pub related_system: Option<Entity>,
}

impl GameEvent {
    /// Construct a `GameEvent` tagged with a freshly allocated [`EventId`].
    /// Callers that *also* write a paired `KnowledgeFact` should instead
    /// allocate the id explicitly (via [`NextEventId::allocate`] or
    /// [`crate::knowledge::FactSysParam::allocate_event_id`]) and reuse the
    /// same id in both the event and the fact.
    pub fn new(
        next_id: &mut NextEventId,
        timestamp: i64,
        kind: GameEventKind,
        description: String,
        related_system: Option<Entity>,
    ) -> Self {
        Self {
            id: next_id.allocate(),
            timestamp,
            kind,
            description,
            related_system,
        }
    }
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
    PlayerRespawn,
    ColonyFailed,
    AnomalyDiscovered,
    /// #298 (S-4): An Infrastructure Core has been conquered (hull reached 1.0).
    CoreConquered,
    /// #298 (S-4): Peacetime attack on an Infrastructure Core — grounds for
    /// war declaration (actual auto-war deferred to S-11).
    CasusBelli,
    /// #305 (S-11): War declared via the Casus Belli system.
    WarDeclared,
    /// #305 (S-11): War ended via the Casus Belli system.
    WarEnded,
    /// #324: A faction has been annihilated (no Core ships, no colonies).
    FactionAnnihilated,
    /// #409: A ship has been destroyed in combat.
    ShipDestroyed,
    /// #409: A ship has not returned by expected time — presumed missing.
    ShipMissing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventCategory {
    Combat,
    Exploration,
    Colony,
    Ship,
    Diplomatic,
    Resource,
}

impl EventCategory {
    pub fn color(&self) -> [u8; 3] {
        match self {
            EventCategory::Combat => [220, 80, 80],
            EventCategory::Exploration => [80, 200, 120],
            EventCategory::Colony => [230, 200, 90],
            EventCategory::Ship => [100, 180, 230],
            EventCategory::Diplomatic => [180, 130, 230],
            EventCategory::Resource => [230, 160, 60],
        }
    }
}

impl GameEventKind {
    pub fn category(&self) -> EventCategory {
        match self {
            GameEventKind::CombatVictory
            | GameEventKind::CombatDefeat
            | GameEventKind::HostileDetected
            | GameEventKind::CoreConquered
            | GameEventKind::ShipDestroyed
            | GameEventKind::ShipMissing => EventCategory::Combat,

            GameEventKind::SurveyComplete
            | GameEventKind::SurveyDiscovery
            | GameEventKind::AnomalyDiscovered => EventCategory::Exploration,

            GameEventKind::ColonyEstablished
            | GameEventKind::ColonyFailed
            | GameEventKind::BuildingDemolished => EventCategory::Colony,

            GameEventKind::ShipArrived
            | GameEventKind::ShipBuilt
            | GameEventKind::ShipScrapped
            | GameEventKind::PlayerRespawn => EventCategory::Ship,

            GameEventKind::CasusBelli
            | GameEventKind::WarDeclared
            | GameEventKind::WarEnded
            | GameEventKind::FactionAnnihilated => EventCategory::Diplomatic,

            GameEventKind::ResourceAlert => EventCategory::Resource,
        }
    }

    /// Whether this event kind should auto-pause the game.
    pub fn should_pause(&self) -> bool {
        match self {
            GameEventKind::SurveyComplete
            | GameEventKind::SurveyDiscovery
            | GameEventKind::ColonyEstablished
            | GameEventKind::CombatVictory
            | GameEventKind::CombatDefeat
            | GameEventKind::HostileDetected
            | GameEventKind::PlayerRespawn
            | GameEventKind::ColonyFailed
            | GameEventKind::AnomalyDiscovered
            | GameEventKind::CoreConquered
            | GameEventKind::CasusBelli
            | GameEventKind::WarDeclared
            | GameEventKind::WarEnded
            | GameEventKind::FactionAnnihilated
            | GameEventKind::ShipDestroyed
            | GameEventKind::ShipMissing => true,

            GameEventKind::ShipArrived
            | GameEventKind::ShipBuilt
            | GameEventKind::BuildingDemolished
            | GameEventKind::ShipScrapped
            | GameEventKind::ResourceAlert => false,
        }
    }
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
            .init_resource::<NextEventId>()
            .init_resource::<crate::knowledge::NotifiedEventIds>()
            .add_systems(Update, (collect_events, auto_pause_on_event));
    }
}

/// Collect GameEvents from the Bevy message queue into the EventLog
pub fn collect_events(mut reader: MessageReader<GameEvent>, mut log: ResMut<EventLog>) {
    for event in reader.read() {
        log.push(event.clone());
    }
}

/// Auto-pause when a pause-worthy GameEvent fires
pub fn auto_pause_on_event(mut reader: MessageReader<GameEvent>, mut speed: ResMut<GameSpeed>) {
    for event in reader.read() {
        if event.kind.should_pause() {
            speed.pause();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(timestamp: i64, kind: GameEventKind, desc: &str) -> GameEvent {
        GameEvent {
            id: EventId::default(),
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
            log.push(make_event(
                i,
                GameEventKind::ShipArrived,
                &format!("event {}", i),
            ));
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

    #[test]
    fn important_events_should_pause() {
        assert!(GameEventKind::SurveyComplete.should_pause());
        assert!(GameEventKind::SurveyDiscovery.should_pause());
        assert!(GameEventKind::ColonyEstablished.should_pause());
        assert!(GameEventKind::CombatVictory.should_pause());
        assert!(GameEventKind::CombatDefeat.should_pause());
        assert!(GameEventKind::HostileDetected.should_pause());
        assert!(GameEventKind::PlayerRespawn.should_pause());
        assert!(GameEventKind::ColonyFailed.should_pause());
        assert!(GameEventKind::AnomalyDiscovered.should_pause());
        assert!(GameEventKind::CoreConquered.should_pause());
        assert!(GameEventKind::CasusBelli.should_pause());
        assert!(GameEventKind::WarDeclared.should_pause());
        assert!(GameEventKind::WarEnded.should_pause());
        assert!(GameEventKind::ShipDestroyed.should_pause());
        assert!(GameEventKind::ShipMissing.should_pause());
    }

    #[test]
    fn ship_destroyed_is_distinct_from_combat_defeat() {
        assert_ne!(GameEventKind::ShipDestroyed, GameEventKind::CombatDefeat);
        assert!(GameEventKind::ShipDestroyed.should_pause());
    }

    #[test]
    fn routine_events_should_not_pause() {
        assert!(!GameEventKind::ShipArrived.should_pause());
        assert!(!GameEventKind::ShipBuilt.should_pause());
        assert!(!GameEventKind::BuildingDemolished.should_pause());
        assert!(!GameEventKind::ShipScrapped.should_pause());
        assert!(!GameEventKind::ResourceAlert.should_pause());
    }

    #[test]
    fn all_event_kinds_have_category() {
        let all_kinds = [
            GameEventKind::ShipArrived,
            GameEventKind::SurveyComplete,
            GameEventKind::SurveyDiscovery,
            GameEventKind::ColonyEstablished,
            GameEventKind::ShipBuilt,
            GameEventKind::BuildingDemolished,
            GameEventKind::CombatVictory,
            GameEventKind::CombatDefeat,
            GameEventKind::HostileDetected,
            GameEventKind::ShipScrapped,
            GameEventKind::ResourceAlert,
            GameEventKind::PlayerRespawn,
            GameEventKind::ColonyFailed,
            GameEventKind::AnomalyDiscovered,
            GameEventKind::CoreConquered,
            GameEventKind::CasusBelli,
            GameEventKind::WarDeclared,
            GameEventKind::WarEnded,
            GameEventKind::FactionAnnihilated,
            GameEventKind::ShipDestroyed,
            GameEventKind::ShipMissing,
        ];
        for kind in &all_kinds {
            let _cat = kind.category();
        }
    }

    #[test]
    fn category_colors_are_distinct() {
        use std::collections::HashSet;
        let categories = [
            EventCategory::Combat,
            EventCategory::Exploration,
            EventCategory::Colony,
            EventCategory::Ship,
            EventCategory::Diplomatic,
            EventCategory::Resource,
        ];
        let mut colors = HashSet::new();
        for cat in &categories {
            assert!(colors.insert(cat.color()), "Duplicate color for {:?}", cat);
        }
        assert_eq!(colors.len(), categories.len());
    }
}
