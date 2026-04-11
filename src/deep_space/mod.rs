use std::collections::HashMap;
use std::path::Path;

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
            prerequisites: Some(Condition::Atom(ConditionAtom::HasTech(
                "ftl_communications".to_string(),
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
            prerequisites: Some(Condition::Atom(ConditionAtom::HasTech(
                "ftl_interdiction_tech".to_string(),
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
                load_structure_definitions.after(crate::scripting::init_scripting),
            );
    }
}

/// Startup system that loads structure definitions from Lua scripts, falling back
/// to hardcoded defaults if the scripts directory is missing.
pub fn load_structure_definitions(
    engine: Res<crate::scripting::ScriptEngine>,
    mut registry: ResMut<StructureRegistry>,
) {
    let structure_dir = Path::new("scripts/structures");
    if structure_dir.exists() {
        match engine.load_directory(structure_dir) {
            Err(e) => {
                warn!("Failed to load structure scripts: {e}; using default definitions");
                for def in default_structure_definitions() {
                    registry.insert(def);
                }
            }
            Ok(()) => match crate::scripting::structure_api::parse_structure_definitions(
                engine.lua(),
            ) {
                Ok(defs) => {
                    if defs.is_empty() {
                        info!("No structure definitions found in scripts; using defaults");
                        for def in default_structure_definitions() {
                            registry.insert(def);
                        }
                    } else {
                        let count = defs.len();
                        for def in defs {
                            registry.insert(def);
                        }
                        info!("Structure registry loaded with {} definitions", count);
                    }
                }
                Err(e) => {
                    warn!("Failed to parse structure definitions: {e}; using defaults");
                    for def in default_structure_definitions() {
                        registry.insert(def);
                    }
                }
            },
        }
    } else {
        info!("scripts/structures directory not found; using default structure definitions");
        for def in default_structure_definitions() {
            registry.insert(def);
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
            prerequisites: Some(Condition::Atom(ConditionAtom::HasTech(
                "advanced_sensors".to_string(),
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
