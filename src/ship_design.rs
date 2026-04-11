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
    pub description: String,
    pub base_hp: f64,
    pub base_speed: f64,
    pub base_evasion: f64,
    pub slots: Vec<HullSlot>,
    pub build_cost_minerals: Amt,
    pub build_cost_energy: Amt,
    pub build_time: i64,
    pub maintenance: Amt,
    pub modifiers: Vec<ModuleModifier>,
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

/// An upgrade path from one module to another.
#[derive(Clone, Debug)]
pub struct ModuleUpgradePath {
    /// Target module ID to upgrade to.
    pub target_id: String,
    /// Mineral cost of the upgrade.
    pub cost_minerals: Amt,
    /// Energy cost of the upgrade.
    pub cost_energy: Amt,
}

/// Defines a ship module.
#[derive(Clone, Debug)]
pub struct ModuleDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub slot_type: String,
    pub modifiers: Vec<ModuleModifier>,
    pub weapon: Option<WeaponStats>,
    pub cost_minerals: Amt,
    pub cost_energy: Amt,
    pub prerequisite_tech: Option<String>,
    /// Available upgrade paths from this module.
    pub upgrade_to: Vec<ModuleUpgradePath>,
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
    pub description: String,
    pub hull_id: String,
    pub modules: Vec<DesignSlotAssignment>,
    /// Whether this design can perform surveys.
    pub can_survey: bool,
    /// Whether this design can colonize planets.
    pub can_colonize: bool,
    /// Energy maintenance cost per hexadies.
    pub maintenance: Amt,
    /// Mineral cost to build this design.
    pub build_cost_minerals: Amt,
    /// Energy cost to build this design.
    pub build_cost_energy: Amt,
    /// Build time in hexadies.
    pub build_time: i64,
    /// Hull hitpoints.
    pub hp: f64,
    /// Sub-light speed (fraction of c).
    pub sublight_speed: f64,
    /// FTL range in light-years (0 = no FTL).
    pub ftl_range: f64,
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

    /// Check if a design can perform surveys.
    pub fn can_survey(&self, id: &str) -> bool {
        self.designs.get(id).map(|d| d.can_survey).unwrap_or(false)
    }

    /// Check if a design can colonize planets.
    pub fn can_colonize(&self, id: &str) -> bool {
        self.designs.get(id).map(|d| d.can_colonize).unwrap_or(false)
    }

    /// Get build cost (minerals, energy) for a design.
    pub fn build_cost(&self, id: &str) -> (Amt, Amt) {
        self.designs
            .get(id)
            .map(|d| (d.build_cost_minerals, d.build_cost_energy))
            .unwrap_or((Amt::units(200), Amt::units(100)))
    }

    /// Get build time in hexadies for a design.
    pub fn build_time(&self, id: &str) -> i64 {
        self.designs.get(id).map(|d| d.build_time).unwrap_or(60)
    }

    /// Get maintenance cost per hexadies for a design.
    pub fn maintenance(&self, id: &str) -> Amt {
        self.designs
            .get(id)
            .map(|d| d.maintenance)
            .unwrap_or(Amt::new(0, 500))
    }

    /// Scrap refund: 50% of (hull build cost + equipped module costs).
    pub fn scrap_refund(
        &self,
        id: &str,
        modules: &[crate::ship::EquippedModule],
        module_registry: &ModuleRegistry,
    ) -> (Amt, Amt) {
        let (hull_m, hull_e) = self.build_cost(id);
        let mut total_m = hull_m;
        let mut total_e = hull_e;
        for equipped in modules {
            if let Some(def) = module_registry.get(&equipped.module_id) {
                total_m = total_m.add(def.cost_minerals);
                total_e = total_e.add(def.cost_energy);
            }
        }
        (Amt::milli(total_m.raw() / 2), Amt::milli(total_e.raw() / 2))
    }

    /// Get all design IDs sorted alphabetically.
    pub fn all_design_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.designs.keys().cloned().collect();
        ids.sort();
        ids
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
                load_ship_designs.after(crate::scripting::load_all_scripts),
            );
    }
}

