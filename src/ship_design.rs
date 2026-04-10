use std::collections::HashMap;

use bevy::prelude::*;

use crate::amount::Amt;

/// Defines a module slot type (weapon, utility, engine, special).
#[derive(Clone, Debug)]
pub struct SlotTypeDefinition {
    pub id: String,
    pub name: String,
}

#[derive(Resource, Default)]
pub struct SlotTypeRegistry {
    pub types: HashMap<String, SlotTypeDefinition>,
}

impl SlotTypeRegistry {
    pub fn get(&self, id: &str) -> Option<&SlotTypeDefinition> {
        self.types.get(id)
    }

    pub fn insert(&mut self, def: SlotTypeDefinition) {
        self.types.insert(def.id.clone(), def);
    }
}

/// A slot on a hull.
#[derive(Clone, Debug)]
pub struct HullSlot {
    pub slot_type: String,
    pub count: u32,
}

/// Defines a ship hull.
#[derive(Clone, Debug)]
pub struct HullDefinition {
    pub id: String,
    pub name: String,
    pub base_hp: f64,
    pub base_speed: f64,
    pub base_evasion: f64,
    pub slots: Vec<HullSlot>,
    pub build_cost_minerals: Amt,
    pub build_cost_energy: Amt,
    pub build_time: i64,
    pub maintenance: Amt,
}

#[derive(Resource, Default)]
pub struct HullRegistry {
    pub hulls: HashMap<String, HullDefinition>,
}

impl HullRegistry {
    pub fn get(&self, id: &str) -> Option<&HullDefinition> {
        self.hulls.get(id)
    }

    pub fn insert(&mut self, def: HullDefinition) {
        self.hulls.insert(def.id.clone(), def);
    }
}

/// Weapon-specific stats for a module.
#[derive(Clone, Debug)]
pub struct WeaponStats {
    pub track: f64,
    pub precision: f64,
    pub cooldown: i64,
    pub range: f64,
    pub shield_damage: f64,
    pub shield_damage_div: f64,
    pub shield_piercing: f64,
    pub armor_damage: f64,
    pub armor_damage_div: f64,
    pub armor_piercing: f64,
    pub hull_damage: f64,
    pub hull_damage_div: f64,
}

/// A modifier that a module applies when equipped.
#[derive(Clone, Debug)]
pub struct ModuleModifier {
    pub target: String,
    pub base_add: f64,
    pub multiplier: f64,
    pub add: f64,
}

/// Defines a ship module.
#[derive(Clone, Debug)]
pub struct ModuleDefinition {
    pub id: String,
    pub name: String,
    pub slot_type: String,
    pub modifiers: Vec<ModuleModifier>,
    pub weapon: Option<WeaponStats>,
    pub cost_minerals: Amt,
    pub cost_energy: Amt,
    pub prerequisite_tech: Option<String>,
}

#[derive(Resource, Default)]
pub struct ModuleRegistry {
    pub modules: HashMap<String, ModuleDefinition>,
}

impl ModuleRegistry {
    pub fn get(&self, id: &str) -> Option<&ModuleDefinition> {
        self.modules.get(id)
    }

    pub fn insert(&mut self, def: ModuleDefinition) {
        self.modules.insert(def.id.clone(), def);
    }
}

/// A slot assignment in a ship design.
#[derive(Clone, Debug)]
pub struct DesignSlotAssignment {
    pub slot_type: String,
    pub module_id: String,
}

/// A complete ship design (template).
#[derive(Clone, Debug)]
pub struct ShipDesignDefinition {
    pub id: String,
    pub name: String,
    pub hull_id: String,
    pub modules: Vec<DesignSlotAssignment>,
}

#[derive(Resource, Default)]
pub struct ShipDesignRegistry {
    pub designs: HashMap<String, ShipDesignDefinition>,
}

impl ShipDesignRegistry {
    pub fn get(&self, id: &str) -> Option<&ShipDesignDefinition> {
        self.designs.get(id)
    }

    pub fn insert(&mut self, def: ShipDesignDefinition) {
        self.designs.insert(def.id.clone(), def);
    }
}

/// Plugin that loads ship design definitions from Lua scripts.
pub struct ShipDesignPlugin;

impl Plugin for ShipDesignPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SlotTypeRegistry>()
            .init_resource::<HullRegistry>()
            .init_resource::<ModuleRegistry>()
            .init_resource::<ShipDesignRegistry>()
            .add_systems(
                Startup,
                load_ship_designs.after(crate::scripting::init_scripting),
            );
    }
}

