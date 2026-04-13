//! Reverse index from technology IDs to the things they unlock.
//!
//! For each tech, lists the modules, buildings, structures, and dependent
//! techs that become available when it is researched. Built once at startup
//! after all registries are loaded; consumed by the research panel UI.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::condition::{AtomKind, Condition};
use crate::deep_space::StructureRegistry;
use crate::scripting::building_api::BuildingRegistry;
use crate::ship_design::{
    ship_design_effective_prerequisites, HullRegistry, ModuleRegistry, ShipDesignRegistry,
};

use super::tree::{TechId, TechTree};

/// What category of game element a tech unlocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlockKind {
    Module,
    Building,
    Structure,
    Tech,
    /// A hull unlocked by tech. Populated once HullDefinition gains a
    /// `prerequisites: Option<Condition>` field (#226).
    Hull,
    /// A ship design unlocked by tech (derived from hull + modules, #226).
    ShipDesign,
}

/// A single thing unlocked by a technology.
#[derive(Clone, Debug)]
pub struct UnlockEntry {
    pub kind: UnlockKind,
    pub id: String,
    pub name: String,
}

/// Reverse map from `tech_id` -> things unlocked when researched.
///
/// Built once at startup by `build_tech_unlock_index` after all registries
/// (modules, buildings, structures, technologies) have been loaded.
#[derive(Resource, Default, Debug, Clone)]
pub struct TechUnlockIndex {
    pub unlocks: HashMap<String, Vec<UnlockEntry>>,
}