/// Parse ship design definitions from Lua accumulators into registries.
/// Scripts are loaded by `load_all_scripts`; this system only parses the results.
pub fn load_ship_designs(
    engine: Res<crate::scripting::ScriptEngine>,
    mut slot_types: ResMut<SlotTypeRegistry>,
    mut hulls: ResMut<HullRegistry>,
    mut modules: ResMut<ModuleRegistry>,
    mut designs: ResMut<ShipDesignRegistry>,
) {
    use crate::scripting::ship_design_api;

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

/// Compute total cost for a ship design: hull cost + sum of module costs.
/// Returns (minerals, energy, build_time, maintenance).
pub fn design_cost(
    hull: &HullDefinition,
    modules: &[&ModuleDefinition],
) -> (Amt, Amt, i64, Amt) {
    let mut minerals = hull.build_cost_minerals;
    let mut energy = hull.build_cost_energy;
    let mut maintenance = hull.maintenance;
    for m in modules {
        minerals = minerals.add(m.cost_minerals);
        energy = energy.add(m.cost_energy);
        // Each module adds 10% of its mineral cost as maintenance
        maintenance = maintenance.add(Amt::milli(m.cost_minerals.raw() / 10));
    }
    (minerals, energy, hull.build_time, maintenance)
}

/// Compute total stats for a design: HP, speed, evasion from hull + module modifiers.
pub fn design_stats(
    hull: &HullDefinition,
    modules: &[&ModuleDefinition],
) -> (f64, f64, f64) {
    let mut hp = hull.base_hp;
    let mut speed = hull.base_speed;
    let mut evasion = hull.base_evasion;
    for m in modules {
        for modifier in &m.modifiers {
            match modifier.target.as_str() {
                "ship.speed" => speed += modifier.base_add,
                "ship.evasion" => evasion += modifier.base_add,
                // HP modifiers affect hull HP
                _ => {}
            }
        }
    }
    (hp, speed, evasion)
}

/// Compute refit cost: new module cost - 50% of old module value.
/// Returns (minerals, energy, time).
pub fn refit_cost(
    old_modules: &[&ModuleDefinition],
    new_modules: &[&ModuleDefinition],
    hull: &HullDefinition,
) -> (Amt, Amt, i64) {
    let mut old_m = Amt::ZERO;
    let mut old_e = Amt::ZERO;
    for m in old_modules {
        old_m = old_m.add(m.cost_minerals);
        old_e = old_e.add(m.cost_energy);
    }
    let mut new_m = Amt::ZERO;
    let mut new_e = Amt::ZERO;
    for m in new_modules {
        new_m = new_m.add(m.cost_minerals);
        new_e = new_e.add(m.cost_energy);
    }
    // Refund 50% of old module value
    let refund_m = Amt::milli(old_m.raw() / 2);
    let refund_e = Amt::milli(old_e.raw() / 2);
    let cost_m = if new_m > refund_m { new_m.sub(refund_m) } else { Amt::ZERO };
    let cost_e = if new_e > refund_e { new_e.sub(refund_e) } else { Amt::ZERO };
    // Refit time: half hull build time
    let time = hull.build_time / 2;
    (cost_m, cost_e, time.max(1))
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
            description: String::new(),
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
            modifiers: vec![],
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
            description: String::new(),
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
            upgrade_to: Vec::new(),
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
            description: String::new(),
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
            can_survey: true,
            can_colonize: false,
            maintenance: Amt::new(0, 500),
            build_cost_minerals: Amt::units(200),
            build_cost_energy: Amt::units(100),
            build_time: 60,
            hp: 50.0,
            sublight_speed: 0.75,
            ftl_range: 10.0,
        });

        let explorer = registry.get("explorer_mk1").unwrap();
        assert_eq!(explorer.name, "Explorer Mk.I");
        assert_eq!(explorer.hull_id, "corvette");
        assert_eq!(explorer.modules.len(), 2);
        assert_eq!(explorer.modules[0].module_id, "ftl_drive");
    }
}
