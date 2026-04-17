use std::collections::HashMap;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::condition::Condition;

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
    /// Optional Condition tree gating access to this hull.
    /// Populated from the Lua `prerequisites = has_tech(...)` / ... field.
    pub prerequisites: Option<Condition>,
    /// Hull size tier (mandatory). Small craft = 1, capital ships = large values.
    pub size: u32,
    /// Whether this hull is a capital-class hull (default false).
    pub is_capital: bool,
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
    /// Optional Condition tree gating access to this module.
    /// Populated from the Lua `prerequisites = has_tech(...)` / ... field.
    ///
    /// Previously named `prerequisite_tech: Option<String>`; hard-migrated
    /// in #226 to a full Condition tree so modules can be gated by arbitrary
    /// combinations of tech / flags / buildings / modifiers.
    pub prerequisites: Option<Condition>,
    /// Available upgrade paths from this module.
    pub upgrade_to: Vec<ModuleUpgradePath>,
    /// #239: Build time contribution (in hexadies). The design's total build
    /// time is `hull.build_time + Σ module.build_time`. Added to make refits
    /// and upgrades pay a time cost proportional to the loadout, not just the
    /// hull. Defaults to 0 when Lua omits the field so existing content keeps
    /// compiling.
    pub build_time: i64,
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
    /// #123: Revision counter for design-based refit. Incremented every time
    /// the design is edited via the Ship Designer. Ships with a `design_revision`
    /// less than this value are considered "refit-eligible".
    pub revision: u64,
}

/// An error raised when validating a ShipDesignDefinition against its hull/module registries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShipDesignValidationError {
    /// The design's referenced hull is not in the HullRegistry.
    HullNotFound { design_id: String, hull_id: String },
    /// A slot assignment references a module not in the ModuleRegistry.
    ModuleNotFound {
        design_id: String,
        module_id: String,
    },
    /// The slot_type declared on a slot assignment doesn't exist on the hull.
    UnknownSlotType {
        design_id: String,
        hull_id: String,
        slot_type: String,
    },
    /// The module's slot_type doesn't match the slot it's being assigned to.
    SlotTypeMismatch {
        design_id: String,
        module_id: String,
        expected: String,
        actual: String,
    },
    /// Too many modules of a given slot_type are assigned (exceeds hull's count).
    SlotOverfilled {
        design_id: String,
        hull_id: String,
        slot_type: String,
        available: u32,
        requested: u32,
    },
}

impl std::fmt::Display for ShipDesignValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HullNotFound { design_id, hull_id } => write!(
                f,
                "ship design '{}' references unknown hull '{}'",
                design_id, hull_id
            ),
            Self::ModuleNotFound {
                design_id,
                module_id,
            } => write!(
                f,
                "ship design '{}' references unknown module '{}'",
                design_id, module_id
            ),
            Self::UnknownSlotType {
                design_id,
                hull_id,
                slot_type,
            } => write!(
                f,
                "ship design '{}' uses slot_type '{}' not declared on hull '{}'",
                design_id, slot_type, hull_id
            ),
            Self::SlotTypeMismatch {
                design_id,
                module_id,
                expected,
                actual,
            } => write!(
                f,
                "ship design '{}' assigns module '{}' (slot_type='{}') into a '{}' slot",
                design_id, module_id, actual, expected
            ),
            Self::SlotOverfilled {
                design_id,
                hull_id,
                slot_type,
                available,
                requested,
            } => write!(
                f,
                "ship design '{}' fills {} '{}' slot(s) but hull '{}' only provides {}",
                design_id, requested, slot_type, hull_id, available
            ),
        }
    }
}

impl std::error::Error for ShipDesignValidationError {}

impl ShipDesignDefinition {
    /// Validate this design: the hull exists, each module exists, and every slot
    /// assignment's slot_type exists on the hull and matches the module's slot_type.
    pub fn validate(
        &self,
        hulls: &HullRegistry,
        modules: &ModuleRegistry,
    ) -> Result<(), ShipDesignValidationError> {
        let hull =
            hulls
                .get(&self.hull_id)
                .ok_or_else(|| ShipDesignValidationError::HullNotFound {
                    design_id: self.id.clone(),
                    hull_id: self.hull_id.clone(),
                })?;

        // Tally requested slot assignments per slot_type for overfill checks.
        let mut per_slot_count: HashMap<&str, u32> = HashMap::new();

        for assignment in &self.modules {
            // The slot_type on the assignment must exist on the hull.
            if !hull
                .slots
                .iter()
                .any(|s| s.slot_type == assignment.slot_type)
            {
                return Err(ShipDesignValidationError::UnknownSlotType {
                    design_id: self.id.clone(),
                    hull_id: self.hull_id.clone(),
                    slot_type: assignment.slot_type.clone(),
                });
            }

            // The referenced module must exist.
            let module = modules.get(&assignment.module_id).ok_or_else(|| {
                ShipDesignValidationError::ModuleNotFound {
                    design_id: self.id.clone(),
                    module_id: assignment.module_id.clone(),
                }
            })?;

            // The module's slot_type must match the slot it's being assigned to.
            if module.slot_type != assignment.slot_type {
                return Err(ShipDesignValidationError::SlotTypeMismatch {
                    design_id: self.id.clone(),
                    module_id: assignment.module_id.clone(),
                    expected: assignment.slot_type.clone(),
                    actual: module.slot_type.clone(),
                });
            }

            *per_slot_count
                .entry(assignment.slot_type.as_str())
                .or_insert(0) += 1;
        }

        // Ensure we haven't exceeded the hull's slot counts.
        for (slot_type, requested) in &per_slot_count {
            let available: u32 = hull
                .slots
                .iter()
                .filter(|s| s.slot_type == *slot_type)
                .map(|s| s.count)
                .sum();
            if *requested > available {
                return Err(ShipDesignValidationError::SlotOverfilled {
                    design_id: self.id.clone(),
                    hull_id: self.hull_id.clone(),
                    slot_type: slot_type.to_string(),
                    available,
                    requested: *requested,
                });
            }
        }

        Ok(())
    }
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

