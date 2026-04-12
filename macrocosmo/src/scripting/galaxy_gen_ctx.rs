//! Lua UserData context types exposed to the three galaxy-generation hooks
//! (#181):
//!
//! - `on_galaxy_generate_empty(ctx)` — Phase A. The callback populates a list
//!   of empty star systems (position + star type).
//! - `on_choose_capitals(ctx)` — Phase B. The callback picks which of the
//!   already-generated systems should become capitals, for which factions.
//! - `on_initialize_system(ctx, system)` — Phase C. Called once per system
//!   produced by Phase A; the callback can replace the default planet
//!   generation by spawning planets / overriding the system attributes.
//!
//! Each ctx is a thin, record-only UserData. The Lua side pushes intent into
//! an `Arc<Mutex<...>>` accumulator; the Rust side (galaxy::generation)
//! consumes those records after the callback returns.

use std::sync::{Arc, Mutex};

use super::helpers::extract_id_from_lua_value;
use super::map_api::{PredefinedPlanetSpec, PredefinedSystemRegistry};

/// A `[f64; 3]` position recorded by `ctx:spawn_empty_system`.
pub type PositionF64 = [f64; 3];

/// Planets that should be spawned for a system produced by this callback.
/// Populated only by `ctx:spawn_predefined_system` — plain
/// `ctx:spawn_empty_system` leaves it empty (default planet generation runs
/// in Phase C).
#[derive(Clone, Debug, Default)]
pub struct PredefinedPlanetsForSpawn {
    pub planets: Vec<PredefinedPlanetSpec>,
}

/// A record produced by Lua `ctx:spawn_empty_system(name, position, star_type)`
/// (Phase A), or by `ctx:spawn_predefined_system(id)` (which expands to the
/// same record plus a carried planet list + capital hint).
#[derive(Clone, Debug)]
pub struct SpawnedEmptySystemSpec {
    pub name: String,
    pub position: PositionF64,
    pub star_type: String,
    /// Set when this spawn came from `spawn_predefined_system`; the generation
    /// pipeline uses this in Phase C to skip the default planet roll.
    pub planets: PredefinedPlanetsForSpawn,
    /// Set when this spawn came from `spawn_predefined_system` and the
    /// predefined definition had `capital_for_faction = "..."`. Used as a
    /// hint by the Phase B fallback / Lua `ctx:assign_predefined_capitals`.
    pub capital_for_faction: Option<String>,
}

/// Immutable snapshot of a galaxy-generation parameter set, exposed to Lua as
/// a read-only table via `ctx.settings`.
#[derive(Clone, Debug)]
pub struct GenerationSettings {
    pub num_systems: usize,
    pub num_arms: usize,
    pub galaxy_radius: f64,
    pub arm_twist: f64,
    pub arm_spread: f64,
    pub min_distance: f64,
    pub max_neighbor_distance: f64,
    /// #199: Baseline FTL range used by Lua-side connectivity loops to decide
    /// which system pairs are FTL-adjacent during galaxy generation. Independent
    /// of per-ship/tech values — this is the reference threshold for the
    /// generator only.
    pub initial_ftl_range: f64,
}

/// Actions recorded by a `on_galaxy_generate_empty` callback.
#[derive(Default, Debug, Clone)]
pub struct GalaxyGenerateActions {
    pub spawned_systems: Vec<SpawnedEmptySystemSpec>,
}

/// UserData handed to `on_galaxy_generate_empty(ctx)`.
///
/// Lua API:
/// - `ctx.settings` — table with numeric galaxy params (read-only snapshot).
/// - `ctx:spawn_empty_system(name, {x, y, z}, star_type)` — record a new
///   empty system. `star_type` accepts a string id or a `define_star_type`
///   reference.
#[derive(Clone)]
pub struct GalaxyGenerateCtx {
    pub settings: GenerationSettings,
    pub actions: Arc<Mutex<GalaxyGenerateActions>>,
    /// Optional snapshot of the predefined-system registry, used by
    /// `spawn_predefined_system`. Captured at ctx creation so Phase A can
    /// run without having to thread the Bevy World into Lua.
    pub predefined: Option<Arc<PredefinedSystemRegistry>>,
}

impl GalaxyGenerateCtx {
    pub fn new(settings: GenerationSettings) -> Self {
        Self {
            settings,
            actions: Arc::new(Mutex::new(GalaxyGenerateActions::default())),
            predefined: None,
        }
    }

    pub fn with_predefined(mut self, registry: Arc<PredefinedSystemRegistry>) -> Self {
        self.predefined = Some(registry);
        self
    }

