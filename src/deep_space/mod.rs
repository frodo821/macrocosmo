use std::collections::HashMap;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::condition::Condition;
use crate::ship::Owner;

/// A structure placed at arbitrary galactic coordinates, not attached to any star system.
#[derive(Component, Clone, Debug)]
pub struct DeepSpaceStructure {
    pub definition_id: String,
    pub name: String,
    pub owner: Owner,
}

/// Hitpoints for a deep-space structure.
#[derive(Component, Clone, Debug)]
pub struct StructureHitpoints {
    pub current: f64,
    pub max: f64,
}

/// Resource cost for building a structure.
#[derive(Clone, Debug, Default)]
pub struct ResourceCost {
    pub minerals: Amt,
    pub energy: Amt,
}

/// Parameters for a named capability (e.g. detection range for sensors).
#[derive(Clone, Debug, Default)]
pub struct CapabilityParams {
    pub range: f64,
    // Extensible: add more fields as needed.
}

/// Data-driven definition of a structure type, loaded from Lua or hardcoded fallback.
#[derive(Clone, Debug)]
pub struct StructureDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub max_hp: f64,
    pub cost: ResourceCost,
    pub build_time: i64, // hexadies
    pub energy_drain: Amt, // per hexady maintenance
    pub prerequisites: Option<Condition>,
    pub capabilities: HashMap<String, CapabilityParams>,
}

/// Registry of all structure definitions.
#[derive(Resource, Default)]
pub struct StructureRegistry {
    pub definitions: HashMap<String, StructureDefinition>,
}

impl StructureRegistry {
    /// Look up a structure definition by id.
    pub fn get(&self, id: &str) -> Option<&StructureDefinition> {
        self.definitions.get(id)
    }

    /// Insert a structure definition, replacing any existing one with the same id.
    pub fn insert(&mut self, def: StructureDefinition) {
        self.definitions.insert(def.id.clone(), def);
    }
}

/// Default structure definitions used when Lua scripts are not available (e.g. in tests).
pub fn default_structure_definitions() -> Vec<StructureDefinition> {
    use crate::condition::ConditionAtom;

    vec![
        StructureDefinition {
            id: "sensor_buoy".to_string(),
            name: "Sensor Buoy".to_string(),
            description: "Detects sublight vessel movements.".to_string(),
            max_hp: 20.0,
            cost: ResourceCost {
                minerals: Amt::units(50),
                energy: Amt::units(30),
            },
            build_time: 15,
            capabilities: HashMap::from([(
                "detect_sublight".to_string(),
                CapabilityParams { range: 3.0 },
            )]),
            energy_drain: Amt::milli(100),
            prerequisites: None,
        },
        StructureDefinition {
            id: "ftl_comm_relay".to_string(),
            name: "FTL Comm Relay".to_string(),
            description: "Enables faster-than-light communication across systems.".to_string(),
            max_hp: 50.0,
            cost: ResourceCost {
                minerals: Amt::units(200),
                energy: Amt::units(150),
            },
            build_time: 30,
            capabilities: HashMap::from([(
                "ftl_comm".to_string(),
                CapabilityParams { range: 0.0 },
            )]),
            energy_drain: Amt::milli(500),
            prerequisites: Some(Condition::Atom(ConditionAtom::has_tech(
                "ftl_communications",
            ))),
        },
        StructureDefinition {
            id: "interdictor".to_string(),
            name: "Interdictor".to_string(),
            description: "Disrupts FTL travel within its interdiction range.".to_string(),
            max_hp: 80.0,
            cost: ResourceCost {
                minerals: Amt::units(300),
                energy: Amt::units(200),
            },
            build_time: 45,
            capabilities: HashMap::from([(
                "ftl_interdiction".to_string(),
                CapabilityParams { range: 5.0 },
            )]),
            energy_drain: Amt::units(1),
            prerequisites: Some(Condition::Atom(ConditionAtom::has_tech(
                "ftl_interdiction_tech",
            ))),
        },
    ]
}

pub struct DeepSpacePlugin;

impl Plugin for DeepSpacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StructureRegistry>()
            .add_systems(
                Startup,
                load_structure_definitions.after(crate::scripting::load_all_scripts),
            )
            .add_systems(
                Update,
                sensor_buoy_detect_system
                    .after(crate::time_system::advance_game_time)
                    .after(crate::ship::sublight_movement_system)
                    .after(crate::ship::process_ftl_travel),
            );
    }
}

