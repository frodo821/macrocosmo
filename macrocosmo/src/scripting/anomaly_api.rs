use bevy::prelude::*;
use rand::Rng;

/// A Lua-defined anomaly that can be discovered during surveys.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct AnomalyDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Probability weight for weighted random selection.
    pub weight: u32,
    /// Effects applied when this anomaly is discovered.
    pub effects: Vec<AnomalyEffectDef>,
}

/// An effect that an anomaly applies when discovered.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub enum AnomalyEffectDef {
    /// Upgrade a resource level (minerals/energy/research).
    ResourceBonus { resource: String },
    /// Grant a one-time research bonus.
    ResearchBonus { amount: f64 },
    /// Add extra building slots to the system.
    BuildingSlots { extra: u8 },
    /// Deal damage to the surveying ship (percentage of max hull).
    Hazard { damage_percent: f64 },
}

/// Registry of all anomaly definitions loaded from Lua scripts.
#[derive(Resource, Default, Debug, Reflect)]
#[reflect(Resource)]
pub struct AnomalyRegistry {
    pub anomalies: Vec<AnomalyDefinition>,
}

impl AnomalyRegistry {
    /// Roll for anomaly discovery. Returns None 40% of the time,
    /// otherwise weighted random selection from the registry.
    pub fn roll_discovery(&self, rng: &mut impl Rng) -> Option<&AnomalyDefinition> {
        if self.anomalies.is_empty() {
            return None;
        }

        // 40% chance of nothing
        let roll: f64 = rng.random_range(0.0..1.0);
        if roll < 0.40 {
            return None;
        }

        // Weighted random selection from registry
        let total_weight: u32 = self.anomalies.iter().map(|a| a.weight).sum();
        if total_weight == 0 {
            return None;
        }

        let mut pick = rng.random_range(0..total_weight);
        for anomaly in &self.anomalies {
            if pick < anomaly.weight {
                return Some(anomaly);
            }
            pick -= anomaly.weight;
        }

        // Fallback (shouldn't reach here)
        self.anomalies.last()
    }
}

/// Parse anomaly definitions from the Lua `_anomaly_definitions` global table.
pub fn parse_anomaly_definitions(lua: &mlua::Lua) -> Result<Vec<AnomalyDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_anomaly_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();
        let weight: u32 = table.get::<Option<u32>>("weight")?.unwrap_or(10);

        let effects = parse_effects(&table)?;

        result.push(AnomalyDefinition {
            id,
            name,
            description,
            weight,
            effects,
        });
    }

    Ok(result)
}

/// Parse the `effects` array from an anomaly definition table.
fn parse_effects(table: &mlua::Table) -> Result<Vec<AnomalyEffectDef>, mlua::Error> {
    let effects_value: mlua::Value = table.get("effects")?;
    let effects_table = match effects_value {
        mlua::Value::Table(t) => t,
        mlua::Value::Nil => return Ok(Vec::new()),
        _ => {
            return Err(mlua::Error::RuntimeError(
                "Expected table or nil for 'effects' field".to_string(),
            ));
        }
    };

    let mut effects = Vec::new();
    for pair in effects_table.pairs::<i64, mlua::Table>() {
        let (_, effect_table) = pair?;
        let effect_type: String = effect_table.get("type")?;

        let effect = match effect_type.as_str() {
            "resource_bonus" => {
                let resource: String = effect_table
                    .get::<Option<String>>("resource")?
                    .unwrap_or_else(|| "minerals".to_string());
                AnomalyEffectDef::ResourceBonus { resource }
            }
            "research_bonus" => {
                let amount: f64 = effect_table.get::<Option<f64>>("amount")?.unwrap_or(100.0);
                AnomalyEffectDef::ResearchBonus { amount }
            }
            "building_slots" => {
                let extra: u8 = effect_table.get::<Option<u8>>("extra")?.unwrap_or(1);
                AnomalyEffectDef::BuildingSlots { extra }
            }
            "hazard" => {
                let damage_percent: f64 = effect_table
                    .get::<Option<f64>>("damage_percent")?
                    .unwrap_or(20.0);
                AnomalyEffectDef::Hazard { damage_percent }
            }
            other => {
                warn!("Unknown anomaly effect type: {}", other);
                continue;
            }
        };

        effects.push(effect);
    }

    Ok(effects)
}

/// Startup system: parse Lua anomaly definitions into the AnomalyRegistry resource.
pub fn load_anomaly_registry(engine: Res<crate::scripting::ScriptEngine>, mut commands: Commands) {
    match parse_anomaly_definitions(engine.lua()) {
        Ok(anomalies) => {
            info!("Loaded {} anomaly definitions from Lua", anomalies.len());
            commands.insert_resource(AnomalyRegistry { anomalies });
        }
        Err(e) => {
            warn!("Failed to parse anomaly definitions: {e}; using empty registry");
            commands.insert_resource(AnomalyRegistry::default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_anomaly_definitions() {
        let engine = crate::scripting::ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_anomaly {
                id = "test_mineral",
                name = "Test Mineral",
                description = "A test mineral vein",
                weight = 15,
                effects = {
                    { type = "resource_bonus", resource = "minerals" },
                },
            }
            define_anomaly {
                id = "test_ruins",
                name = "Test Ruins",
                description = "Ancient test ruins",
                weight = 10,
                effects = {
                    { type = "research_bonus", amount = 150 },
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_anomaly_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].id, "test_mineral");
        assert_eq!(defs[0].weight, 15);
        assert_eq!(defs[0].effects.len(), 1);
        assert!(
            matches!(&defs[0].effects[0], AnomalyEffectDef::ResourceBonus { resource } if resource == "minerals")
        );
        assert_eq!(defs[1].id, "test_ruins");
        assert!(
            matches!(&defs[1].effects[0], AnomalyEffectDef::ResearchBonus { amount } if (*amount - 150.0).abs() < 0.01)
        );
    }

    #[test]
    fn test_anomaly_registry_roll_empty() {
        let registry = AnomalyRegistry::default();
        let mut rng = rand::rng();
        assert!(registry.roll_discovery(&mut rng).is_none());
    }

    #[test]
    fn test_anomaly_registry_roll_returns_valid() {
        let registry = AnomalyRegistry {
            anomalies: vec![AnomalyDefinition {
                id: "test".into(),
                name: "Test".into(),
                description: "Desc".into(),
                weight: 100,
                effects: vec![],
            }],
        };
        let mut rng = rand::rng();
        // Run many times; should get Some at least once (60% chance each time)
        let mut found = false;
        for _ in 0..100 {
            if registry.roll_discovery(&mut rng).is_some() {
                found = true;
                break;
            }
        }
        assert!(found, "Expected at least one discovery in 100 rolls");
    }
}