impl TechUnlockIndex {
    /// Get the list of unlocks for a given tech id, or an empty slice if none.
    pub fn for_tech(&self, tech_id: &str) -> &[UnlockEntry] {
        self.unlocks
            .get(tech_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Total number of unlock entries across all techs.
    pub fn total_entries(&self) -> usize {
        self.unlocks.values().map(|v| v.len()).sum()
    }

    fn push(&mut self, tech_id: String, entry: UnlockEntry) {
        self.unlocks.entry(tech_id).or_default().push(entry);
    }
}

/// Walk a `Condition` tree and collect every `HasTech` atom's tech id.
///
/// Used to determine which techs gate a structure/building when prerequisites
/// are expressed as a `Condition` tree (potentially nested with All/Any/etc).
pub fn extract_tech_ids(cond: &Condition) -> Vec<String> {
    let mut techs = Vec::new();
    collect_tech_ids(cond, &mut techs);
    techs
}

fn collect_tech_ids(cond: &Condition, out: &mut Vec<String>) {
    match cond {
        Condition::Atom(atom) => {
            if let AtomKind::HasTech(id) = &atom.kind {
                out.push(id.clone());
            }
        }
        Condition::All(children)
        | Condition::Any(children)
        | Condition::OneOf(children) => {
            for c in children {
                collect_tech_ids(c, out);
            }
        }
        Condition::Not(inner) => collect_tech_ids(inner, out),
    }
}

/// Build the `TechUnlockIndex` resource by scanning all definition registries.
///
/// Runs once at startup, after every registry has finished its own Lua-load
/// system. Iterates:
///
/// * `ModuleRegistry` — `prerequisites: Option<Condition>`
/// * `BuildingRegistry` — `prerequisites: Option<Condition>`
/// * `HullRegistry` — `prerequisites: Option<Condition>`
/// * `StructureRegistry` — `prerequisites: Option<Condition>`
/// * `TechTree` — each tech's `prerequisites: Vec<TechId>` (reversed)
pub fn build_tech_unlock_index(
    mut index: ResMut<TechUnlockIndex>,
    modules: Res<ModuleRegistry>,
    buildings: Res<BuildingRegistry>,
    structures: Res<StructureRegistry>,
    hulls: Res<HullRegistry>,
    designs: Res<ShipDesignRegistry>,
    tech_trees: Query<&TechTree>,
    tech_tree_res: Option<Res<TechTree>>,
) {
    // Reset in case the system somehow runs again.
    index.unlocks.clear();

    // Modules: walk the optional Condition tree for HasTech atoms.
    for (id, def) in &modules.modules {
        if let Some(cond) = &def.prerequisites {
            for tech_id in extract_tech_ids(cond) {
                index.push(
                    tech_id,
                    UnlockEntry {
                        kind: UnlockKind::Module,
                        id: id.clone(),
                        name: def.name.clone(),
                    },
                );
            }
        }
    }

    // Buildings: walk the optional Condition tree for HasTech atoms.
    for (id, def) in &buildings.buildings {
        if let Some(cond) = &def.prerequisites {
            for tech_id in extract_tech_ids(cond) {
                index.push(
                    tech_id,
                    UnlockEntry {
                        kind: UnlockKind::Building,
                        id: id.clone(),
                        name: def.name.clone(),
                    },
                );
            }
        }
    }

    // Hulls: walk the optional Condition tree for HasTech atoms.
    for (id, def) in &hulls.hulls {
        if let Some(cond) = &def.prerequisites {
            for tech_id in extract_tech_ids(cond) {
                index.push(
                    tech_id,
                    UnlockEntry {
                        kind: UnlockKind::Hull,
                        id: id.clone(),
                        name: def.name.clone(),
                    },
                );
            }
        }
    }

    // Ship designs: derive effective prerequisites from hull + modules, then
    // walk them for HasTech atoms.
    for (id, design) in &designs.designs {
        if let Some(cond) = ship_design_effective_prerequisites(design, &hulls, &modules) {
            for tech_id in extract_tech_ids(&cond) {
                index.push(
                    tech_id,
                    UnlockEntry {
                        kind: UnlockKind::ShipDesign,
                        id: id.clone(),
                        name: design.name.clone(),
                    },
                );
            }
        }
    }

    // Structures: walk the optional Condition tree for HasTech atoms.
    for (id, def) in &structures.definitions {
        if let Some(cond) = &def.prerequisites {
            for tech_id in extract_tech_ids(cond) {
                index.push(
                    tech_id,
                    UnlockEntry {
                        kind: UnlockKind::Structure,
                        id: id.clone(),
                        name: def.name.clone(),
                    },
                );
            }
        }
    }

    // Technologies: each tech's prerequisites point at predecessors -- so the
    // *predecessor* "unlocks" this tech.
    let push_from_tree = |index: &mut TechUnlockIndex, tree: &TechTree| {
        for tech in tree.technologies.values() {
            for prereq in &tech.prerequisites {
                index.push(
                    prereq.0.clone(),
                    UnlockEntry {
                        kind: UnlockKind::Tech,
                        id: tech.id.0.clone(),
                        name: tech.name.clone(),
                    },
                );
            }
        }
    };

    // TechTree lives on the player empire entity (with a resource fallback).
    let mut found_via_query = false;
    for tree in tech_trees.iter() {
        push_from_tree(&mut index, tree);
        found_via_query = true;
    }
    if !found_via_query {
        if let Some(tree) = tech_tree_res.as_deref() {
            push_from_tree(&mut index, tree);
        }
    }

    info!(
        "TechUnlockIndex built: {} techs unlock {} total entries",
        index.unlocks.len(),
        index.total_entries()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::condition::ConditionAtom;
    use crate::deep_space::{ResourceCost, StructureDefinition};
    use crate::scripting::building_api::{BuildingDefinition, CapabilityParams as BCapabilityParams};
    use crate::ship_design::{
        DesignSlotAssignment, HullDefinition, HullRegistry, ModuleDefinition, ModuleRegistry,
        ShipDesignDefinition, ShipDesignRegistry,
    };
    use crate::technology::tree::{TechCost, Technology};
    use std::collections::HashMap;

    fn make_module(id: &str, name: &str, prereq: Option<Condition>) -> ModuleDefinition {
        ModuleDefinition {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: None,
            cost_minerals: Amt::ZERO,
            cost_energy: Amt::ZERO,
            prerequisites: prereq,
            upgrade_to: Vec::new(),
        }
    }

    fn make_structure(id: &str, name: &str, cond: Option<Condition>) -> StructureDefinition {
        StructureDefinition {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            max_hp: 10.0,
            energy_drain: Amt::ZERO,
            prerequisites: cond,
            capabilities: HashMap::new(),
            deliverable: None,
            upgrade_to: Vec::new(),
            upgrade_from: None,
        }
    }

    fn make_tech(id: &str, name: &str, prereqs: Vec<&str>) -> Technology {
        Technology {
            id: TechId(id.to_string()),
            name: name.to_string(),
            description: String::new(),
            branch: "physics".to_string(),
            cost: TechCost::research_only(Amt::units(100)),
            prerequisites: prereqs.into_iter().map(|s| TechId(s.to_string())).collect(),
            dangerous: false,
        }
    }

    fn make_building(id: &str, name: &str, prereq: Option<Condition>) -> BuildingDefinition {
        BuildingDefinition {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 10,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::<String, BCapabilityParams>::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: prereq,
        }
    }

    /// Run `build_tech_unlock_index` against a freshly-built ECS world with
    /// the given registries / TechTree resource.
    fn run_index(
        modules: ModuleRegistry,
        buildings: BuildingRegistry,
        structures: StructureRegistry,
        tree: TechTree,
    ) -> TechUnlockIndex {
        run_index_full(
            modules,
            buildings,
            structures,
            HullRegistry::default(),
            ShipDesignRegistry::default(),
            tree,
        )
    }

    fn run_index_with_hulls(
        modules: ModuleRegistry,
        buildings: BuildingRegistry,
        structures: StructureRegistry,
        hulls: HullRegistry,
        tree: TechTree,
    ) -> TechUnlockIndex {
        run_index_full(
            modules,
            buildings,
            structures,
            hulls,
            ShipDesignRegistry::default(),
            tree,
        )
    }

    fn run_index_full(
        modules: ModuleRegistry,
        buildings: BuildingRegistry,
        structures: StructureRegistry,
        hulls: HullRegistry,
        designs: ShipDesignRegistry,
        tree: TechTree,
    ) -> TechUnlockIndex {
        let mut app = App::new();
        app.init_resource::<TechUnlockIndex>();
        app.insert_resource(modules);
        app.insert_resource(buildings);
        app.insert_resource(structures);
        app.insert_resource(hulls);
        app.insert_resource(designs);
        // Use the resource fallback path -- simpler than spawning an empire.
        app.insert_resource(tree);
        app.add_systems(Update, build_tech_unlock_index);
        app.update();
        app.world_mut()
            .remove_resource::<TechUnlockIndex>()
            .expect("TechUnlockIndex should exist after update")
    }

    fn make_hull(id: &str, name: &str, prereq: Option<Condition>) -> HullDefinition {
        HullDefinition {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            base_hp: 50.0,
            base_speed: 0.5,
            base_evasion: 0.0,
            slots: Vec::new(),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 10,
            maintenance: Amt::ZERO,
            modifiers: Vec::new(),
            prerequisites: prereq,
        }
    }

    #[test]
    fn module_unlocked_by_has_tech() {
        let mut modules = ModuleRegistry::default();
        modules.insert(make_module(
            "laser_mk2",
            "Laser Mk.II",
            Some(Condition::Atom(ConditionAtom::has_tech("laser_weapons"))),
        ));
        modules.insert(make_module("plain", "Plain Module", None));

        let index = run_index(
            modules,
            BuildingRegistry::default(),
            StructureRegistry::default(),
            TechTree::default(),
        );

        let entries = index.for_tech("laser_weapons");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, UnlockKind::Module);
        assert_eq!(entries[0].id, "laser_mk2");
        assert_eq!(entries[0].name, "Laser Mk.II");
        // The module without a prereq should not appear under any tech.
        assert!(!index.unlocks.values().flatten().any(|e| e.id == "plain"));
    }

    #[test]
    fn module_unlocked_by_complex_condition() {
        let mut modules = ModuleRegistry::default();
        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("advanced_weapons")),
            Condition::Any(vec![
                Condition::Atom(ConditionAtom::has_tech("fusion_power")),
                Condition::Atom(ConditionAtom::has_flag("superpowered")),
            ]),
        ]);
        modules.insert(make_module("super_weapon", "Super Weapon", Some(cond)));