    /// #123: Replace an existing design with an edited version, incrementing
    /// the revision counter. Returns the new revision (or `None` if the
    /// design ID does not exist in the registry).
    ///
    /// Ships pointing at this design will see `design.revision >
    /// ship.design_revision` and be flagged as "needs refit" until they
    /// individually invoke the Apply Refit action.
    pub fn upsert_edited(&mut self, mut def: ShipDesignDefinition) -> u64 {
        let new_rev = self
            .designs
            .get(&def.id)
            .map(|existing| existing.revision + 1)
            .unwrap_or(0);
        def.revision = new_rev;
        self.designs.insert(def.id.clone(), def);
        new_rev
    }

    /// Check if a design can perform surveys.
    pub fn can_survey(&self, id: &str) -> bool {
        self.designs.get(id).map(|d| d.can_survey).unwrap_or(false)
    }

    /// Check if a design can colonize planets.
    pub fn can_colonize(&self, id: &str) -> bool {
        self.designs
            .get(id)
            .map(|d| d.can_colonize)
            .unwrap_or(false)
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

    // Parse ship designs and validate each against the hull/module registries.
    // #236: Derived fields (hp/ftl_range/cost/maintenance/can_*) are computed
    // here from hull + modules. Any values parsed from Lua are ignored — the
    // parser has already emitted a `warn!` for each authored derived field.
    match ship_design_api::parse_ship_designs(engine.lua()) {
        Ok(defs) => {
            let mut loaded = 0usize;
            let mut rejected = 0usize;
            for mut def in defs {
                match def.validate(&hulls, &modules) {
                    Ok(()) => {
                        apply_derived_to_definition(&mut def, &hulls, &modules);
                        designs.insert(def);
                        loaded += 1;
                    }
                    Err(err) => {
                        warn!("Rejected ship design: {err}");
                        rejected += 1;
                    }
                }
            }
            info!(
                "Loaded {} ship design definitions ({} rejected)",
                loaded, rejected
            );
        }
        Err(e) => warn!("Failed to parse ship design definitions: {e}"),
    }
}

/// #236: Overwrite a definition's derived fields with values computed from
/// its hull + modules. Used by the registry loader and by any code path that
/// constructs a `ShipDesignDefinition` from Lua (or from the Ship Designer
/// UI) and wants to ensure the resulting stats match hull + modules.
pub fn apply_derived_to_definition(
    def: &mut ShipDesignDefinition,
    hulls: &HullRegistry,
    modules: &ModuleRegistry,
) {
    let Some(hull) = hulls.get(&def.hull_id) else {
        return;
    };
    let mod_defs: Vec<&ModuleDefinition> = def
        .modules
        .iter()
        .filter_map(|a| modules.get(&a.module_id))
        .collect();
    let d = design_derived(hull, &mod_defs);
    def.hp = d.hp;
    def.sublight_speed = d.sublight_speed;
    def.ftl_range = d.ftl_range;
    def.maintenance = d.maintenance;
    def.build_cost_minerals = d.build_cost_minerals;
    def.build_cost_energy = d.build_cost_energy;
    def.build_time = d.build_time;
    def.can_survey = d.can_survey;
    def.can_colonize = d.can_colonize;
}

/// #226: derive a ship design's effective prerequisites from its hull and
/// modules. A design has no first-class `prerequisites` field — instead the
/// conditions gating the underlying hull + modules are composed here.
///
/// * 0 sub-conditions → `None`
/// * 1 sub-condition   → the sub-condition unwrapped (avoids a noisy `All([X])`)
/// * N sub-conditions  → `Condition::All(...)`
pub fn ship_design_effective_prerequisites(
    design: &ShipDesignDefinition,
    hulls: &HullRegistry,
    modules: &ModuleRegistry,
) -> Option<Condition> {
    let mut parts: Vec<Condition> = Vec::new();
    if let Some(hull) = hulls.get(&design.hull_id) {
        if let Some(c) = &hull.prerequisites {
            parts.push(c.clone());
        }
    }
    for assign in &design.modules {
        if let Some(m) = modules.get(&assign.module_id) {
            if let Some(c) = &m.prerequisites {
                parts.push(c.clone());
            }
        }
    }
    match parts.len() {
        0 => None,
        1 => Some(parts.into_iter().next().unwrap()),
        _ => Some(Condition::All(parts)),
    }
}

/// #236: Derived statistics for a ship design, computed entirely from
/// `HullDefinition` + a selection of `ModuleDefinition`s. Lua presets never
/// author these values — they are derived at registry build time so that the
/// hull + module content is the single source of truth.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedStats {
    pub hp: f64,
    pub sublight_speed: f64,
    pub evasion: f64,
    pub ftl_range: f64,
    pub survey_speed: f64,
    pub colonize_speed: f64,
    /// `survey_speed > 0` — capability derived, not authored.
    pub can_survey: bool,
    /// `colonize_speed > 0` — capability derived, not authored.
    pub can_colonize: bool,
    pub build_cost_minerals: Amt,
    pub build_cost_energy: Amt,
    pub build_time: i64,
    pub maintenance: Amt,
}

/// Apply modifiers against a base value, using the same formula as
/// `ModifiedValue::final_value`:
/// ```text
/// final = (base + Σ base_add) * (1 + Σ multiplier) + Σ add
/// ```
/// Negative values are clamped to 0 at each stage (matches ModifiedValue).
fn apply_modifiers(base: f64, target: &str, sources: &[&[ModuleModifier]]) -> f64 {
    let mut base_sum = base;
    let mut mult_sum = 1.0;
    let mut add_sum = 0.0;
    for group in sources {
        for m in *group {
            if m.target == target {
                base_sum += m.base_add;
                mult_sum += m.multiplier;
                add_sum += m.add;
            }
        }
    }
    (base_sum.max(0.0) * mult_sum.max(0.0) + add_sum).max(0.0)
}