    pub fn take_actions(&self) -> GalaxyGenerateActions {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

fn parse_position(table: &mlua::Table) -> Result<PositionF64, mlua::Error> {
    // Accept either array form {x, y, z} or named { x=..., y=..., z=... }.
    if let Ok(x) = table.get::<f64>(1) {
        let y: f64 = table.get(2)?;
        let z: f64 = table.get::<Option<f64>>(3)?.unwrap_or(0.0);
        return Ok([x, y, z]);
    }
    let x: f64 = table.get("x")?;
    let y: f64 = table.get("y")?;
    let z: f64 = table.get::<Option<f64>>("z")?.unwrap_or(0.0);
    Ok([x, y, z])
}

impl mlua::UserData for GalaxyGenerateCtx {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("settings", |lua, this| {
            let t = lua.create_table()?;
            t.set("num_systems", this.settings.num_systems as i64)?;
            t.set("num_arms", this.settings.num_arms as i64)?;
            t.set("galaxy_radius", this.settings.galaxy_radius)?;
            t.set("arm_twist", this.settings.arm_twist)?;
            t.set("arm_spread", this.settings.arm_spread)?;
            t.set("min_distance", this.settings.min_distance)?;
            t.set("max_neighbor_distance", this.settings.max_neighbor_distance)?;
            t.set("initial_ftl_range", this.settings.initial_ftl_range)?;
            Ok(t)
        });

        // Read-only sequence of systems recorded so far (same shape as
        // Phase B `ctx.systems`). Useful for after-Phase-A connectivity
        // loops to inspect what Phase A produced.
        fields.add_field_method_get("systems", |lua, this| {
            let actions = this.actions.lock().unwrap();
            let arr = lua.create_table()?;
            for (i, sys) in actions.spawned_systems.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("name", sys.name.as_str())?;
                entry.set("star_type", sys.star_type.as_str())?;
                let pos = lua.create_table()?;
                pos.set(1, sys.position[0])?;
                pos.set(2, sys.position[1])?;
                pos.set(3, sys.position[2])?;
                pos.set("x", sys.position[0])?;
                pos.set("y", sys.position[1])?;
                pos.set("z", sys.position[2])?;
                entry.set("position", pos)?;
                entry.set("index", (i + 1) as i64)?;
                arr.set(i + 1, entry)?;
            }
            Ok(arr)
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "spawn_empty_system",
            |_,
             this,
             (name, position, star_type): (String, mlua::Table, mlua::Value)| {
                let pos = parse_position(&position)?;
                let star_type_id = extract_id_from_lua_value(&star_type)?;
                let mut actions = this.actions.lock().unwrap();
                actions.spawned_systems.push(SpawnedEmptySystemSpec {
                    name,
                    position: pos,
                    star_type: star_type_id,
                    planets: PredefinedPlanetsForSpawn::default(),
                    capital_for_faction: None,
                });
                Ok(())
            },
        );

        // ctx:spawn_predefined_system(id_or_ref [, { position = {x,y,z} }])
        //
        // Expands a `define_predefined_system` block to a full spawn:
        //   - name, star_type, capital_for_faction come from the definition
        //   - position defaults to the definition's, can be overridden by the
        //     optional 2nd-arg table (useful when a generator wants to place
        //     the predefined system at a scripted location)
        //   - planets are carried verbatim into Phase C
        //
        // Errors if no PredefinedSystemRegistry was attached or the id is
        // unknown — generators are expected to opt into predefined systems
        // intentionally, not silently.
        methods.add_method(
            "spawn_predefined_system",
            |_,
             this,
             (id_value, overrides): (mlua::Value, Option<mlua::Table>)| {
                let id = extract_id_from_lua_value(&id_value)?;
                let registry = this.predefined.as_ref().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "spawn_predefined_system called but no PredefinedSystemRegistry is available"
                            .into(),
                    )
                })?;
                let def = registry.systems.get(&id).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "spawn_predefined_system: unknown predefined system '{}'",
                        id
                    ))
                })?;

                // Position: override if supplied, else the definition's.
                let position = if let Some(ref t) = overrides {
                    match t.get::<mlua::Value>("position")? {
                        mlua::Value::Table(pt) => parse_position(&pt)?,
                        _ => def.position,
                    }
                } else {
                    def.position
                };
                let name = overrides
                    .as_ref()
                    .and_then(|t| t.get::<Option<String>>("name").ok().flatten())
                    .unwrap_or_else(|| def.name.clone());

                let mut actions = this.actions.lock().unwrap();
                actions.spawned_systems.push(SpawnedEmptySystemSpec {
                    name,
                    position,
                    star_type: def.star_type_id.clone(),
                    planets: PredefinedPlanetsForSpawn {
                        planets: def.planets.clone(),
                    },
                    capital_for_faction: def.capital_for_faction.clone(),
                });
                Ok(())
            },
        );

        // ctx:insert_bridge_at(position [, star_type])
        //
        // #199: Append a bridge system at an intermediate position. Used by
        // Lua-side connectivity loops to fill FTL-reachability gaps. Default
        // star_type is "yellow_dwarf"; callers can pass any string id or a
        // `define_star_type` reference. The system gets an auto-generated
        // `Bridge-NNN` name.
        methods.add_method(
            "insert_bridge_at",
            |_,
             this,
             (position, star_type): (mlua::Table, Option<mlua::Value>)| {
                let pos = parse_position(&position)?;
                let star_type_id = match star_type {
                    Some(v) => extract_id_from_lua_value(&v)?,
                    None => "yellow_dwarf".to_string(),
                };
                let mut actions = this.actions.lock().unwrap();
                let idx = actions.spawned_systems.len();
                actions.spawned_systems.push(SpawnedEmptySystemSpec {
                    name: format!("Bridge-{:03}", idx),
                    position: pos,
                    star_type: star_type_id,
                    planets: PredefinedPlanetsForSpawn::default(),
                    capital_for_faction: None,
                });
                Ok(())
            },
        );

        // ctx:pick_provisional_capital()
        //
        // #199: Return the system closest to origin (0,0,0), used as a proxy
        // for the future Phase B capital while the real capital is not yet
        // selected. Returns nil if no systems have been spawned.
        methods.add_method("pick_provisional_capital", |lua, this, ()| {
            let actions = this.actions.lock().unwrap();
            let mut best: Option<(usize, f64)> = None;
            for (i, sys) in actions.spawned_systems.iter().enumerate() {
                let d2 = sys.position[0] * sys.position[0]
                    + sys.position[1] * sys.position[1]
                    + sys.position[2] * sys.position[2];
                match best {
                    Some((_, bd)) if bd <= d2 => {}
                    _ => best = Some((i, d2)),
                }
            }
            let Some((idx, _)) = best else {
                return Ok(mlua::Value::Nil);
            };
            let sys = &actions.spawned_systems[idx];
            let entry = lua.create_table()?;
            entry.set("name", sys.name.as_str())?;
            entry.set("star_type", sys.star_type.as_str())?;
            let pos = lua.create_table()?;
            pos.set(1, sys.position[0])?;
            pos.set(2, sys.position[1])?;
            pos.set(3, sys.position[2])?;
            pos.set("x", sys.position[0])?;
            pos.set("y", sys.position[1])?;
            pos.set("z", sys.position[2])?;
            entry.set("position", pos)?;
            entry.set("index", (idx + 1) as i64)?;
            Ok(mlua::Value::Table(entry))
        });

        // ctx:build_ftl_graph(ftl_range)
        //
        // #199: Compute an FtlGraph snapshot of currently-spawned systems.
        // Edges connect systems within `ftl_range` of each other. Returned
        // UserData offers queries used by connectivity loops
        // (`unreachable_from`, `connected_components`,
        // `closest_cross_cluster_pair`).
        methods.add_method("build_ftl_graph", |_, this, ftl_range: f64| {
            let actions = this.actions.lock().unwrap();
            let nodes: Vec<FtlNode> = actions
                .spawned_systems
                .iter()
                .enumerate()
                .map(|(i, sys)| FtlNode {
                    index: i + 1,
                    name: sys.name.clone(),
                    star_type: sys.star_type.clone(),
                    position: sys.position,
                })
                .collect();
            Ok(FtlGraph::build(nodes, ftl_range))
        });
    }
}

