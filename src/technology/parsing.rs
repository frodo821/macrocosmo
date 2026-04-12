use crate::amount::Amt;

use super::tree::{TechBranchDefinition, TechCost, TechId, TechTree, Technology};

/// Parse tech branch definitions from the Lua `_tech_branch_definitions` accumulator.
/// Each entry must contain at minimum `id` and `name`; `color` defaults to grey
/// when absent and `icon` is optional.
pub fn parse_tech_branch_definitions(
    lua: &mlua::Lua,
) -> Result<Vec<TechBranchDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_tech_branch_definitions")?;
    let mut branches = Vec::new();
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id: String = table.get("id")?;
        let name: String = table.get::<Option<String>>("name")?.unwrap_or_else(|| id.clone());
        let color = match table.get::<mlua::Value>("color")? {
            mlua::Value::Table(t) => {
                let r: f32 = t.get::<f64>(1).unwrap_or(0.5) as f32;
                let g: f32 = t.get::<f64>(2).unwrap_or(0.5) as f32;
                let b: f32 = t.get::<f64>(3).unwrap_or(0.5) as f32;
                [r, g, b]
            }
            _ => [0.5, 0.5, 0.5],
        };
        let icon: Option<String> = table.get::<Option<String>>("icon")?;
        branches.push(TechBranchDefinition { id, name, color, icon });
    }
    Ok(branches)
}

/// Read `_tech_definitions` from the Lua state and convert to `Vec<Technology>`.
/// The `on_researched` callback stays in the Lua table and is not extracted here;
/// it will be invoked by the scripting system when research completes.
///
/// `branch` is parsed as a string and is not validated here against any registry —
/// validation (warning on unknown branches) happens at registry-load time so that
/// definition order between branches and techs need not be strictly enforced
/// inside this pure parser.
pub fn parse_tech_definitions(lua: &mlua::Lua) -> Result<Vec<Technology>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_tech_definitions")?;
    let mut techs = Vec::new();
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id = TechId(table.get::<String>("id")?);
        let name: String = table.get("name")?;
        let branch: String = table.get("branch")?;
        // Support both scalar cost (backward compat: research-only) and table cost
        let cost: TechCost = match table.get::<mlua::Value>("cost")? {
            mlua::Value::Number(n) => TechCost {
                research: Amt::from_f64(n),
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
            },
            mlua::Value::Integer(n) => TechCost {
                research: Amt::units(n as u64),
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
            },
            mlua::Value::Table(t) => TechCost {
                research: Amt::from_f64(t.get::<f64>("research").unwrap_or(0.0)),
                minerals: Amt::from_f64(t.get::<f64>("minerals").unwrap_or(0.0)),
                energy: Amt::from_f64(t.get::<f64>("energy").unwrap_or(0.0)),
            },
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "cost must be a number or table".to_string(),
                ))
            }
        };

        let prereqs_table: mlua::Table = table.get("prerequisites")?;
        let prerequisites: Vec<TechId> = prereqs_table
            .sequence_values::<mlua::Value>()
            .map(|r| {
                let val = r?;
                crate::scripting::extract_ref_id(&val).map(TechId)
            })
            .collect::<Result<_, _>>()?;

        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();

        techs.push(Technology {
            id,
            name,
            branch,
            cost,
            prerequisites,
            description,
        });
    }
    Ok(techs)
}