/// #118: Sensor Buoy detection.
///
/// Each `DeepSpaceStructure` whose definition exposes the `detect_sublight`
/// capability scans for sublight-traveling and idle (docked/surveying/etc.)
/// ships within `range` light-years of its own `Position`. FTL ships are
/// invisible to base sensor buoys (FTL wake detection is gated behind a
/// future tech, #120).
///
/// Detection events are pushed into the player empire's `KnowledgeStore`
/// as `ShipSnapshot`s with an `observed_at` timestamp delayed by
/// `distance(buoy → player) / c`. Existing snapshots with newer
/// `observed_at` are preserved by `KnowledgeStore::update_ship`. When a
/// courier with a `KnowledgeRelay` route visits the buoy's owner system
/// (#117), the relay delivers the same data faster than light — both
/// pathways coexist and the freshest observation wins.
pub fn sensor_buoy_detect_system(
    clock: Res<crate::time_system::GameClock>,
    registry: Res<StructureRegistry>,
    structures: Query<(&DeepSpaceStructure, &crate::components::Position)>,
    ships: Query<(
        Entity,
        &crate::ship::Ship,
        &crate::ship::ShipState,
        &crate::components::Position,
        &crate::ship::ShipHitpoints,
    )>,
    player_q: Query<&crate::player::StationedAt, With<crate::player::Player>>,
    positions: Query<&crate::components::Position>,
    mut empire_q: Query<&mut crate::knowledge::KnowledgeStore, With<crate::player::PlayerEmpire>>,
) {
    use crate::knowledge::{ShipSnapshot, ShipSnapshotState};

    let Ok(mut store) = empire_q.single_mut() else {
        return;
    };
    let Some(stationed) = player_q.iter().next() else {
        return;
    };
    let Ok(player_pos) = positions.get(stationed.system) else {
        return;
    };

    for (structure, buoy_pos) in &structures {
        let Some(def) = registry.get(&structure.definition_id) else {
            continue;
        };
        let Some(cap) = def.capabilities.get("detect_sublight") else {
            continue;
        };
        let detect_range = cap.range;
        if detect_range <= 0.0 {
            continue;
        }

        // Light-speed delay from buoy to player observer.
        let buoy_to_player = crate::physics::distance_ly(buoy_pos, player_pos);
        let delay = crate::physics::light_delay_hexadies(buoy_to_player);
        let observed_at = clock.elapsed - delay;
        if observed_at < 0 {
            continue;
        }

        for (ship_entity, ship, state, ship_pos, hp) in &ships {
            // FTL ships are invisible to baseline sensor buoys (#120 future).
            if matches!(state, crate::ship::ShipState::InFTL { .. }) {
                continue;
            }

            // Range check: distance from buoy to ship.
            let dist = crate::physics::distance_ly(buoy_pos, ship_pos);
            if dist > detect_range {
                continue;
            }

            // Skip if existing knowledge is at least as fresh.
            if store
                .get_ship(ship_entity)
                .is_some_and(|existing| existing.observed_at >= observed_at)
            {
                continue;
            }

            let (snapshot_state, last_system) = match state {
                crate::ship::ShipState::Docked { system } => {
                    (ShipSnapshotState::Docked, Some(*system))
                }
                crate::ship::ShipState::SubLight { target_system, .. } => {
                    (ShipSnapshotState::InTransit, *target_system)
                }
                crate::ship::ShipState::InFTL {
                    destination_system, ..
                } => (ShipSnapshotState::InTransit, Some(*destination_system)),
                crate::ship::ShipState::Surveying { target_system, .. } => {
                    (ShipSnapshotState::Surveying, Some(*target_system))
                }
                crate::ship::ShipState::Settling { system, .. } => {
                    (ShipSnapshotState::Settling, Some(*system))
                }
                crate::ship::ShipState::Refitting { system, .. } => {
                    (ShipSnapshotState::Refitting, Some(*system))
                }
            };

            store.update_ship(ShipSnapshot {
                entity: ship_entity,
                name: ship.name.clone(),
                design_id: ship.design_id.clone(),
                last_known_state: snapshot_state,
                last_known_system: last_system,
                observed_at,
                hp: hp.hull,
                hp_max: hp.hull_max,
            });
        }
    }
}