// --- #199: FtlGraph UserData -------------------------------------------

/// A node in an FtlGraph snapshot.
#[derive(Clone, Debug)]
pub struct FtlNode {
    /// 1-based index matching Phase A `ctx.systems` ordering.
    pub index: usize,
    pub name: String,
    pub star_type: String,
    pub position: PositionF64,
}

/// A snapshot of the FTL-reachability graph for a set of systems, under a
/// given FTL range. Edges are undirected: a pair `(i, j)` is connected iff
/// `distance(i, j) <= ftl_range`.
///
/// Union-Find based: `component[i]` is a representative index. The actual
/// component membership queries recompute sets on demand.
#[derive(Clone, Debug)]
pub struct FtlGraph {
    pub nodes: Vec<FtlNode>,
    pub ftl_range: f64,
    /// Union-Find parent array, sized like nodes.
    parent: Vec<usize>,
}

impl FtlGraph {
    pub fn build(nodes: Vec<FtlNode>, ftl_range: f64) -> Self {
        let n = nodes.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        fn union(parent: &mut [usize], a: usize, b: usize) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra != rb {
                parent[ra] = rb;
            }
        }
        let r2 = ftl_range * ftl_range;
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = nodes[i].position[0] - nodes[j].position[0];
                let dy = nodes[i].position[1] - nodes[j].position[1];
                let dz = nodes[i].position[2] - nodes[j].position[2];
                let d2 = dx * dx + dy * dy + dz * dz;
                if d2 <= r2 {
                    union(&mut parent, i, j);
                }
            }
        }
        Self {
            nodes,
            ftl_range,
            parent,
        }
    }

    /// Resolve the Union-Find root of a node index (0-based).
    fn root(&self, mut x: usize) -> usize {
        while self.parent[x] != x {
            x = self.parent[x];
        }
        x
    }

    /// Return the Union-Find roots, one per node (0-based).
    fn roots(&self) -> Vec<usize> {
        (0..self.nodes.len()).map(|i| self.root(i)).collect()
    }
}

fn resolve_system_index(
    value: &mlua::Value,
    node_count: usize,
) -> Result<usize, mlua::Error> {
    let idx = match value {
        mlua::Value::Integer(i) => *i as usize,
        mlua::Value::Number(f) => *f as usize,
        mlua::Value::Table(t) => {
            let i: i64 = t.get("index")?;
            i as usize
        }
        _ => {
            return Err(mlua::Error::RuntimeError(
                "FtlGraph query: expected a system index or a system table with `index`".into(),
            ));
        }
    };
    if idx == 0 || idx > node_count {
        return Err(mlua::Error::RuntimeError(format!(
            "FtlGraph query: system index {} out of range (1..={})",
            idx, node_count
        )));
    }
    Ok(idx - 1) // convert to 0-based
}

fn node_to_lua_table<'lua>(
    lua: &'lua mlua::Lua,
    node: &FtlNode,
) -> Result<mlua::Table, mlua::Error> {
    let entry = lua.create_table()?;
    entry.set("index", node.index as i64)?;
    entry.set("name", node.name.as_str())?;
    entry.set("star_type", node.star_type.as_str())?;
    let pos = lua.create_table()?;
    pos.set(1, node.position[0])?;
    pos.set(2, node.position[1])?;
    pos.set(3, node.position[2])?;
    pos.set("x", node.position[0])?;
    pos.set("y", node.position[1])?;
    pos.set("z", node.position[2])?;
    entry.set("position", pos)?;
    Ok(entry)
}

impl mlua::UserData for FtlGraph {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("ftl_range", |_, this| Ok(this.ftl_range));
        fields.add_field_method_get("size", |_, this| Ok(this.nodes.len() as i64));
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // graph:unreachable_from(system) → list of system entries not in the
        // same FTL component as `system`.
        methods.add_method("unreachable_from", |lua, this, from: mlua::Value| {
            let arr = lua.create_table()?;
            if this.nodes.is_empty() {
                return Ok(arr);
            }
            let origin = resolve_system_index(&from, this.nodes.len())?;
            let origin_root = this.root(origin);
            let mut written = 0_i64;
            for i in 0..this.nodes.len() {
                if this.root(i) != origin_root {
                    written += 1;
                    arr.set(written, node_to_lua_table(lua, &this.nodes[i])?)?;
                }
            }
            Ok(arr)
        });