/// Hardcoded fallback tech definitions used when Lua scripts are unavailable.
pub fn create_initial_tech_tree_vec() -> Vec<Technology> {
    vec![
        // === Social Branch ===
        Technology {
            id: TechId("social_xenolinguistics".into()),
            name: "Xenolinguistics".into(),
            branch: "social".into(),
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: "Foundational study of alien communication patterns".into(),
        },
        Technology {
            id: TechId("social_colonial_admin".into()),
            name: "Colonial Administration".into(),
            branch: "social".into(),
            cost: TechCost::research_only(Amt::units(150)),
            prerequisites: vec![],
            description: "Improved governance structures for distant colonies".into(),
        },
        Technology {
            id: TechId("social_interstellar_commerce".into()),
            name: "Interstellar Commerce".into(),
            branch: "social".into(),
            cost: TechCost::research_only(Amt::units(250)),
            prerequisites: vec![TechId("social_colonial_admin".into())],
            description: "Trade frameworks spanning star systems".into(),
        },
        Technology {
            id: TechId("social_cultural_exchange".into()),
            name: "Cultural Exchange Protocols".into(),
            branch: "social".into(),
            cost: TechCost::research_only(Amt::units(300)),
            prerequisites: vec![TechId("social_xenolinguistics".into())],
            description: "Formalised frameworks for cross-species cultural interaction".into(),
        },
        // === Physics Branch ===
        Technology {
            id: TechId("physics_sensor_arrays".into()),
            name: "Advanced Sensor Arrays".into(),
            branch: "physics".into(),
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: "Next-generation sensors for deep space observation".into(),
        },
        Technology {
            id: TechId("physics_sublight_drives".into()),
            name: "Improved Sublight Drives".into(),
            branch: "physics".into(),
            cost: TechCost::research_only(Amt::units(200)),
            prerequisites: vec![],
            description: "Enhances sublight drive efficiency".into(),
        },
        Technology {
            id: TechId("physics_ftl_theory".into()),
            name: "FTL Theory".into(),
            branch: "physics".into(),
            cost: TechCost::research_only(Amt::units(400)),
            prerequisites: vec![TechId("physics_sublight_drives".into())],
            description: "Theoretical foundations for faster-than-light travel".into(),
        },
        Technology {
            id: TechId("physics_warp_stabilisation".into()),
            name: "Warp Field Stabilisation".into(),
            branch: "physics".into(),
            cost: TechCost::research_only(Amt::units(600)),
            prerequisites: vec![TechId("physics_ftl_theory".into())],
            description: "Stabilise warp fields for safer FTL travel".into(),
        },
        // === Industrial Branch ===
        Technology {
            id: TechId("industrial_automated_mining".into()),
            name: "Automated Mining".into(),
            branch: "industrial".into(),
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: "Robotic systems for autonomous resource extraction".into(),
        },
        Technology {
            id: TechId("industrial_orbital_fabrication".into()),
            name: "Orbital Fabrication".into(),
            branch: "industrial".into(),
            cost: TechCost::research_only(Amt::units(200)),
            prerequisites: vec![TechId("industrial_automated_mining".into())],
            description: "Manufacturing facilities in orbit for zero-gravity construction".into(),
        },
        Technology {
            id: TechId("industrial_fusion_power".into()),
            name: "Fusion Power Plants".into(),
            branch: "industrial".into(),
            cost: TechCost::research_only(Amt::units(300)),
            prerequisites: vec![TechId("industrial_automated_mining".into())],
            description: "Harness fusion reactions for abundant clean energy".into(),
        },
        Technology {
            id: TechId("industrial_nano_assembly".into()),
            name: "Nano-Assembly".into(),
            branch: "industrial".into(),
            cost: TechCost::research_only(Amt::units(500)),
            prerequisites: vec![TechId("industrial_orbital_fabrication".into())],
            description: "Molecular-scale construction for unprecedented precision".into(),
        },
        // === Military Branch ===
        Technology {
            id: TechId("military_kinetic_weapons".into()),
            name: "Kinetic Weapons".into(),
            branch: "military".into(),
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: vec![],
            description: "Mass-driver based weapon systems".into(),
        },
        Technology {
            id: TechId("military_deflector_shields".into()),
            name: "Deflector Shields".into(),
            branch: "military".into(),
            cost: TechCost::research_only(Amt::units(200)),
            prerequisites: vec![],
            description: "Energy barriers to deflect incoming projectiles".into(),
        },
        Technology {
            id: TechId("military_composite_armor".into()),
            name: "Composite Armor".into(),
            branch: "military".into(),
            cost: TechCost::research_only(Amt::units(250)),
            prerequisites: vec![TechId("military_kinetic_weapons".into())],
            description: "Multi-layered hull plating for enhanced protection".into(),
        },
    ]
}

