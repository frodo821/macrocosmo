//! Entity remap infrastructure for save/load.
//!
//! When saving, every `Entity` is translated to a stable `u64` save id. On load,
//! a fresh [`EntityMap`] is built that maps each save id to a newly allocated
//! `Entity`. Types that carry entity references implement [`RemapEntities`] so
//! the load pipeline can rewrite their u64-encoded fields to the freshly
//! allocated entities.
//!
//! Phase A keeps the trait minimal — the core shape is in place so Phase B can
//! extend it to ship-/colony-extension components without changing the wire
//! format.

use bevy::prelude::Entity;
use std::collections::HashMap;

/// Bidirectional mapping between save-id `u64`s and live `Entity` ids.
///
/// Built at two points:
/// - **Save**: populated while walking entities so components can encode their
///   references as `u64` save ids.
/// - **Load**: populated as entities are spawned so [`RemapEntities::remap_entities`]
///   can translate saved u64s back into live `Entity`s.
#[derive(Debug, Default)]
pub struct EntityMap {
    save_to_entity: HashMap<u64, Entity>,
    entity_to_save: HashMap<Entity, u64>,
}

impl EntityMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the mapping in both directions.
    pub fn insert(&mut self, save_id: u64, entity: Entity) {
        self.save_to_entity.insert(save_id, entity);
        self.entity_to_save.insert(entity, save_id);
    }

    /// Look up the `Entity` for a given save id.
    pub fn entity(&self, save_id: u64) -> Option<Entity> {
        self.save_to_entity.get(&save_id).copied()
    }

    /// Look up the save id for a given `Entity`.
    pub fn save_id(&self, entity: Entity) -> Option<u64> {
        self.entity_to_save.get(&entity).copied()
    }

    pub fn len(&self) -> usize {
        self.save_to_entity.len()
    }

    pub fn is_empty(&self) -> bool {
        self.save_to_entity.is_empty()
    }
}

/// Types that carry encoded entity references which must be rewritten at load
/// time. Implementors translate their internal `u64` save-ids to live
/// `Entity`s using `map.entity(save_id)`.
///
/// Missing ids are left as-is or reported by the caller; Phase A uses a
/// best-effort strategy (falls back to `Entity::PLACEHOLDER` for missing
/// references) so corrupt saves degrade rather than panic.
pub trait RemapEntities {
    fn remap_entities(&mut self, map: &EntityMap);
}

impl<T: RemapEntities> RemapEntities for Option<T> {
    fn remap_entities(&mut self, map: &EntityMap) {
        if let Some(inner) = self.as_mut() {
            inner.remap_entities(map);
        }
    }
}

impl<T: RemapEntities> RemapEntities for Vec<T> {
    fn remap_entities(&mut self, map: &EntityMap) {
        for item in self.iter_mut() {
            item.remap_entities(map);
        }
    }
}

/// Custom serde adapter for `HashMap<Entity, V>` that encodes as
/// `Vec<(u64, V)>`. Used on wire-format structs.
pub mod entity_map_serde {
    use bevy::prelude::Entity;
    use serde::de::{Deserialize, Deserializer};
    use serde::ser::{Serialize, Serializer};
    use std::collections::HashMap;

    pub fn serialize<S, V>(map: &HashMap<Entity, V>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        V: Serialize,
    {
        let as_vec: Vec<(u64, &V)> = map.iter().map(|(e, v)| (e.to_bits(), v)).collect();
        as_vec.serialize(ser)
    }

    pub fn deserialize<'de, D, V>(de: D) -> Result<HashMap<Entity, V>, D::Error>
    where
        D: Deserializer<'de>,
        V: Deserialize<'de>,
    {
        let as_vec: Vec<(u64, V)> = Vec::deserialize(de)?;
        Ok(as_vec
            .into_iter()
            .map(|(bits, v)| (Entity::from_bits(bits), v))
            .collect())
    }
}

/// Custom serde adapter for `HashMap<(Entity, Entity), V>`.
pub mod entity_pair_map_serde {
    use bevy::prelude::Entity;
    use serde::de::{Deserialize, Deserializer};
    use serde::ser::{Serialize, Serializer};
    use std::collections::HashMap;

    pub fn serialize<S, V>(
        map: &HashMap<(Entity, Entity), V>,
        ser: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        V: Serialize,
    {
        let as_vec: Vec<(u64, u64, &V)> = map
            .iter()
            .map(|((a, b), v)| (a.to_bits(), b.to_bits(), v))
            .collect();
        as_vec.serialize(ser)
    }

    pub fn deserialize<'de, D, V>(
        de: D,
    ) -> Result<HashMap<(Entity, Entity), V>, D::Error>
    where
        D: Deserializer<'de>,
        V: Deserialize<'de>,
    {
        let as_vec: Vec<(u64, u64, V)> = Vec::deserialize(de)?;
        Ok(as_vec
            .into_iter()
            .map(|(a, b, v)| ((Entity::from_bits(a), Entity::from_bits(b)), v))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::World;

    #[test]
    fn entity_map_round_trip() {
        let mut world = World::new();
        let e1 = world.spawn_empty().id();
        let e2 = world.spawn_empty().id();

        let mut map = EntityMap::new();
        map.insert(1, e1);
        map.insert(2, e2);

        assert_eq!(map.entity(1), Some(e1));
        assert_eq!(map.entity(2), Some(e2));
        assert_eq!(map.save_id(e1), Some(1));
        assert_eq!(map.save_id(e2), Some(2));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn empty_map() {
        let map = EntityMap::new();
        assert!(map.is_empty());
        assert_eq!(map.entity(42), None);
    }
}