/// Load ship design definitions from Lua scripts into registries.
/// Falls back to empty registries if scripts are missing or fail to parse.
pub fn load_ship_designs(
    engine: Res<crate::scripting::ScriptEngine>,
    mut slot_types: ResMut<SlotTypeRegistry>,
    mut hulls: ResMut<HullRegistry>,
    mut modules: ResMut<ModuleRegistry>,
    mut designs: ResMut<ShipDesignRegistry>,
) {
    use crate::scripting::ship_design_api;
    use std::path::Path;

    let ships_dir = Path::new("scripts/ships");
    if !ships_dir.exists() {
        info!("scripts/ships directory not found; ship design registries will be empty");
        return;
    }

    if let Err(e) = engine.load_directory(ships_dir) {
        warn!("Failed to load ship design scripts: {e}; registries will be empty");
        return;
    }

    // Parse slot types
    match ship_design_api::parse_slot_types(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                slot_types.insert(def);
            }
            info!("Loaded {} slot type definitions", count);
        }
        Err(e) => warn!("Failed to parse slot type definitions: {e}"),
    }

    // Parse hulls
    match ship_design_api::parse_hulls(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                hulls.insert(def);
            }
            info!("Loaded {} hull definitions", count);
        }
        Err(e) => warn!("Failed to parse hull definitions: {e}"),
    }

    // Parse modules
    match ship_design_api::parse_modules(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                modules.insert(def);
            }
            info!("Loaded {} module definitions", count);
        }
        Err(e) => warn!("Failed to parse module definitions: {e}"),
    }

    // Parse ship designs
    match ship_design_api::parse_ship_designs(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                designs.insert(def);
            }
            info!("Loaded {} ship design definitions", count);
        }
        Err(e) => warn!("Failed to parse ship design definitions: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hull_registry_lookup() {
        let mut registry = HullRegistry::default();
        assert!(registry.get("corvette").is_none());

        registry.insert(HullDefinition {
            id: "corvette".to_string(),
            name: "Corvette".to_string(),
            base_hp: 50.0,
            base_speed: 0.75,
            base_evasion: 30.0,
            slots: vec![
                HullSlot { slot_type: "weapon".to_string(), count: 2 },
                HullSlot { slot_type: "engine".to_string(), count: 1 },
            ],
            build_cost_minerals: Amt::units(200),
            build_cost_energy: Amt::units(100),
            build_time: 60,
            maintenance: Amt::new(0, 500),
        });

        let corvette = registry.get("corvette").unwrap();
        assert_eq!(corvette.name, "Corvette");
        assert_eq!(corvette.base_hp, 50.0);
        assert_eq!(corvette.slots.len(), 2);
        assert_eq!(corvette.build_cost_minerals, Amt::units(200));
    }

    #[test]
    fn test_module_registry_lookup() {
        let mut registry = ModuleRegistry::default();
        assert!(registry.get("ftl_drive").is_none());

        registry.insert(ModuleDefinition {
            id: "ftl_drive".to_string(),
            name: "FTL Drive".to_string(),
            slot_type: "engine".to_string(),
            modifiers: vec![ModuleModifier {
                target: "ship.ftl_range".to_string(),
                base_add: 15.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::units(100),
            cost_energy: Amt::units(50),
            prerequisite_tech: None,
        });

        let ftl = registry.get("ftl_drive").unwrap();
        assert_eq!(ftl.name, "FTL Drive");
        assert_eq!(ftl.slot_type, "engine");
        assert_eq!(ftl.modifiers.len(), 1);
        assert_eq!(ftl.modifiers[0].target, "ship.ftl_range");
        assert!(ftl.weapon.is_none());
    }

    #[test]
    fn test_design_registry_lookup() {
        let mut registry = ShipDesignRegistry::default();
        assert!(registry.get("explorer_mk1").is_none());

        registry.insert(ShipDesignDefinition {
            id: "explorer_mk1".to_string(),
            name: "Explorer Mk.I".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![
                DesignSlotAssignment {
                    slot_type: "engine".to_string(),
                    module_id: "ftl_drive".to_string(),
                },
                DesignSlotAssignment {
                    slot_type: "utility".to_string(),
                    module_id: "survey_equipment".to_string(),
                },
            ],
        });

        let explorer = registry.get("explorer_mk1").unwrap();
        assert_eq!(explorer.name, "Explorer Mk.I");
        assert_eq!(explorer.hull_id, "corvette");
        assert_eq!(explorer.modules.len(), 2);
        assert_eq!(explorer.modules[0].module_id, "ftl_drive");
    }
}