        // graph:connected_components() → list of components. Each component
        // is itself a list of system entries.
        methods.add_method("connected_components", |lua, this, ()| {
            let outer = lua.create_table()?;
            if this.nodes.is_empty() {
                return Ok(outer);
            }
            let roots = this.roots();
            // Group nodes by root. Use a stable ordering: first-seen root.
            let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
            for (i, r) in roots.iter().enumerate() {
                if let Some((_, members)) =
                    groups.iter_mut().find(|(root, _)| root == r)
                {
                    members.push(i);
                } else {
                    groups.push((*r, vec![i]));
                }
            }
            for (gi, (_, members)) in groups.iter().enumerate() {
                let inner = lua.create_table()?;
                for (mi, &node_idx) in members.iter().enumerate() {
                    inner.set(mi as i64 + 1, node_to_lua_table(lua, &this.nodes[node_idx])?)?;
                }
                outer.set(gi as i64 + 1, inner)?;
            }
            Ok(outer)
        });

        // graph:closest_cross_cluster_pair(from_system)
        //     → system_a, system_b (or nil, nil)
        //
        // Returns the closest pair of systems (a, b) where `a` is in the
        // same component as `from_system` and `b` is in a different
        // component. Used by connectivity loops to decide where to insert
        // a bridge.
        methods.add_method(
            "closest_cross_cluster_pair",
            |lua, this, from: mlua::Value| {
                if this.nodes.is_empty() {
                    return Ok((mlua::Value::Nil, mlua::Value::Nil));
                }
                let origin = resolve_system_index(&from, this.nodes.len())?;
                let origin_root = this.root(origin);
                let mut best: Option<(f64, usize, usize)> = None;
                for i in 0..this.nodes.len() {
                    if this.root(i) != origin_root {
                        continue;
                    }
                    for j in 0..this.nodes.len() {
                        if this.root(j) == origin_root {
                            continue;
                        }
                        let dx = this.nodes[i].position[0] - this.nodes[j].position[0];
                        let dy = this.nodes[i].position[1] - this.nodes[j].position[1];
                        let dz = this.nodes[i].position[2] - this.nodes[j].position[2];
                        let d2 = dx * dx + dy * dy + dz * dz;
                        match best {
                            Some((bd2, _, _)) if bd2 <= d2 => {}
                            _ => best = Some((d2, i, j)),
                        }
                    }
                }
                match best {
                    Some((_, a, b)) => Ok((
                        mlua::Value::Table(node_to_lua_table(lua, &this.nodes[a])?),
                        mlua::Value::Table(node_to_lua_table(lua, &this.nodes[b])?),
                    )),
                    None => Ok((mlua::Value::Nil, mlua::Value::Nil)),
                }
            },
        );
    }
}

// --- Phase B: choose capitals ------------------------------------------

/// A capital assignment record produced by Lua `ctx:assign_capital(sys_idx, faction)`.
#[derive(Clone, Debug)]
pub struct CapitalAssignmentSpec {
    /// 1-based index into the `systems` list provided to the callback.
    pub system_index: usize,
    pub faction_id: String,
}

/// Actions recorded by a `on_choose_capitals` callback.
#[derive(Default, Debug, Clone)]
pub struct ChooseCapitalsActions {
    pub assignments: Vec<CapitalAssignmentSpec>,
}

/// Read-only snapshot of a system that Phase B hooks can inspect.
#[derive(Clone, Debug)]
pub struct SystemSnapshot {
    pub name: String,
    pub position: PositionF64,
    pub star_type: String,
    /// If this system was spawned via `spawn_predefined_system` AND the
    /// predefined definition carried `capital_for_faction = "..."`, this
    /// field forwards that hint into Phase B. Consumed by
    /// `ctx:assign_predefined_capitals()` and by the default Phase B fallback.
    pub capital_for_faction: Option<String>,
}

/// UserData handed to `on_choose_capitals(ctx)`.
///
/// Lua API:
/// - `ctx.factions` — sequence of faction id strings.
/// - `ctx.systems` — sequence of `{name=..., position={x,y,z}, star_type=...}`.
/// - `ctx:assign_capital(system_index, faction)` — record a capital.
#[derive(Clone)]
pub struct ChooseCapitalsCtx {
    pub systems: Vec<SystemSnapshot>,
    pub factions: Vec<String>,
    pub actions: Arc<Mutex<ChooseCapitalsActions>>,
}

impl ChooseCapitalsCtx {
    pub fn new(systems: Vec<SystemSnapshot>, factions: Vec<String>) -> Self {
        Self {
            systems,
            factions,
            actions: Arc::new(Mutex::new(ChooseCapitalsActions::default())),
        }
    }