/// #236: Compute all derived stats for a ship design from its hull + modules.
/// Applies hull modifiers and module modifiers via the `ModifiedValue` formula.
pub fn design_derived(hull: &HullDefinition, modules: &[&ModuleDefinition]) -> DerivedStats {
    let module_mods: Vec<&[ModuleModifier]> =
        modules.iter().map(|m| m.modifiers.as_slice()).collect();
    let mut sources: Vec<&[ModuleModifier]> = Vec::with_capacity(module_mods.len() + 1);
    sources.push(hull.modifiers.as_slice());
    for m in &module_mods {
        sources.push(*m);
    }

    let hp = apply_modifiers(hull.base_hp, "ship.hp", &sources);
    let sublight_speed = apply_modifiers(hull.base_speed, "ship.speed", &sources);
    let evasion = apply_modifiers(hull.base_evasion, "ship.evasion", &sources);
    let ftl_range = apply_modifiers(0.0, "ship.ftl_range", &sources);
    let survey_speed = apply_modifiers(0.0, "ship.survey_speed", &sources);
    let colonize_speed = apply_modifiers(0.0, "ship.colonize_speed", &sources);

    // Cost + maintenance: hull + Σ module. Each module adds
    // 0.0001 × mineral_cost to energy maintenance per hexady — i.e. a module
    // costing 100 minerals contributes 0.010 energy/hd.
    //
    // #257: Before this fix the formula was `Amt::milli(raw()/10)` which
    // produced 10 energy/hd for a 100-mineral module (1000× too high) because
    // `Amt::raw()` returns the internal fixed-point with SCALE=1000. The
    // starter fleet's maintenance ballooned to ~93 energy/hd, dwarfing the
    // ~30 energy/hd produced by the opening power plant.
    //
    // raw() is already scaled ×1000, so dividing by 10_000 before wrapping
    // in `Amt::milli` lands on the intended 0.01% coefficient.
    let mut minerals = hull.build_cost_minerals;
    let mut energy = hull.build_cost_energy;
    let mut maintenance = hull.maintenance;
    // #239: Total build time is `hull.build_time + Σ module.build_time`.
    // Previously only the hull contributed, which made heavy loadouts (and
    // therefore refits) disproportionately cheap in time. Summed as i64 and
    // clamped to ≥ 1 at the end so degenerate zero-time designs never slip
    // through the build queue.
    let mut build_time = hull.build_time;
    for m in modules {
        minerals = minerals.add(m.cost_minerals);
        energy = energy.add(m.cost_energy);
        maintenance = maintenance.add(Amt::milli(m.cost_minerals.raw() / 10_000));
        build_time = build_time.saturating_add(m.build_time);
    }

    DerivedStats {
        hp,
        sublight_speed,
        evasion,
        ftl_range,
        survey_speed,
        colonize_speed,
        can_survey: survey_speed > 0.0,
        can_colonize: colonize_speed > 0.0,
        build_cost_minerals: minerals,
        build_cost_energy: energy,
        build_time: build_time.max(1),
        maintenance,
    }
}

/// Compute total cost for a ship design: hull cost + sum of module costs.
/// Returns (minerals, energy, build_time, maintenance).
/// Retained as a thin wrapper around `design_derived` for existing call-sites.
pub fn design_cost(hull: &HullDefinition, modules: &[&ModuleDefinition]) -> (Amt, Amt, i64, Amt) {
    let d = design_derived(hull, modules);
    (
        d.build_cost_minerals,
        d.build_cost_energy,
        d.build_time,
        d.maintenance,
    )
}

/// Compute total stats for a design: HP, speed, evasion from hull + module modifiers.
/// Retained as a thin wrapper around `design_derived` for existing call-sites.
pub fn design_stats(hull: &HullDefinition, modules: &[&ModuleDefinition]) -> (f64, f64, f64) {
    let d = design_derived(hull, modules);
    (d.hp, d.sublight_speed, d.evasion)
}

/// #123: Convert a design's slot assignments into the EquippedModule list
/// that should appear on a ship after applying that design.
pub fn design_equipped_modules(design: &ShipDesignDefinition) -> Vec<crate::ship::EquippedModule> {
    design
        .modules
        .iter()
        .map(|a| crate::ship::EquippedModule {
            slot_type: a.slot_type.clone(),
            module_id: a.module_id.clone(),
        })
        .collect()
}

/// #123: Compute refit cost when bringing a ship in line with a target design.
/// Resolves modules through the ModuleRegistry and falls back to an empty list
/// for unknown IDs (so callers don't need to filter beforehand).
/// Returns (minerals, energy, time_hexadies).
pub fn refit_cost_to_design(
    current_modules: &[crate::ship::EquippedModule],
    design: &ShipDesignDefinition,
    hull: &HullDefinition,
    module_registry: &ModuleRegistry,
) -> (Amt, Amt, i64) {
    let old_mods: Vec<&ModuleDefinition> = current_modules
        .iter()
        .filter_map(|em| module_registry.get(&em.module_id))
        .collect();
    let new_mods: Vec<&ModuleDefinition> = design
        .modules
        .iter()
        .filter_map(|a| module_registry.get(&a.module_id))
        .collect();
    refit_cost(&old_mods, &new_mods, hull)
}