        let index = run_index(
            modules,
            BuildingRegistry::default(),
            StructureRegistry::default(),
            TechTree::default(),
        );

        // advanced_weapons and fusion_power each index the module once.
        assert_eq!(index.for_tech("advanced_weapons").len(), 1);
        assert_eq!(index.for_tech("fusion_power").len(), 1);
        assert_eq!(index.for_tech("advanced_weapons")[0].id, "super_weapon");
        assert_eq!(index.for_tech("advanced_weapons")[0].kind, UnlockKind::Module);
    }

    #[test]
    fn structure_unlocked_by_has_tech_atom() {
        let mut structures = StructureRegistry::default();
        let cond = Condition::Atom(ConditionAtom::has_tech("ftl_comms"));
        structures.insert(make_structure("comm_relay", "FTL Comm Relay", Some(cond)));

        let index = run_index(
            ModuleRegistry::default(),
            BuildingRegistry::default(),
            structures,
            TechTree::default(),
        );

        let entries = index.for_tech("ftl_comms");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, UnlockKind::Structure);
        assert_eq!(entries[0].id, "comm_relay");
        assert_eq!(entries[0].name, "FTL Comm Relay");
    }

    #[test]
    fn dependent_tech_unlocked_by_predecessor() {
        let tree = TechTree::from_vec(vec![
            make_tech("basic", "Basic", vec![]),
            make_tech("advanced", "Advanced", vec!["basic"]),
        ]);

        let index = run_index(
            ModuleRegistry::default(),
            BuildingRegistry::default(),
            StructureRegistry::default(),
            tree,
        );

        let entries = index.for_tech("basic");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, UnlockKind::Tech);
        assert_eq!(entries[0].id, "advanced");
        assert_eq!(entries[0].name, "Advanced");

        // The terminal tech should have no unlocks.
        assert!(index.for_tech("advanced").is_empty());
    }

    #[test]
    fn nested_condition_indexes_all_tech_atoms() {
        let mut structures = StructureRegistry::default();
        // Not(All(HasTech("a"), Any(HasTech("b"), HasTech("c")))) — three atoms.
        let cond = Condition::Not(Box::new(Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("a")),
            Condition::Any(vec![
                Condition::Atom(ConditionAtom::has_tech("b")),
                Condition::Atom(ConditionAtom::has_tech("c")),
            ]),
        ])));
        structures.insert(make_structure("complex_thing", "Complex", Some(cond)));

        let index = run_index(
            ModuleRegistry::default(),
            BuildingRegistry::default(),
            structures,
            TechTree::default(),
        );

        for tech in ["a", "b", "c"] {
            let entries = index.for_tech(tech);
            assert_eq!(entries.len(), 1, "tech {tech} should index the structure");
            assert_eq!(entries[0].id, "complex_thing");
            assert_eq!(entries[0].kind, UnlockKind::Structure);
        }
    }

    #[test]
    fn extract_tech_ids_handles_non_tech_atoms() {
        // HasModifier / HasBuilding / HasFlag should be ignored.
        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("t1")),
            Condition::Atom(ConditionAtom::has_modifier("m1")),
            Condition::Atom(ConditionAtom::has_building("b1")),
            Condition::Atom(ConditionAtom::has_flag("f1")),
        ]);
        let ids = extract_tech_ids(&cond);
        assert_eq!(ids, vec!["t1".to_string()]);
    }

    #[test]
    fn building_without_prerequisites_produces_no_entries() {
        let mut buildings = BuildingRegistry::default();
        buildings.insert(make_building("mine", "Mine", None));
        let index = run_index(
            ModuleRegistry::default(),
            buildings,
            StructureRegistry::default(),
            TechTree::default(),
        );
        assert_eq!(index.total_entries(), 0);
    }

    #[test]
    fn building_unlocked_by_has_tech() {
        let mut buildings = BuildingRegistry::default();
        let cond = Condition::Atom(ConditionAtom::has_tech("industrial_automated_mining"));
        buildings.insert(make_building("advanced_mine", "Advanced Mine", Some(cond)));

        let index = run_index(
            ModuleRegistry::default(),
            buildings,
            StructureRegistry::default(),
            TechTree::default(),
        );

        let entries = index.for_tech("industrial_automated_mining");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, UnlockKind::Building);
        assert_eq!(entries[0].id, "advanced_mine");
        assert_eq!(entries[0].name, "Advanced Mine");
    }

    #[test]
    fn building_unlocked_by_complex_condition() {
        let mut buildings = BuildingRegistry::default();
        let cond = Condition::All(vec![
            Condition::Atom(ConditionAtom::has_tech("tech_a")),
            Condition::Any(vec![
                Condition::Atom(ConditionAtom::has_tech("tech_b")),
                Condition::Atom(ConditionAtom::has_flag("some_flag")),
            ]),
        ]);
        buildings.insert(make_building("mega_mine", "Mega Mine", Some(cond)));
        let index = run_index(
            ModuleRegistry::default(),
            buildings,
            StructureRegistry::default(),
            TechTree::default(),
        );
        // Both tech_a and tech_b should index the building; the has_flag atom is ignored.
        assert_eq!(index.for_tech("tech_a").len(), 1);
        assert_eq!(index.for_tech("tech_b").len(), 1);
        assert_eq!(index.for_tech("tech_a")[0].kind, UnlockKind::Building);
        assert_eq!(index.for_tech("tech_a")[0].id, "mega_mine");
    }

    #[test]
    fn empty_registries_produce_empty_index() {
        let index = run_index(
            ModuleRegistry::default(),
            BuildingRegistry::default(),
            StructureRegistry::default(),
            TechTree::default(),
        );
        assert_eq!(index.total_entries(), 0);
        assert!(index.for_tech("anything").is_empty());
    }

    #[test]
    fn ship_design_unlocked_from_derived_prereqs() {
        // A design whose hull requires T1 and whose module requires T2 should
        // appear as a UnlockKind::ShipDesign entry under BOTH techs.
        let mut hulls = HullRegistry::default();
        let mut corvette = make_hull(
            "corvette",
            "Corvette",
            Some(Condition::Atom(ConditionAtom::has_tech("T1"))),
        );
        // Give the hull at least one slot so the design can reference it.
        corvette.slots = vec![crate::ship_design::HullSlot {
            slot_type: "ftl".into(),
            count: 1,
        }];
        hulls.insert(corvette);

        let mut modules = ModuleRegistry::default();
        modules.insert(ModuleDefinition {
            id: "ftl_drive".into(),
            name: "FTL Drive".into(),
            description: String::new(),
            slot_type: "ftl".into(),
            modifiers: Vec::new(),
            weapon: None,
            cost_minerals: Amt::ZERO,
            cost_energy: Amt::ZERO,
            prerequisites: Some(Condition::Atom(ConditionAtom::has_tech("T2"))),
            upgrade_to: Vec::new(),
        });

        let mut designs = ShipDesignRegistry::default();
        designs.insert(ShipDesignDefinition {
            id: "explorer".into(),
            name: "Explorer".into(),
            description: String::new(),
            hull_id: "corvette".into(),
            modules: vec![DesignSlotAssignment {
                slot_type: "ftl".into(),
                module_id: "ftl_drive".into(),
            }],
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::ZERO,
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 1,
            hp: 50.0,
            sublight_speed: 0.5,
            ftl_range: 10.0,
            revision: 0,
        });

        let index = run_index_full(
            modules,
            BuildingRegistry::default(),
            StructureRegistry::default(),
            hulls,
            designs,
            TechTree::default(),
        );

        // Both T1 and T2 should list the design + the hull / module themselves.
        let t1 = index.for_tech("T1");
        assert!(t1.iter().any(|e| e.kind == UnlockKind::ShipDesign && e.id == "explorer"));
        assert!(t1.iter().any(|e| e.kind == UnlockKind::Hull && e.id == "corvette"));

        let t2 = index.for_tech("T2");
        assert!(t2.iter().any(|e| e.kind == UnlockKind::ShipDesign && e.id == "explorer"));
        assert!(t2.iter().any(|e| e.kind == UnlockKind::Module && e.id == "ftl_drive"));
    }

    #[test]
    fn hull_unlocked_by_has_tech() {
        let mut hulls = HullRegistry::default();
        let cond = Condition::Atom(ConditionAtom::has_tech("hull_cruiser"));
        hulls.insert(make_hull("cruiser", "Cruiser", Some(cond)));

        let index = run_index_with_hulls(
            ModuleRegistry::default(),
            BuildingRegistry::default(),
            StructureRegistry::default(),
            hulls,
            TechTree::default(),
        );

        let entries = index.for_tech("hull_cruiser");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, UnlockKind::Hull);
        assert_eq!(entries[0].id, "cruiser");
        assert_eq!(entries[0].name, "Cruiser");
    }
}