    pub fn take_actions(&self) -> ChooseCapitalsActions {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl mlua::UserData for ChooseCapitalsCtx {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("factions", |lua, this| {
            let t = lua.create_table()?;
            for (i, id) in this.factions.iter().enumerate() {
                t.set(i + 1, id.as_str())?;
            }
            Ok(t)
        });

        fields.add_field_method_get("systems", |lua, this| {
            let arr = lua.create_table()?;
            for (i, sys) in this.systems.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("name", sys.name.as_str())?;
                entry.set("star_type", sys.star_type.as_str())?;
                let pos = lua.create_table()?;
                pos.set(1, sys.position[0])?;
                pos.set(2, sys.position[1])?;
                pos.set(3, sys.position[2])?;
                pos.set("x", sys.position[0])?;
                pos.set("y", sys.position[1])?;
                pos.set("z", sys.position[2])?;
                entry.set("position", pos)?;
                entry.set("index", (i + 1) as i64)?;
                if let Some(faction) = &sys.capital_for_faction {
                    entry.set("capital_for_faction", faction.as_str())?;
                }
                arr.set(i + 1, entry)?;
            }
            Ok(arr)
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // ctx:assign_predefined_capitals() — walks every system in the Phase B
        // snapshot list; whenever one has a `capital_for_faction` hint (set by
        // `spawn_predefined_system`), records a matching assignment. Returns
        // the number of capitals assigned.
        //
        // Common usage in `on_choose_capitals`:
        //     ctx:assign_predefined_capitals()
        //     -- then do fallback logic for factions that had no predefined capital
        methods.add_method("assign_predefined_capitals", |_, this, ()| {
            let mut actions = this.actions.lock().unwrap();
            let mut count = 0_i64;
            for (i, sys) in this.systems.iter().enumerate() {
                if let Some(faction) = &sys.capital_for_faction {
                    actions.assignments.push(CapitalAssignmentSpec {
                        system_index: i + 1,
                        faction_id: faction.clone(),
                    });
                    count += 1;
                }
            }
            Ok(count)
        });

        // assign_capital(system_index_or_table, faction)
        // Accepts either:
        //   ctx:assign_capital(3, "humanity_empire")
        //   ctx:assign_capital(ctx.systems[3], "humanity_empire")
        // Also tolerates faction as a reference table (if factions ever get _def_type).
        methods.add_method(
            "assign_capital",
            |_,
             this,
             (sys_ref, faction): (mlua::Value, mlua::Value)| {
                let system_index = match sys_ref {
                    mlua::Value::Integer(i) => i as usize,
                    mlua::Value::Number(f) => f as usize,
                    mlua::Value::Table(t) => {
                        // Accept { index = N } — as returned by ctx.systems entries.
                        let idx: i64 = t.get("index")?;
                        idx as usize
                    }
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "assign_capital: first arg must be a system index or a system table"
                                .into(),
                        ));
                    }
                };
                let faction_id = extract_id_from_lua_value(&faction)?;
                let mut actions = this.actions.lock().unwrap();
                actions.assignments.push(CapitalAssignmentSpec {
                    system_index,
                    faction_id,
                });
                Ok(())
            },
        );
    }
}

// --- Phase C: initialize a single system -------------------------------

/// Attribute overrides for a spawned planet. Mirrors `game_start_ctx`.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct PlanetAttrsOverride {
    pub habitability: Option<f64>,
    pub mineral_richness: Option<f64>,
    pub energy_potential: Option<f64>,
    pub research_potential: Option<f64>,
    pub max_building_slots: Option<u8>,
}

fn parse_planet_attrs(table: &mlua::Table) -> Result<PlanetAttrsOverride, mlua::Error> {
    let mut spec = PlanetAttrsOverride::default();
    if let Ok(v) = table.get::<f64>("habitability") {
        spec.habitability = Some(v);
    }
    if let Ok(v) = table.get::<f64>("mineral_richness") {
        spec.mineral_richness = Some(v);
    }
    if let Ok(v) = table.get::<f64>("energy_potential") {
        spec.energy_potential = Some(v);
    }
    if let Ok(v) = table.get::<f64>("research_potential") {
        spec.research_potential = Some(v);
    }
    if let Ok(v) = table.get::<u32>("max_building_slots") {
        spec.max_building_slots = Some(v.min(u8::MAX as u32) as u8);
    }
    Ok(spec)
}

/// A planet record produced by `system_ctx:spawn_planet`.
#[derive(Clone, Debug)]
pub struct InitializeSpawnedPlanet {
    pub name: String,
    pub planet_type: String,
    pub attrs: PlanetAttrsOverride,
}

/// Actions recorded by a single `on_initialize_system` callback call.
#[derive(Default, Debug, Clone)]
pub struct InitializeSystemActions {
    /// If true, the default planet-generation step is skipped entirely — only
    /// the planets spawned by the callback are created for this system.
    ///
    /// This is implicitly `true` whenever the callback spawns at least one
    /// planet. The field is exposed so that a callback that only wants to
    /// override system attributes (without planets) can opt out of the
    /// default planets explicitly.
    pub override_default_planets: bool,
    pub spawned_planets: Vec<InitializeSpawnedPlanet>,
    /// Optional override for the system's surveyed flag.
    pub surveyed: Option<bool>,
    /// Optional override for the system name.
    pub name: Option<String>,
}

/// UserData handed to `on_initialize_system(ctx, system)`.
///
/// Lua API (on `ctx`):
/// - `ctx.index` — 1-based index of the system within the generation list.
/// - `ctx.name`, `ctx.star_type`, `ctx.position` — read-only info for the system.
/// - `ctx.is_capital` — whether the system has been marked a capital in Phase B.
/// - `ctx:spawn_planet(name, type, attrs?)` — record a planet to spawn.
///   The first call implicitly disables the default planet generation.
/// - `ctx:set_attributes({ name=..., surveyed=... })` — override system-level
///   attributes.
#[derive(Clone)]
pub struct InitializeSystemCtx {
    pub index: usize,
    pub name: String,
    pub star_type: String,
    pub position: PositionF64,
    pub is_capital: bool,
    pub actions: Arc<Mutex<InitializeSystemActions>>,
}

impl InitializeSystemCtx {
    pub fn new(
        index: usize,
        name: String,
        star_type: String,
        position: PositionF64,
        is_capital: bool,
    ) -> Self {
        Self {
            index,
            name,
            star_type,
            position,
            is_capital,
            actions: Arc::new(Mutex::new(InitializeSystemActions::default())),
        }
    }