/// Parse structure definitions from Lua accumulators, falling back to defaults.
/// Scripts are loaded by `load_all_scripts`; this system only parses the results.
pub fn load_structure_definitions(
    engine: Res<crate::scripting::ScriptEngine>,
    mut registry: ResMut<StructureRegistry>,
) {
    match crate::scripting::structure_api::parse_structure_definitions(engine.lua()) {
        Ok(defs) if !defs.is_empty() => {
            let count = defs.len();
            for def in defs {
                registry.insert(def);
            }
            info!("Structure registry loaded with {} definitions", count);
        }
        Ok(_) => {
            info!("No structure definitions found in scripts; using defaults");
            for def in default_structure_definitions() {
                registry.insert(def);
            }
        }
        Err(e) => {
            warn!("Failed to parse structure definitions: {e}; using defaults");
            for def in default_structure_definitions() {
                registry.insert(def);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Position;
    use crate::condition::ConditionAtom;

    #[test]
    fn test_default_structure_definitions() {
        let defs = default_structure_definitions();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].id, "sensor_buoy");
        assert_eq!(defs[1].id, "ftl_comm_relay");
        assert_eq!(defs[2].id, "interdictor");
    }

    #[test]
    fn test_structure_registry_lookup() {
        let mut registry = StructureRegistry::default();
        assert!(registry.get("sensor_buoy").is_none());

        for def in default_structure_definitions() {
            registry.insert(def);
        }

        let buoy = registry.get("sensor_buoy").unwrap();
        assert_eq!(buoy.name, "Sensor Buoy");
        assert_eq!(buoy.max_hp, 20.0);
        assert!(buoy.capabilities.contains_key("detect_sublight"));
        assert_eq!(buoy.capabilities["detect_sublight"].range, 3.0);

        let relay = registry.get("ftl_comm_relay").unwrap();
        assert_eq!(relay.name, "FTL Comm Relay");
        assert!(relay.capabilities.contains_key("ftl_comm"));

        let interdictor = registry.get("interdictor").unwrap();
        assert_eq!(interdictor.name, "Interdictor");
        assert!(interdictor.capabilities.contains_key("ftl_interdiction"));
        assert_eq!(interdictor.capabilities["ftl_interdiction"].range, 5.0);
    }

    #[test]
    fn test_structure_registry_replace() {
        let mut registry = StructureRegistry::default();
        for def in default_structure_definitions() {
            registry.insert(def);
        }

        // Replace sensor_buoy with updated values
        registry.insert(StructureDefinition {
            id: "sensor_buoy".to_string(),
            name: "Advanced Sensor Buoy".to_string(),
            description: "Enhanced sensor buoy.".to_string(),
            max_hp: 40.0,
            cost: ResourceCost {
                minerals: Amt::units(100),
                energy: Amt::units(60),
            },
            build_time: 20,
            capabilities: HashMap::from([
                ("detect_sublight".to_string(), CapabilityParams { range: 5.0 }),
                ("detect_ftl".to_string(), CapabilityParams { range: 3.0 }),
            ]),
            energy_drain: Amt::milli(200),
            prerequisites: Some(Condition::Atom(ConditionAtom::has_tech(
                "advanced_sensors",
            ))),
        });

        assert_eq!(registry.definitions.len(), 3);
        let buoy = registry.get("sensor_buoy").unwrap();
        assert_eq!(buoy.name, "Advanced Sensor Buoy");
        assert_eq!(buoy.max_hp, 40.0);
        assert_eq!(buoy.capabilities["detect_sublight"].range, 5.0);
    }

    #[test]
    fn test_spawn_structure_entity() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let entity = app.world_mut().spawn((
            DeepSpaceStructure {
                definition_id: "sensor_buoy".to_string(),
                name: "Buoy Alpha".to_string(),
                owner: Owner::Neutral,
            },
            StructureHitpoints {
                current: 20.0,
                max: 20.0,
            },
            Position {
                x: 10.0,
                y: 5.0,
                z: 0.0,
            },
        )).id();

        let world = app.world();
        let dss = world.get::<DeepSpaceStructure>(entity).unwrap();
        assert_eq!(dss.definition_id, "sensor_buoy");
        assert_eq!(dss.name, "Buoy Alpha");

        let hp = world.get::<StructureHitpoints>(entity).unwrap();
        assert_eq!(hp.current, 20.0);
        assert_eq!(hp.max, 20.0);

        let pos = world.get::<Position>(entity).unwrap();
        assert!((pos.x - 10.0).abs() < f64::EPSILON);
        assert!((pos.y - 5.0).abs() < f64::EPSILON);
    }
}
