use bevy::prelude::*;
use std::collections::HashMap;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::time_system::GameClock;

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

#[derive(Clone, Debug)]
pub struct SystemKnowledge {
    pub system: Entity,
    pub observed_at: i64,
    pub received_at: i64,
    pub data: SystemSnapshot,
}

#[derive(Clone, Debug, Default)]
pub struct SystemSnapshot {
    pub name: String,
    pub position: [f64; 3],
    pub surveyed: bool,
    pub colonized: bool,
    pub population: f64,
    pub production: f64,
}

#[derive(Resource, Default)]
pub struct KnowledgeStore {
    entries: HashMap<Entity, SystemKnowledge>,
}

impl KnowledgeStore {
    pub fn get(&self, system: Entity) -> Option<&SystemKnowledge> {
        self.entries.get(&system)
    }

    pub fn update(&mut self, knowledge: SystemKnowledge) {
        let dominated = self
            .entries
            .get(&knowledge.system)
            .is_some_and(|existing| existing.observed_at >= knowledge.observed_at);

        if !dominated {
            self.entries.insert(knowledge.system, knowledge);
        }
    }

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

pub fn propagate_knowledge(
    clock: Res<GameClock>,
    player_q: Query<&StationedAt, With<Player>>,
    systems: Query<(Entity, &StarSystem, &Position)>,
    positions: Query<&Position>,
    mut store: ResMut<KnowledgeStore>,
) {
    let stationed = match player_q.iter().next() {
        Some(s) => s,
        None => return,
    };

    let player_pos = match positions.get(stationed.system) {
        Ok(pos) => pos,
        Err(_) => return,
    };

    for (entity, star, sys_pos) in &systems {
        let distance = physics::distance_ly(player_pos, sys_pos);
        let delay = physics::light_delay_sexadies(distance);
        let observed_at = clock.elapsed - delay;

        if observed_at < 0 {
            continue;
        }

        let dominated = store
            .get(entity)
            .is_some_and(|existing| existing.observed_at >= observed_at);

        if dominated {
            continue;
        }

        let snapshot = SystemSnapshot {
            name: star.name.clone(),
            position: sys_pos.as_array(),
            surveyed: star.surveyed,
            colonized: star.colonized,
            ..default()
        };

        store.update(SystemKnowledge {
            system: entity,
            observed_at,
            received_at: clock.elapsed,
            data: snapshot,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;

    fn make_knowledge(system: Entity, observed_at: i64) -> SystemKnowledge {
        SystemKnowledge {
            system,
            observed_at,
            received_at: observed_at,
            data: SystemSnapshot::default(),
        }
    }

    #[test]
    fn update_inserts_new_knowledge() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        store.update(make_knowledge(entity, 10));
        assert!(store.get(entity).is_some());
        assert_eq!(store.get(entity).unwrap().observed_at, 10);
    }

    #[test]
    fn newer_observation_replaces_older() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        store.update(make_knowledge(entity, 10));
        store.update(make_knowledge(entity, 20));
        assert_eq!(store.get(entity).unwrap().observed_at, 20);
    }

    #[test]
    fn older_observation_does_not_replace_newer() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        store.update(make_knowledge(entity, 20));
        store.update(make_knowledge(entity, 10));
        assert_eq!(store.get(entity).unwrap().observed_at, 20);
    }

    #[test]
    fn info_age_returns_correct_value() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        store.update(make_knowledge(entity, 10));
        assert_eq!(store.info_age(entity, 25), Some(15));
    }

    #[test]
    fn info_age_returns_none_for_unknown() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let store = KnowledgeStore::default();
        assert_eq!(store.info_age(entity, 100), None);
    }
}