    pub fn take_actions(&self) -> InitializeSystemActions {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl mlua::UserData for InitializeSystemCtx {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("index", |_, this| Ok(this.index as i64));
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("star_type", |_, this| Ok(this.star_type.clone()));
        fields.add_field_method_get("is_capital", |_, this| Ok(this.is_capital));
        fields.add_field_method_get("position", |lua, this| {
            let t = lua.create_table()?;
            t.set(1, this.position[0])?;
            t.set(2, this.position[1])?;
            t.set(3, this.position[2])?;
            t.set("x", this.position[0])?;
            t.set("y", this.position[1])?;
            t.set("z", this.position[2])?;
            Ok(t)
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "spawn_planet",
            |_,
             this,
             (name, planet_type, attrs): (String, mlua::Value, Option<mlua::Table>)| {
                let type_id = extract_id_from_lua_value(&planet_type)?;
                let attributes = match attrs {
                    Some(t) => parse_planet_attrs(&t)?,
                    None => PlanetAttrsOverride::default(),
                };
                let mut actions = this.actions.lock().unwrap();
                actions.override_default_planets = true;
                actions.spawned_planets.push(InitializeSpawnedPlanet {
                    name,
                    planet_type: type_id,
                    attrs: attributes,
                });
                Ok(())
            },
        );

        methods.add_method(
            "override_default_planets",
            |_, this, value: Option<bool>| {
                let mut actions = this.actions.lock().unwrap();
                actions.override_default_planets = value.unwrap_or(true);
                Ok(())
            },
        );

        methods.add_method("set_attributes", |_, this, table: mlua::Table| {
            let mut actions = this.actions.lock().unwrap();
            if let Ok(name) = table.get::<String>("name") {
                actions.name = Some(name);
            }
            if let Ok(surveyed) = table.get::<bool>("surveyed") {
                actions.surveyed = Some(surveyed);
            }
            Ok(())
        });
    }
}

// --- Hook-lookup helpers ------------------------------------------------

/// Names of the Lua global tables that store hook functions for each phase.
pub const GENERATE_EMPTY_HANDLERS: &str = "_on_galaxy_generate_empty_handlers";
pub const CHOOSE_CAPITALS_HANDLERS: &str = "_on_choose_capitals_handlers";
pub const INITIALIZE_SYSTEM_HANDLERS: &str = "_on_initialize_system_handlers";
/// #199: Hook that runs after Phase A completes, regardless of whether Phase A
/// was driven by an active `map_type` generator, the `on_galaxy_generate_empty`
/// hook, or the built-in Rust spiral. Receives the same `GalaxyGenerateCtx`
/// used by Phase A (with `ctx.systems` / `build_ftl_graph` / `insert_bridge_at`
/// / `pick_provisional_capital` available) so Lua can enforce connectivity
/// guarantees before Phase B selects capitals.
pub const AFTER_PHASE_A_HANDLERS: &str = "_on_after_phase_a_handlers";

/// Return the last registered hook function from the given handlers table, if any.
/// "Last wins" matches the semantics expected of a single-replacement hook.
pub fn last_registered_hook(
    lua: &mlua::Lua,
    table_name: &str,
) -> Result<Option<mlua::Function>, mlua::Error> {
    let Ok(handlers) = lua.globals().get::<mlua::Table>(table_name) else {
        return Ok(None);
    };
    let len = handlers.len()?;
    if len == 0 {
        return Ok(None);
    }
    let func: mlua::Function = handlers.get(len)?;
    Ok(Some(func))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    fn test_settings() -> GenerationSettings {
        GenerationSettings {
            num_systems: 100,
            num_arms: 3,
            galaxy_radius: 80.0,
            arm_twist: 2.5,
            arm_spread: 0.4,
            min_distance: 2.0,
            max_neighbor_distance: 8.0,
            initial_ftl_range: 10.0,
        }
    }

    #[test]
    fn test_generate_ctx_spawn_empty_system() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx:spawn_empty_system("Alpha", {1.0, 2.0, 3.0}, "yellow_dwarf")
            ctx:spawn_empty_system("Beta", { x = 4.0, y = 5.0, z = 6.0 }, "red_dwarf")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.spawned_systems.len(), 2);
        assert_eq!(actions.spawned_systems[0].name, "Alpha");
        assert_eq!(actions.spawned_systems[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(actions.spawned_systems[0].star_type, "yellow_dwarf");
        assert_eq!(actions.spawned_systems[1].name, "Beta");
        assert_eq!(actions.spawned_systems[1].position, [4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_generate_ctx_settings_exposed() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let radius: f64 = lua.load("return ctx.settings.galaxy_radius").eval().unwrap();
        assert!((radius - 80.0).abs() < 1e-10);
        let num: i64 = lua.load("return ctx.settings.num_systems").eval().unwrap();
        assert_eq!(num, 100);
    }

    #[test]
    fn test_generate_ctx_accepts_star_type_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            local star = { _def_type = "star_type", id = "yellow_dwarf" }
            ctx:spawn_empty_system("A", {0, 0, 0}, star)
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.spawned_systems[0].star_type, "yellow_dwarf");
    }

    #[test]
    fn test_choose_capitals_ctx_assignments() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let systems = vec![
            SystemSnapshot {
                name: "Sol".into(),
                position: [0.0, 0.0, 0.0],
                star_type: "yellow_dwarf".into(),
                capital_for_faction: None,
            },
            SystemSnapshot {
                name: "Beta".into(),
                position: [10.0, 0.0, 0.0],
                star_type: "red_dwarf".into(),
                capital_for_faction: None,
            },
        ];
        let ctx = ChooseCapitalsCtx::new(systems, vec!["humanity_empire".into()]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx:assign_capital(1, ctx.factions[1])
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.assignments.len(), 1);
        assert_eq!(actions.assignments[0].system_index, 1);
        assert_eq!(actions.assignments[0].faction_id, "humanity_empire");
    }

    #[test]
    fn test_choose_capitals_ctx_assign_from_system_table() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let systems = vec![SystemSnapshot {
            name: "Sol".into(),
            position: [0.0, 0.0, 0.0],
            star_type: "yellow_dwarf".into(),
            capital_for_faction: None,
        }];
        let ctx = ChooseCapitalsCtx::new(systems, vec!["humanity_empire".into()]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx:assign_capital(ctx.systems[1], "humanity_empire")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.assignments[0].system_index, 1);
    }