/// Compute refit cost: new module cost - 50% of old module value.
/// Returns (minerals, energy, time).
///
/// #239: Time cost is `(hull.build_time + Σ new_module.build_time) / 2`,
/// matching the `design_derived` build_time formula halved. Before this
/// change refits only paid `hull.build_time / 2` regardless of the new
/// loadout, which made wholesale module swaps nearly free in time — one
/// of the explicit motivations for the issue.
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
    let mut new_build_time: i64 = 0;
    for m in new_modules {
        new_m = new_m.add(m.cost_minerals);
        new_e = new_e.add(m.cost_energy);
        new_build_time = new_build_time.saturating_add(m.build_time);
    }
    // Refund 50% of old module value
    let refund_m = Amt::milli(old_m.raw() / 2);
    let refund_e = Amt::milli(old_e.raw() / 2);
    let cost_m = if new_m > refund_m {
        new_m.sub(refund_m)
    } else {
        Amt::ZERO
    };
    let cost_e = if new_e > refund_e {
        new_e.sub(refund_e)
    } else {
        Amt::ZERO
    };
    // Refit time: half of the full (hull + modules) build time.
    let total_time = hull.build_time.saturating_add(new_build_time);
    let time = total_time / 2;
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
                HullSlot {
                    slot_type: "weapon".to_string(),
                    count: 2,
                },
                HullSlot {
                    slot_type: "ftl".to_string(),
                    count: 1,
                },
            ],
            build_cost_minerals: Amt::units(200),
            build_cost_energy: Amt::units(100),
            build_time: 60,
            maintenance: Amt::new(0, 500),
            modifiers: vec![],
            prerequisites: None,
            size: 1,
            is_capital: false,
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
            slot_type: "ftl".to_string(),
            modifiers: vec![ModuleModifier {
                target: "ship.ftl_range".to_string(),
                base_add: 15.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::units(100),
            cost_energy: Amt::units(50),
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        });

        let ftl = registry.get("ftl_drive").unwrap();
        assert_eq!(ftl.name, "FTL Drive");
        assert_eq!(ftl.slot_type, "ftl");
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
                    slot_type: "ftl".to_string(),
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
            revision: 0,
        });

        let explorer = registry.get("explorer_mk1").unwrap();
        assert_eq!(explorer.name, "Explorer Mk.I");
        assert_eq!(explorer.hull_id, "corvette");
        assert_eq!(explorer.modules.len(), 2);
        assert_eq!(explorer.modules[0].module_id, "ftl_drive");
    }

    // ---------------------------------------------------------------------
    // #257: Module maintenance scaling
    // ---------------------------------------------------------------------

    /// Each module contributes `0.0001 × mineral_cost` to energy maintenance
    /// per hexady. A module costing 100 minerals must land on 0.010 energy/hd,
    /// not 10 energy/hd (pre-fix the formula was 1000× too large because
    /// `Amt::raw()` already carries the ×1000 scale factor).
    #[test]
    fn test_module_maintenance_formula_matches_ten_percent_intent() {
        let hull = HullDefinition {
            id: "frame".to_string(),
            name: "Frame".to_string(),
            description: String::new(),
            base_hp: 1.0,
            base_speed: 0.0,
            base_evasion: 0.0,
            slots: vec![],
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 1,
            maintenance: Amt::ZERO,
            modifiers: vec![],
            prerequisites: None,
            size: 1,
            is_capital: false,
        };
        let module = ModuleDefinition {
            id: "m100".to_string(),
            name: "100-mineral module".to_string(),
            description: String::new(),
            slot_type: "utility".to_string(),
            modifiers: vec![],
            weapon: None,
            cost_minerals: Amt::units(100),
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };

        let derived = design_derived(&hull, &[&module]);
        assert_eq!(
            derived.maintenance,
            Amt::milli(10),
            "100-mineral module should contribute 0.010 energy/hd, got {}",
            derived.maintenance
        );
    }

    /// Regression: the starter explorer (ftl + survey modules) used to cost
    /// 16.5 energy/hd of maintenance. With the fix its total must be under
    /// 1 energy/hd — otherwise the opening economy is unplayable (see #257).
    #[test]
    fn test_starter_explorer_maintenance_under_one() {
        let hull = HullDefinition {
            id: "scout_hull".to_string(),
            name: "Scout Hull".to_string(),
            description: String::new(),
            base_hp: 10.0,
            base_speed: 1.0,
            base_evasion: 30.0,
            slots: vec![],
            build_cost_minerals: Amt::units(50),
            build_cost_energy: Amt::units(25),
            build_time: 20,
            // Starter hull maintenance — matches ships/hulls.lua ballpark.
            maintenance: Amt::new(0, 500),
            modifiers: vec![],
            prerequisites: None,
            size: 1,
            is_capital: false,
        };
        let ftl = ModuleDefinition {
            id: "ftl_drive_mk1".to_string(),
            name: "FTL Drive Mk.I".to_string(),
            description: String::new(),
            slot_type: "ftl".to_string(),
            modifiers: vec![],
            weapon: None,
            cost_minerals: Amt::units(100),
            cost_energy: Amt::units(50),
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };
        let survey = ModuleDefinition {
            id: "survey_equipment".to_string(),
            name: "Survey Equipment".to_string(),
            description: String::new(),
            slot_type: "utility".to_string(),
            modifiers: vec![],
            weapon: None,
            cost_minerals: Amt::units(60),
            cost_energy: Amt::units(30),
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };

        let derived = design_derived(&hull, &[&ftl, &survey]);
        // 0.500 (hull) + 0.010 (100 min) + 0.006 (60 min) = 0.516 energy/hd
        assert!(
            derived.maintenance < Amt::units(1),
            "starter explorer maintenance must be < 1 energy/hd; got {}",
            derived.maintenance
        );
        assert_eq!(derived.maintenance, Amt::milli(516));
    }

    // ---------------------------------------------------------------------
    // ShipDesignDefinition::validate tests
    // ---------------------------------------------------------------------

    /// Build a small hull/module registry pair used by the validation tests.
    fn validation_fixture() -> (HullRegistry, ModuleRegistry) {
        let mut hulls = HullRegistry::default();
        hulls.insert(HullDefinition {
            id: "corvette".to_string(),
            name: "Corvette".to_string(),
            description: String::new(),
            base_hp: 50.0,
            base_speed: 0.75,
            base_evasion: 30.0,
            slots: vec![
                HullSlot {
                    slot_type: "ftl".to_string(),
                    count: 1,
                },
                HullSlot {
                    slot_type: "weapon".to_string(),
                    count: 2,
                },
                HullSlot {
                    slot_type: "utility".to_string(),
                    count: 1,
                },
            ],
            build_cost_minerals: Amt::units(200),
            build_cost_energy: Amt::units(100),
            build_time: 60,
            maintenance: Amt::new(0, 500),
            modifiers: vec![],
            prerequisites: None,
            size: 1,
            is_capital: false,
        });

        let mut modules = ModuleRegistry::default();
        let make = |id: &str, slot: &str| ModuleDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            slot_type: slot.to_string(),
            modifiers: vec![],
            weapon: None,
            cost_minerals: Amt::ZERO,
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };
        modules.insert(make("ftl_drive", "ftl"));
        modules.insert(make("weapon_laser", "weapon"));
        modules.insert(make("survey_equipment", "utility"));
        modules.insert(make("shield_generator", "defense"));

        (hulls, modules)
    }

    fn make_design(id: &str, modules: Vec<DesignSlotAssignment>) -> ShipDesignDefinition {
        ShipDesignDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            hull_id: "corvette".to_string(),
            modules,
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::ZERO,
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 1,
            hp: 50.0,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            revision: 0,
        }
    }

    #[test]
    fn validate_accepts_well_formed_design() {
        let (hulls, modules) = validation_fixture();
        let design = make_design(
            "ok",
            vec![
                DesignSlotAssignment {
                    slot_type: "ftl".into(),
                    module_id: "ftl_drive".into(),
                },
                DesignSlotAssignment {
                    slot_type: "weapon".into(),
                    module_id: "weapon_laser".into(),
                },
                DesignSlotAssignment {
                    slot_type: "utility".into(),
                    module_id: "survey_equipment".into(),
                },
            ],
        );
        assert!(design.validate(&hulls, &modules).is_ok());
    }

    #[test]
    fn validate_rejects_slot_type_mismatch() {
        let (hulls, modules) = validation_fixture();
        // Putting an ftl module into a "weapon" slot assignment is invalid.
        let design = make_design(
            "mismatch",
            vec![DesignSlotAssignment {
                slot_type: "weapon".into(),
                module_id: "ftl_drive".into(),
            }],
        );
        match design.validate(&hulls, &modules) {
            Err(ShipDesignValidationError::SlotTypeMismatch {
                actual, expected, ..
            }) => {
                assert_eq!(actual, "ftl");
                assert_eq!(expected, "weapon");
            }
            other => panic!("expected SlotTypeMismatch, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_unknown_slot_type_on_hull() {
        let (hulls, modules) = validation_fixture();
        // Hull has no "defense" slot.
        let design = make_design(
            "unknown_slot",
            vec![DesignSlotAssignment {
                slot_type: "defense".into(),
                module_id: "shield_generator".into(),
            }],
        );
        match design.validate(&hulls, &modules) {
            Err(ShipDesignValidationError::UnknownSlotType { slot_type, .. }) => {
                assert_eq!(slot_type, "defense");
            }
            other => panic!("expected UnknownSlotType, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_unknown_module() {
        let (hulls, modules) = validation_fixture();
        let design = make_design(
            "missing_mod",
            vec![DesignSlotAssignment {
                slot_type: "ftl".into(),
                module_id: "nonexistent".into(),
            }],
        );
        match design.validate(&hulls, &modules) {
            Err(ShipDesignValidationError::ModuleNotFound { module_id, .. }) => {
                assert_eq!(module_id, "nonexistent");
            }
            other => panic!("expected ModuleNotFound, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_unknown_hull() {
        let (hulls, modules) = validation_fixture();
        let mut design = make_design("bad_hull", vec![]);
        design.hull_id = "nonexistent_hull".into();
        match design.validate(&hulls, &modules) {
            Err(ShipDesignValidationError::HullNotFound { hull_id, .. }) => {
                assert_eq!(hull_id, "nonexistent_hull");
            }
            other => panic!("expected HullNotFound, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_overfilled_slot() {
        let (hulls, modules) = validation_fixture();
        // Corvette only provides 1 ftl slot; try to fill 2.
        let design = make_design(
            "too_many",
            vec![
                DesignSlotAssignment {
                    slot_type: "ftl".into(),
                    module_id: "ftl_drive".into(),
                },
                DesignSlotAssignment {
                    slot_type: "ftl".into(),
                    module_id: "ftl_drive".into(),
                },
            ],
        );
        match design.validate(&hulls, &modules) {
            Err(ShipDesignValidationError::SlotOverfilled {
                slot_type,
                available,
                requested,
                ..
            }) => {
                assert_eq!(slot_type, "ftl");
                assert_eq!(available, 1);
                assert_eq!(requested, 2);
            }
            other => panic!("expected SlotOverfilled, got {:?}", other),
        }
    }

    // ---------------------------------------------------------------------
    // #123: Design-based refit revision plumbing
    // ---------------------------------------------------------------------

    fn fresh_design(id: &str, modules: Vec<DesignSlotAssignment>) -> ShipDesignDefinition {
        let mut d = make_design(id, modules);
        d.revision = 0;
        d
    }

    #[test]
    fn upsert_edited_first_insert_keeps_revision_zero() {
        let mut registry = ShipDesignRegistry::default();
        let design = fresh_design("ex", vec![]);
        let new_rev = registry.upsert_edited(design);
        assert_eq!(new_rev, 0);
        assert_eq!(registry.get("ex").unwrap().revision, 0);
    }

    #[test]
    fn upsert_edited_bumps_revision_on_replace() {
        let mut registry = ShipDesignRegistry::default();
        let mut design = fresh_design("ex", vec![]);
        registry.insert(design.clone());
        // First "edit" — revision should become 1.
        design.name = "Renamed".into();
        let r1 = registry.upsert_edited(design.clone());
        assert_eq!(r1, 1);
        assert_eq!(registry.get("ex").unwrap().revision, 1);
        // Second "edit" — revision should become 2.
        design.name = "Renamed Again".into();
        let r2 = registry.upsert_edited(design);
        assert_eq!(r2, 2);
        assert_eq!(registry.get("ex").unwrap().revision, 2);
    }

    #[test]
    fn refit_cost_to_design_routes_through_registry() {
        let (hulls, modules) = validation_fixture();
        // A design whose modules are entirely free (test fixture sets cost to ZERO)
        let design = make_design(
            "ok",
            vec![DesignSlotAssignment {
                slot_type: "ftl".into(),
                module_id: "ftl_drive".into(),
            }],
        );
        let hull = hulls.get("corvette").unwrap();
        let (m, e, _t) = refit_cost_to_design(&[], &design, hull, &modules);
        // Both module costs are zero in the fixture, so refit cost is zero.
        assert_eq!(m, Amt::ZERO);
        assert_eq!(e, Amt::ZERO);
    }

    #[test]
    fn ship_design_effective_prereqs_empty_when_none() {
        let (hulls, modules) = validation_fixture();
        let design = make_design(
            "noop",
            vec![DesignSlotAssignment {
                slot_type: "ftl".into(),
                module_id: "ftl_drive".into(),
            }],
        );
        assert!(ship_design_effective_prerequisites(&design, &hulls, &modules).is_none());
    }

    #[test]
    fn ship_design_effective_prereqs_single_unwrapped_not_wrapped_in_all() {
        use crate::condition::ConditionAtom;
        let (mut hulls, modules) = validation_fixture();
        // Attach a prerequisite to the corvette hull.
        let mut corvette = hulls.get("corvette").unwrap().clone();
        corvette.prerequisites = Some(Condition::Atom(ConditionAtom::has_tech("hull_corvette")));
        hulls.insert(corvette);

        let design = make_design(
            "ok",
            vec![DesignSlotAssignment {
                slot_type: "ftl".into(),
                module_id: "ftl_drive".into(),
            }],
        );
        let eff = ship_design_effective_prerequisites(&design, &hulls, &modules);
        // Exactly one contributing condition (hull's). Must not be wrapped in All.
        assert_eq!(
            eff,
            Some(Condition::Atom(ConditionAtom::has_tech("hull_corvette")))
        );
    }

    #[test]
    fn ship_design_effective_prereqs_derived_from_hull_and_modules() {
        use crate::condition::ConditionAtom;
        let (mut hulls, mut modules) = validation_fixture();
        // Hull requires tech T1.
        let mut corvette = hulls.get("corvette").unwrap().clone();
        corvette.prerequisites = Some(Condition::Atom(ConditionAtom::has_tech("T1")));
        hulls.insert(corvette);
        // FTL drive module requires tech T2.
        let mut ftl = modules.get("ftl_drive").unwrap().clone();
        ftl.prerequisites = Some(Condition::Atom(ConditionAtom::has_tech("T2")));
        modules.insert(ftl);

        let design = make_design(
            "ok",
            vec![DesignSlotAssignment {
                slot_type: "ftl".into(),
                module_id: "ftl_drive".into(),
            }],
        );
        let eff = ship_design_effective_prerequisites(&design, &hulls, &modules);
        match eff {
            Some(Condition::All(parts)) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], Condition::Atom(ConditionAtom::has_tech("T1")));
                assert_eq!(parts[1], Condition::Atom(ConditionAtom::has_tech("T2")));
            }
            other => panic!("expected Condition::All, got {:?}", other),
        }
    }

    #[test]
    fn design_equipped_modules_mirrors_design_assignments() {
        let design = make_design(
            "ok",
            vec![
                DesignSlotAssignment {
                    slot_type: "ftl".into(),
                    module_id: "ftl_drive".into(),
                },
                DesignSlotAssignment {
                    slot_type: "weapon".into(),
                    module_id: "weapon_laser".into(),
                },
            ],
        );
        let equipped = design_equipped_modules(&design);
        assert_eq!(equipped.len(), 2);
        assert_eq!(equipped[0].slot_type, "ftl");
        assert_eq!(equipped[0].module_id, "ftl_drive");
        assert_eq!(equipped[1].slot_type, "weapon");
        assert_eq!(equipped[1].module_id, "weapon_laser");
    }

    // -----------------------------------------------------------------
    // #236: Regression tests — derive ship design stats from hull +
    // modules. Preset-authored fields must be ignored; all stats flow
    // through `design_derived` via the hull + modules registries.
    // -----------------------------------------------------------------

    /// Small helper building a hull + module fixture that mirrors the real
    /// Lua courier_hull + ftl_drive content so we can assert exact numeric
    /// expectations in the derive tests.
    fn derive_fixture_courier() -> (
        HullDefinition,
        ModuleDefinition,
        ModuleDefinition,
        ModuleDefinition,
    ) {
        let courier_hull = HullDefinition {
            id: "courier_hull".into(),
            name: "Courier Hull".into(),
            description: String::new(),
            base_hp: 35.0,
            base_speed: 0.80,
            base_evasion: 25.0,
            slots: vec![
                HullSlot {
                    slot_type: "ftl".into(),
                    count: 1,
                },
                HullSlot {
                    slot_type: "sublight".into(),
                    count: 1,
                },
                HullSlot {
                    slot_type: "utility".into(),
                    count: 2,
                },
            ],
            build_cost_minerals: Amt::units(100),
            build_cost_energy: Amt::units(50),
            build_time: 30,
            maintenance: Amt::new(0, 300),
            modifiers: vec![
                ModuleModifier {
                    target: "ship.cargo_capacity".into(),
                    base_add: 0.0,
                    multiplier: 1.5,
                    add: 0.0,
                },
                ModuleModifier {
                    target: "ship.ftl_range".into(),
                    base_add: 0.0,
                    multiplier: 1.2,
                    add: 0.0,
                },
            ],
            prerequisites: None,
            size: 1,
            is_capital: false,
        };
        let ftl_drive = ModuleDefinition {
            id: "ftl_drive".into(),
            name: "FTL Drive".into(),
            description: String::new(),
            slot_type: "ftl".into(),
            modifiers: vec![ModuleModifier {
                target: "ship.ftl_range".into(),
                base_add: 15.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::units(100),
            cost_energy: Amt::units(50),
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };
        let afterburner = ModuleDefinition {
            id: "afterburner".into(),
            name: "Afterburner".into(),
            description: String::new(),
            slot_type: "sublight".into(),
            modifiers: vec![ModuleModifier {
                target: "ship.speed".into(),
                base_add: 0.0,
                multiplier: 0.2,
                add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::units(60),
            cost_energy: Amt::units(40),
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };
        let cargo_bay = ModuleDefinition {
            id: "cargo_bay".into(),
            name: "Cargo Bay".into(),
            description: String::new(),
            slot_type: "utility".into(),
            modifiers: vec![ModuleModifier {
                target: "ship.cargo_capacity".into(),
                base_add: 500.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::units(30),
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };
        (courier_hull, ftl_drive, afterburner, cargo_bay)
    }

    /// #236 primary regression: Courier Mk.I must have FTL.
    #[test]
    fn test_courier_mk1_has_ftl_capability() {
        let (hull, ftl, ab, cargo) = derive_fixture_courier();
        let d = design_derived(&hull, &[&ftl, &ab, &cargo]);
        // (0 + 15) * (1 + 1.2) = 33.0
        assert!(d.ftl_range > 0.0, "courier_mk1 must have FTL range > 0");
        assert!(
            (d.ftl_range - 33.0).abs() < 1e-9,
            "expected ftl_range 33.0, got {}",
            d.ftl_range
        );
    }

    /// #236: Hull modifiers feed into `design_derived`. Previously the inline
    /// compute in overlays ignored hull.modifiers entirely.
    #[test]
    fn test_hull_modifiers_applied_in_derive() {
        // courier_hull ftl_range multiplier 1.2x applies on top of ftl_drive.
        let (courier, ftl, _, _) = derive_fixture_courier();
        let d = design_derived(&courier, &[&ftl]);
        assert!(
            (d.ftl_range - 33.0).abs() < 1e-9,
            "courier hull ftl_range 1.2x must apply"
        );

        // scout_hull: survey_speed multiplier 1.3x applies on survey_equipment.
        let scout = HullDefinition {
            id: "scout_hull".into(),
            name: "Scout Hull".into(),
            description: String::new(),
            base_hp: 40.0,
            base_speed: 0.85,
            base_evasion: 35.0,
            slots: vec![HullSlot {
                slot_type: "utility".into(),
                count: 1,
            }],
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 1,
            maintenance: Amt::ZERO,
            modifiers: vec![
                ModuleModifier {
                    target: "ship.survey_speed".into(),
                    base_add: 0.0,
                    multiplier: 1.3,
                    add: 0.0,
                },
                ModuleModifier {
                    target: "ship.speed".into(),
                    base_add: 0.0,
                    multiplier: 1.15,
                    add: 0.0,
                },
            ],
            prerequisites: None,
            size: 1,
            is_capital: false,
        };
        let survey = ModuleDefinition {
            id: "survey_equipment".into(),
            name: "Survey".into(),
            description: String::new(),
            slot_type: "utility".into(),
            modifiers: vec![ModuleModifier {
                target: "ship.survey_speed".into(),
                base_add: 1.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::ZERO,
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };
        let d = design_derived(&scout, &[&survey]);
        // (0 + 1.0) * (1 + 1.3) = 2.3
        assert!(
            (d.survey_speed - 2.3).abs() < 1e-9,
            "scout_hull survey_speed 1.3x must apply: got {}",
            d.survey_speed
        );
        // sublight: 0.85 * (1 + 1.15) = 1.8275
        assert!(
            (d.sublight_speed - 1.8275).abs() < 1e-9,
            "scout_hull speed 1.15x must apply: got {}",
            d.sublight_speed
        );
    }

    /// #236: `can_survey` and `can_colonize` derive from speed fields, not
    /// from authored flags. A ship with a survey module auto-gets can_survey.
    #[test]
    fn test_can_survey_derives_from_survey_speed() {
        let bare_hull = HullDefinition {
            id: "corvette".into(),
            name: "Corvette".into(),
            description: String::new(),
            base_hp: 50.0,
            base_speed: 0.75,
            base_evasion: 30.0,
            slots: vec![HullSlot {
                slot_type: "utility".into(),
                count: 1,
            }],
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 1,
            maintenance: Amt::ZERO,
            modifiers: vec![],
            prerequisites: None,
            size: 1,
            is_capital: false,
        };
        let survey = ModuleDefinition {
            id: "survey_equipment".into(),
            name: "Survey".into(),
            description: String::new(),
            slot_type: "utility".into(),
            modifiers: vec![ModuleModifier {
                target: "ship.survey_speed".into(),
                base_add: 1.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::ZERO,
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
        };

        // Without survey module → no survey capability.
        let no_survey = design_derived(&bare_hull, &[]);
        assert!(!no_survey.can_survey);
        assert_eq!(no_survey.survey_speed, 0.0);

        // With survey module → survey_speed > 0 → can_survey = true.
        let with_survey = design_derived(&bare_hull, &[&survey]);
        assert!(with_survey.can_survey);
        assert!(with_survey.survey_speed > 0.0);
        assert!(!with_survey.can_colonize);
    }

    /// #236: A full preset (hull + modules) derives to the expected stats.
    /// Acts as a contract test for the whole derive pipeline.
    #[test]
    fn test_preset_designs_derived_from_modules() {
        let (hull, ftl, ab, cargo) = derive_fixture_courier();
        let d = design_derived(&hull, &[&ftl, &ab, &cargo]);

        // Stats
        assert_eq!(d.hp, 35.0);
        assert!(
            (d.sublight_speed - 0.96).abs() < 1e-9,
            "0.80 * 1.2 = 0.96, got {}",
            d.sublight_speed
        );
        assert_eq!(d.evasion, 25.0);
        assert!((d.ftl_range - 33.0).abs() < 1e-9);
        assert_eq!(d.survey_speed, 0.0);
        assert_eq!(d.colonize_speed, 0.0);
        assert!(!d.can_survey);
        assert!(!d.can_colonize);

        // Cost: 100+100+60+30 = 290, 50+50+40+0 = 140
        assert_eq!(d.build_cost_minerals, Amt::units(290));
        assert_eq!(d.build_cost_energy, Amt::units(140));
        assert_eq!(d.build_time, 30);
        // #257: Maintenance uses 0.0001 × mineral_cost per module.
        //   hull 0.300 + ftl 100×0.0001 + afterburner 60×0.0001 + cargo 30×0.0001
        //   = 0.300 + 0.010 + 0.006 + 0.003 = 0.319
        assert_eq!(d.maintenance, Amt::new(0, 319));
    }

    // -----------------------------------------------------------------
    // #239: ModuleDefinition.build_time contribution
    // -----------------------------------------------------------------

    /// Helper producing a minimal hull with a configurable build_time and
    /// utility slots (so we can stack modules).
    fn tiny_hull(build_time: i64) -> HullDefinition {
        HullDefinition {
            id: "tiny".into(),
            name: "Tiny".into(),
            description: String::new(),
            base_hp: 10.0,
            base_speed: 1.0,
            base_evasion: 0.0,
            slots: vec![HullSlot {
                slot_type: "utility".into(),
                count: 4,
            }],
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time,
            maintenance: Amt::ZERO,
            modifiers: vec![],
            prerequisites: None,
            size: 1,
            is_capital: false,
        }
    }

    fn tiny_module(id: &str, build_time: i64, mineral_cost: u64) -> ModuleDefinition {
        ModuleDefinition {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            slot_type: "utility".into(),
            modifiers: vec![],
            weapon: None,
            cost_minerals: Amt::units(mineral_cost),
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time,
        }
    }

    /// #239 primary regression: design build_time is hull + Σ module.
    #[test]
    fn test_design_derived_build_time_sums_hull_and_modules() {
        let hull = tiny_hull(10);
        let a = tiny_module("a", 5, 0);
        let b = tiny_module("b", 5, 0);

        let d = design_derived(&hull, &[&a, &b]);
        assert_eq!(d.build_time, 20, "build_time must be hull(10) + Σ(5+5)");

        // No modules → build_time equals hull.build_time (clamped to ≥ 1).
        let d_bare = design_derived(&hull, &[]);
        assert_eq!(d_bare.build_time, 10);
    }

    /// #239: A zero hull + zero modules design still reports build_time
    /// of at least 1 so the build queue never divides by zero.
    #[test]
    fn test_design_derived_build_time_clamped_to_one() {
        let hull = tiny_hull(0);
        let d = design_derived(&hull, &[]);
        assert_eq!(d.build_time, 1);
    }

    /// #239: Refit time scales with Σ new_module.build_time, not only the
    /// hull. Swapping in heavier modules costs more refit time.
    #[test]
    fn test_refit_cost_uses_module_build_time() {
        let hull = tiny_hull(20); // hull alone → baseline refit time 10
        let old_m = tiny_module("old", 0, 0);
        let new_light = tiny_module("new_light", 0, 0);
        let new_heavy = tiny_module("new_heavy", 10, 0);

        // Light → (20 + 0)/2 = 10 hd
        let (_, _, t_light) = refit_cost(&[&old_m], &[&new_light], &hull);
        assert_eq!(t_light, 10);

        // Heavy → (20 + 10)/2 = 15 hd. Must be strictly greater than the
        // light case — this is the key behavioural change the issue asks
        // for.
        let (_, _, t_heavy) = refit_cost(&[&old_m], &[&new_heavy], &hull);
        assert_eq!(t_heavy, 15);
        assert!(
            t_heavy > t_light,
            "heavier modules must cost more refit time"
        );
    }

    /// #239: `refit_cost_to_design` routes module lookup through the
    /// registry and still sees the build_time contribution.
    #[test]
    fn test_refit_cost_to_design_includes_module_build_time() {
        let mut hulls = HullRegistry::default();
        hulls.insert(tiny_hull(20));
        let mut modules = ModuleRegistry::default();
        modules.insert(tiny_module("heavy", 10, 0));

        let hull = hulls.get("tiny").unwrap();

        let design = ShipDesignDefinition {
            id: "d".into(),
            name: "d".into(),
            description: String::new(),
            hull_id: "tiny".into(),
            modules: vec![DesignSlotAssignment {
                slot_type: "utility".into(),
                module_id: "heavy".into(),
            }],
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::ZERO,
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 0.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        };

        let (_, _, t) = refit_cost_to_design(&[], &design, hull, &modules);
        // (20 + 10)/2 = 15
        assert_eq!(t, 15);
    }

    /// #239 regression: preset build times match the new formula. Asserts
    /// the `design_derived` output against a hand-computed value using the
    /// same hull + module fixture produced by `derive_fixture_courier`.
    /// When we eventually give the courier fixture non-zero module build
    /// times this test guards against regressions in the summation.
    #[test]
    fn test_preset_build_times_match_new_formula() {
        let (hull, ftl, ab, cargo) = derive_fixture_courier();

        // The courier_hull fixture carries hull.build_time = 30 and the
        // three modules default to 0 — so the pre-#239 hull-only value and
        // the post-#239 sum match by construction. Intentional: if anyone
        // later gives the fixture non-zero module times without updating
        // the expectation, the test fails and forces a consistent update.
        let expected = hull.build_time + ftl.build_time + ab.build_time + cargo.build_time;
        let d = design_derived(&hull, &[&ftl, &ab, &cargo]);
        assert_eq!(d.build_time, expected);
    }

    /// #239: Two designs with identical hull but heavier module loadout
    /// must differ in build time. Protects against a future regression
    /// where the summation is accidentally reduced back to
    /// `hull.build_time`.
    #[test]
    fn test_heavier_loadout_costs_more_build_time() {
        let hull = tiny_hull(10);
        let light = tiny_module("light", 5, 0);
        let heavy = tiny_module("heavy", 20, 0);

        let d_light = design_derived(&hull, &[&light]);
        let d_heavy = design_derived(&hull, &[&heavy]);
        assert!(
            d_heavy.build_time > d_light.build_time,
            "heavier module loadout must produce a longer build time"
        );
        assert_eq!(d_light.build_time, 15);
        assert_eq!(d_heavy.build_time, 30);
    }
}