/// Convenience function: build a TechTree from hardcoded definitions.
pub fn create_initial_tech_tree() -> TechTree {
    TechTree::from_vec(create_initial_tech_tree_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardcoded_tech_tree() {
        let tree = create_initial_tech_tree();
        assert_eq!(tree.technologies.len(), 15);
        assert!(tree.get(&TechId("social_xenolinguistics".into())).is_some());
        assert!(tree.get(&TechId("military_composite_armor".into())).is_some());
    }

    #[test]
    fn test_parse_lua_tech_definitions() {
        let lua = mlua::Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua, &crate::scripting::resolve_scripts_dir()).unwrap();

        lua.load(
            r#"
            define_tech {
                id = "physics_test",
                name = "Test Tech",
                branch = "physics",
                cost = 42.0,
                prerequisites = {},
                description = "A test technology",
                on_researched = function()
                    -- TODO: push_empire_modifier
                end,
            }
            define_tech {
                id = "military_advanced_test",
                name = "Advanced Test Tech",
                branch = "military",
                cost = 100.0,
                prerequisites = { "physics_test" },
                description = "Depends on test tech",
                on_researched = function()
                    -- TODO: push_empire_modifier
                end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let techs = parse_tech_definitions(&lua).unwrap();
        assert_eq!(techs.len(), 2);

        let first = &techs[0];
        assert_eq!(first.id, TechId("physics_test".into()));
        assert_eq!(first.name, "Test Tech");
        assert_eq!(first.branch, "physics");
        assert_eq!(first.cost.research, Amt::units(42));
        assert!(first.prerequisites.is_empty());

        let second = &techs[1];
        assert_eq!(second.id, TechId("military_advanced_test".into()));
        assert_eq!(second.prerequisites, vec![TechId("physics_test".into())]);
    }

    #[test]
    fn test_parse_lua_tech_table_cost() {
        let lua = mlua::Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua, &crate::scripting::resolve_scripts_dir()).unwrap();

        lua.load(
            r#"
            define_tech {
                id = "industrial_expensive",
                name = "Expensive Tech",
                branch = "industrial",
                cost = { research = 200.0, minerals = 50.0, energy = 30.0 },
                prerequisites = {},
                description = "A tech with table cost",
                on_researched = function() end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let techs = parse_tech_definitions(&lua).unwrap();
        assert_eq!(techs.len(), 1);
        let tech = &techs[0];
        assert_eq!(tech.cost.research, Amt::units(200));
        assert_eq!(tech.cost.minerals, Amt::units(50));
        assert_eq!(tech.cost.energy, Amt::units(30));
    }

    #[test]
    fn test_load_lua_files_from_disk() {
        let engine = crate::scripting::ScriptEngine::new().unwrap();
        let init_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/init.lua");
        engine
            .load_file(&init_path)
            .expect("Failed to load scripts via init.lua");
        let techs = parse_tech_definitions(engine.lua()).expect("Failed to parse tech scripts");
        // Should load all 15 technologies from the 4 Lua files
        assert_eq!(techs.len(), 15);
        // Verify one tech from each branch
        assert!(techs
            .iter()
            .any(|t| t.id == TechId("social_xenolinguistics".into()) && t.branch == "social"));
        assert!(techs
            .iter()
            .any(|t| t.id == TechId("physics_sublight_drives".into()) && t.branch == "physics"));
        assert!(techs
            .iter()
            .any(|t| t.id == TechId("industrial_automated_mining".into()) && t.branch == "industrial"));
        assert!(techs
            .iter()
            .any(|t| t.id == TechId("military_composite_armor".into()) && t.branch == "military"));
    }

    #[test]
    fn test_parse_tech_branch_definitions() {
        let lua = mlua::Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua, &crate::scripting::resolve_scripts_dir())
            .unwrap();

        lua.load(
            r#"
            define_tech_branch {
                id = "alpha",
                name = "Alpha",
                color = { 0.1, 0.2, 0.3 },
            }
            define_tech_branch {
                id = "beta",
                name = "Beta",
                color = { 0.4, 0.5, 0.6 },
                icon = "icons/beta.png",
            }
            "#,
        )
        .exec()
        .unwrap();

        let branches = parse_tech_branch_definitions(&lua).unwrap();
        assert_eq!(branches.len(), 2);

        assert_eq!(branches[0].id, "alpha");
        assert_eq!(branches[0].name, "Alpha");
        assert!((branches[0].color[0] - 0.1).abs() < 1e-5);
        assert!((branches[0].color[1] - 0.2).abs() < 1e-5);
        assert!((branches[0].color[2] - 0.3).abs() < 1e-5);
        assert!(branches[0].icon.is_none());

        assert_eq!(branches[1].id, "beta");
        assert_eq!(branches[1].icon.as_deref(), Some("icons/beta.png"));
    }
}
