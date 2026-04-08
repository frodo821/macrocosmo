use bevy::prelude::*;
use std::collections::HashMap;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::player::{Player, StationedAt};

pub struct KnowledgePlugin;

impl Plugin for KnowledgePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(KnowledgeStore::default())
            .add_systems(
                Startup,
                initialize_capital_knowledge
                    .after(crate::galaxy::generate_galaxy)
                    .after(crate::player::spawn_player),
            )
            .add_systems(Update, propagate_knowledge);
    }
}

/// A snapshot of what the player knows about a star system.
#[derive(Clone, Debug)]
pub struct SystemKnowledge {
    pub system: Entity,
    /// Sexadie when this information was generated at the source.
    pub observed_at: i64,
    /// Sexadie when this information reached the player.
    pub received_at: i64,
    pub data: SystemSnapshot,
}

/// Snapshot of a star system's observable state at a point in time.
#[derive(Clone, Debug, Default)]
pub struct SystemSnapshot {
    pub name: String,
    pub position: [f64; 3],
    pub surveyed: bool,
    pub colonized: bool,
    pub population: f64,
    pub production: f64,
}

/// Central store of everything the player knows about star systems.
#[derive(Resource, Default)]
pub struct KnowledgeStore {
    entries: HashMap<Entity, SystemKnowledge>,
}

impl KnowledgeStore {
    pub fn get(&self, system: Entity) -> Option<&SystemKnowledge> {
        self.entries.get(&system)
    }

    /// Update knowledge. Only accepts newer observations.
    pub fn update(&mut self, knowledge: SystemKnowledge) {
        let dominated = self
            .entries
            .get(&knowledge.system)
            .is_some_and(|existing| existing.observed_at >= knowledge.observed_at);

        if !dominated {
            self.entries.insert(knowledge.system, knowledge);
        }
    }

    /// Age of knowledge in sexadies.
    pub fn info_age(&self, system: Entity, current_time: i64) -> Option<i64> {
        self.entries
            .get(&system)
            .map(|k| current_time - k.observed_at)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Entity, &SystemKnowledge)> {
        self.entries.iter()
    }
}

fn initialize_capital_knowledge(
    mut store: ResMut<KnowledgeStore>,
    player_q: Query<&StationedAt, With<Player>>,
    systems: Query<(Entity, &StarSystem, &Position)>,
) {
    let capital_entity = match player_q.iter().next() {
        Some(stationed) => stationed.system,
        None => {
            warn!("Knowledge init: no player found");
            return;
        }
    };

    let (_, capital, capital_pos) = match systems.get(capital_entity) {
        Ok(result) => result,
        Err(_) => {
            warn!("Knowledge init: capital entity not found");
            return;
        }
    };

    let snapshot = SystemSnapshot {
        name: capital.name.clone(),
        position: capital_pos.as_array(),
        surveyed: capital.surveyed,
        colonized: capital.colonized,
        population: 1.0,
        production: 1.0,
        ..default()
    };

    store.update(SystemKnowledge {
        system: capital_entity,
        observed_at: 0,
        received_at: 0,
        data: snapshot,
    });

    info!("Player knowledge initialized: capital '{}'", capital.name);
}

/// Placeholder for future light-speed information propagation.
fn propagate_knowledge() {}