    #[test]
    fn test_choose_capitals_ctx_exposes_fields() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let systems = vec![SystemSnapshot {
            name: "Sol".into(),
            position: [1.0, 2.0, 3.0],
            star_type: "yellow_dwarf".into(),
            capital_for_faction: None,
        }];
        let ctx = ChooseCapitalsCtx::new(systems, vec!["humanity".into(), "xeno".into()]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let num_factions: i64 = lua.load("return #ctx.factions").eval().unwrap();
        assert_eq!(num_factions, 2);
        let num_systems: i64 = lua.load("return #ctx.systems").eval().unwrap();
        assert_eq!(num_systems, 1);
        let first_faction: String = lua.load("return ctx.factions[1]").eval().unwrap();
        assert_eq!(first_faction, "humanity");
        let first_star: String = lua.load("return ctx.systems[1].star_type").eval().unwrap();
        assert_eq!(first_star, "yellow_dwarf");
    }

    #[test]
    fn test_initialize_system_ctx_spawn_planet() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let ctx = InitializeSystemCtx::new(
            3,
            "Sol".into(),
            "yellow_dwarf".into(),
            [1.0, 2.0, 3.0],
            true,
        );
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx:spawn_planet("Earth", "terrestrial", {
                habitability = 1.0,
                max_building_slots = 6,
            })
            ctx:spawn_planet("Mars", "terrestrial")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert!(actions.override_default_planets);
        assert_eq!(actions.spawned_planets.len(), 2);
        assert_eq!(actions.spawned_planets[0].name, "Earth");
        assert_eq!(actions.spawned_planets[0].planet_type, "terrestrial");
        assert_eq!(actions.spawned_planets[0].attrs.habitability, Some(1.0));
        assert_eq!(
            actions.spawned_planets[0].attrs.max_building_slots,
            Some(6)
        );
        assert_eq!(actions.spawned_planets[1].name, "Mars");
    }

    #[test]
    fn test_initialize_system_ctx_no_planets_keeps_default() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let ctx = InitializeSystemCtx::new(
            1,
            "Test".into(),
            "yellow_dwarf".into(),
            [0.0, 0.0, 0.0],
            false,
        );
        lua.globals().set("ctx", ctx.clone()).unwrap();

        // No spawn_planet calls, so override should remain false.
        lua.load(r#"ctx:set_attributes({ name = "Renamed", surveyed = true })"#)
            .exec()
            .unwrap();

        let actions = ctx.take_actions();
        assert!(!actions.override_default_planets);
        assert!(actions.spawned_planets.is_empty());
        assert_eq!(actions.name, Some("Renamed".into()));
        assert_eq!(actions.surveyed, Some(true));
    }

    #[test]
    fn test_initialize_system_ctx_exposes_fields() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let ctx = InitializeSystemCtx::new(
            7,
            "Proxima".into(),
            "red_dwarf".into(),
            [1.5, -2.0, 0.1],
            false,
        );
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let idx: i64 = lua.load("return ctx.index").eval().unwrap();
        assert_eq!(idx, 7);
        let name: String = lua.load("return ctx.name").eval().unwrap();
        assert_eq!(name, "Proxima");
        let star: String = lua.load("return ctx.star_type").eval().unwrap();
        assert_eq!(star, "red_dwarf");
        let cap: bool = lua.load("return ctx.is_capital").eval().unwrap();
        assert!(!cap);
        let x: f64 = lua.load("return ctx.position.x").eval().unwrap();
        assert!((x - 1.5).abs() < 1e-10);
    }

    #[test]
    fn test_initialize_system_ctx_explicit_override() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let ctx = InitializeSystemCtx::new(
            1,
            "Test".into(),
            "yellow_dwarf".into(),
            [0.0, 0.0, 0.0],
            false,
        );
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(r#"ctx:override_default_planets()"#).exec().unwrap();
        let actions = ctx.take_actions();
        assert!(actions.override_default_planets);
        assert!(actions.spawned_planets.is_empty());
    }

    // --- #199: FtlGraph + connectivity API tests -----------------------

    fn spawn(ctx: &GalaxyGenerateCtx, name: &str, pos: [f64; 3]) {
        ctx.actions
            .lock()
            .unwrap()
            .spawned_systems
            .push(SpawnedEmptySystemSpec {
                name: name.into(),
                position: pos,
                star_type: "yellow_dwarf".into(),
                planets: PredefinedPlanetsForSpawn::default(),
                capital_for_faction: None,
            });
    }

    #[test]
    fn test_ftl_graph_connected_components_basic() {
        // Cluster A at origin (two nodes within range), Cluster B far away.
        let graph = FtlGraph::build(
            vec![
                FtlNode {
                    index: 1,
                    name: "A1".into(),
                    star_type: "yellow_dwarf".into(),
                    position: [0.0, 0.0, 0.0],
                },
                FtlNode {
                    index: 2,
                    name: "A2".into(),
                    star_type: "yellow_dwarf".into(),
                    position: [5.0, 0.0, 0.0],
                },
                FtlNode {
                    index: 3,
                    name: "B1".into(),
                    star_type: "yellow_dwarf".into(),
                    position: [100.0, 0.0, 0.0],
                },
            ],
            10.0,
        );
        // A1 and A2 should be in same component; B1 isolated.
        assert_eq!(graph.root(0), graph.root(1));
        assert_ne!(graph.root(0), graph.root(2));
    }

    #[test]
    fn test_ctx_build_ftl_graph_unreachable_from() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        spawn(&ctx, "A1", [0.0, 0.0, 0.0]);
        spawn(&ctx, "A2", [5.0, 0.0, 0.0]);
        spawn(&ctx, "B1", [100.0, 0.0, 0.0]);
        spawn(&ctx, "B2", [105.0, 0.0, 0.0]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let count: i64 = lua
            .load(
                r#"
                local g = ctx:build_ftl_graph(10.0)
                local unreach = g:unreachable_from(1)
                return #unreach
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(count, 2, "A1 should see B1, B2 as unreachable");
    }

    #[test]
    fn test_ctx_build_ftl_graph_connected_components_count() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        spawn(&ctx, "A1", [0.0, 0.0, 0.0]);
        spawn(&ctx, "A2", [5.0, 0.0, 0.0]);
        spawn(&ctx, "B1", [100.0, 0.0, 0.0]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let (num_components, largest): (i64, i64) = lua
            .load(
                r#"
                local g = ctx:build_ftl_graph(10.0)
                local comps = g:connected_components()
                local largest = 0
                for _, c in ipairs(comps) do
                    if #c > largest then largest = #c end
                end
                return #comps, largest
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(num_components, 2);
        assert_eq!(largest, 2);
    }

    #[test]
    fn test_ctx_closest_cross_cluster_pair() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        spawn(&ctx, "A1", [0.0, 0.0, 0.0]);
        spawn(&ctx, "A2", [5.0, 0.0, 0.0]);
        spawn(&ctx, "B1", [30.0, 0.0, 0.0]);
        spawn(&ctx, "B2", [40.0, 0.0, 0.0]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let (a_name, b_name): (String, String) = lua
            .load(
                r#"
                local g = ctx:build_ftl_graph(10.0)
                local a, b = g:closest_cross_cluster_pair(1)
                return a.name, b.name
                "#,
            )
            .eval()
            .unwrap();
        // Closest cross-cluster pair from A-cluster should be A2 (5) and B1 (30).
        assert_eq!(a_name, "A2");
        assert_eq!(b_name, "B1");
    }

    #[test]
    fn test_ctx_insert_bridge_at_appends_system() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();
        lua.load(r#"ctx:insert_bridge_at({10.0, 20.0, 0.0}, "yellow_dwarf")"#)
            .exec()
            .unwrap();
        let actions = ctx.take_actions();
        assert_eq!(actions.spawned_systems.len(), 1);
        assert_eq!(actions.spawned_systems[0].position, [10.0, 20.0, 0.0]);
        assert_eq!(actions.spawned_systems[0].star_type, "yellow_dwarf");
        assert!(actions.spawned_systems[0].name.starts_with("Bridge-"));
    }

    #[test]
    fn test_ctx_insert_bridge_at_default_star_type() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();
        lua.load(r#"ctx:insert_bridge_at({ x = 1.0, y = 2.0, z = 3.0 })"#)
            .exec()
            .unwrap();
        let actions = ctx.take_actions();
        assert_eq!(actions.spawned_systems[0].star_type, "yellow_dwarf");
    }

    #[test]
    fn test_ctx_pick_provisional_capital_closest_to_origin() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        spawn(&ctx, "Far", [100.0, 0.0, 0.0]);
        spawn(&ctx, "Near", [2.0, 0.0, 0.0]);
        spawn(&ctx, "Mid", [10.0, 0.0, 0.0]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let name: String = lua
            .load(
                r#"
                local cap = ctx:pick_provisional_capital()
                return cap.name
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(name, "Near");
    }

    #[test]
    fn test_ctx_pick_provisional_capital_nil_when_empty() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();
        let nil: bool = lua
            .load(r#"return ctx:pick_provisional_capital() == nil"#)
            .eval()
            .unwrap();
        assert!(nil);
    }

    #[test]
    fn test_ctx_settings_exposes_initial_ftl_range() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();
        let v: f64 = lua
            .load("return ctx.settings.initial_ftl_range")
            .eval()
            .unwrap();
        assert!((v - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_ctx_systems_field_reflects_spawns() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        spawn(&ctx, "S1", [1.0, 2.0, 3.0]);
        spawn(&ctx, "S2", [4.0, 5.0, 6.0]);
        lua.globals().set("ctx", ctx.clone()).unwrap();
        let (count, name): (i64, String) = lua
            .load(r#"return #ctx.systems, ctx.systems[2].name"#)
            .eval()
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(name, "S2");
    }

    #[test]
    fn test_last_registered_hook_returns_none_when_absent() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let result = last_registered_hook(lua, GENERATE_EMPTY_HANDLERS).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_last_registered_hook_returns_last() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_galaxy_generate_empty(function(ctx) _first_called = true end)
            on_galaxy_generate_empty(function(ctx) _second_called = true end)
            "#,
        )
        .exec()
        .unwrap();

        let func = last_registered_hook(lua, GENERATE_EMPTY_HANDLERS)
            .unwrap()
            .expect("should find last hook");
        func.call::<()>(mlua::Value::Nil).unwrap();

        let first: Option<bool> = lua.globals().get("_first_called").unwrap();
        let second: Option<bool> = lua.globals().get("_second_called").unwrap();
        assert!(first.is_none(), "only the last registration should run");
        assert_eq!(second, Some(true));
    }
}
