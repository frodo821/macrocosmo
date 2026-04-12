//! #199 — Integration regression tests for the FTL connectivity bridge pass.
//!
//! Covers both the Lua-facing graph primitives (already unit-tested in
//! `galaxy_gen_ctx`) and the end-to-end guarantee that, after default galaxy
//! generation, every spawned system is FTL-reachable from the capital under
//! the configured `initial_ftl_range`.

use std::path::PathBuf;

use bevy::prelude::*;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{generate_galaxy, StarSystem};
use macrocosmo::scripting::galaxy_api::{
    PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition, StarTypeRegistry,
};
use macrocosmo::scripting::map_api::{
    parse_map_types, parse_predefined_systems, MapTypeRegistry, PredefinedSystemRegistry,
};
use macrocosmo::scripting::ScriptEngine;

/// Locate the `scripts/` directory next to this crate's source tree. The
/// tests workspace CWD is the macrocosmo crate; in worktrees the path is
/// always `<crate>/scripts`.
fn find_scripts_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR at compile time is `<crate>` (i.e. macrocosmo/).
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("scripts")
}

fn minimal_star_registry() -> StarTypeRegistry {
    let mut star_reg = StarTypeRegistry::default();
    star_reg.types.push(StarTypeDefinition {
        id: "yellow_dwarf".into(),
        name: "Yellow Dwarf".into(),
        description: String::new(),
        color: [1.0, 1.0, 0.7],
        planet_lambda: 2.0,
        max_planets: 5,
        habitability_bonus: 0.0,
        weight: 1.0,
        modifiers: Vec::new(),
    });
    star_reg
}

fn minimal_planet_registry() -> PlanetTypeRegistry {
    let mut planet_reg = PlanetTypeRegistry::default();
    for (id, hab) in [("terrestrial", 0.8), ("barren", 0.1)] {
        planet_reg.types.push(PlanetTypeDefinition {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            base_habitability: hab,
            base_slots: 4,
            resource_bias: ResourceBias {
                minerals: 1.0,
                energy: 1.0,
                research: 1.0,
            },
            weight: 1.0,
        });
    }
    planet_reg
}

/// Build an app with the real `scripts/` directory loaded (so
/// `scripts/galaxy/map_types.lua` registers the `on_after_phase_a` hook) and
/// with minimal star/planet registries to keep the test hermetic.
fn build_app_with_real_scripts() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    let scripts_dir = find_scripts_dir();
    assert!(
        scripts_dir.join("init.lua").exists(),
        "scripts/init.lua not found at {}",
        scripts_dir.display()
    );
    let engine = ScriptEngine::new_with_scripts_dir(scripts_dir).expect("engine");
    // Load the real init.lua so on_after_phase_a is registered.
    engine
        .load_file(&engine.scripts_dir().join("init.lua"))
        .expect("load init.lua");
    app.insert_resource(engine);

    app.insert_resource(minimal_star_registry());
    app.insert_resource(minimal_planet_registry());

    // Parse the predefined / map_type registries so generate_galaxy sees them.
    let (predefined, map_types, active) = {
        let engine = app.world().resource::<ScriptEngine>();
        let lua = engine.lua();
        let predefined = parse_predefined_systems(lua).unwrap_or_default();
        let map_types = parse_map_types(lua).unwrap_or_default();
        let active = macrocosmo::scripting::map_api::read_active_map_type(lua);
        (predefined, map_types, active)
    };
    let mut p_reg = PredefinedSystemRegistry::default();
    for d in predefined {
        p_reg.systems.insert(d.id.clone(), d);
    }
    let mut m_reg = MapTypeRegistry::default();
    for d in map_types {
        m_reg.types.insert(d.id.clone(), d);
    }
    m_reg.current = active;
    app.insert_resource(p_reg);
    app.insert_resource(m_reg);

    app.add_systems(Startup, generate_galaxy);
    app
}

/// Collect (position, is_capital) for every spawned star system.
fn collect_systems(app: &mut App) -> Vec<([f64; 3], bool)> {
    let mut out = Vec::new();
    let mut query = app.world_mut().query::<(&StarSystem, &Position)>();
    for (s, p) in query.iter(app.world()) {
        out.push(([p.x, p.y, p.z], s.is_capital));
    }
    out
}

/// BFS over the undirected FTL graph (edge iff pairwise distance <= range);
/// return the indices reachable from `start`.
fn reachable_from(systems: &[([f64; 3], bool)], start: usize, ftl_range: f64) -> Vec<usize> {
    let n = systems.len();
    let r2 = ftl_range * ftl_range;
    let mut seen = vec![false; n];
    let mut stack = vec![start];
    seen[start] = true;
    while let Some(i) = stack.pop() {
        for j in 0..n {
            if seen[j] {
                continue;
            }
            let dx = systems[i].0[0] - systems[j].0[0];
            let dy = systems[i].0[1] - systems[j].0[1];
            let dz = systems[i].0[2] - systems[j].0[2];
            if dx * dx + dy * dy + dz * dz <= r2 {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    seen.iter()
        .enumerate()
        .filter_map(|(i, b)| if *b { Some(i) } else { None })
        .collect()
}

/// #199 regression: after default galaxy generation, every system must be
/// FTL-reachable from the capital under `initial_ftl_range = 10.0`. Repeated
/// across many samples to catch spiral-RNG edge cases.
#[test]
fn test_default_map_capital_reachability() {
    const SAMPLES: usize = 20;
    const FTL_RANGE: f64 = 10.0;
    let mut disconnected_samples = 0;
    for sample in 0..SAMPLES {
        let mut app = build_app_with_real_scripts();
        app.update();
        let systems = collect_systems(&mut app);
        assert!(!systems.is_empty(), "sample {sample}: no systems generated");
        let capital_idx = systems
            .iter()
            .position(|(_, cap)| *cap)
            .expect("capital must exist");
        let reachable = reachable_from(&systems, capital_idx, FTL_RANGE);
        if reachable.len() != systems.len() {
            disconnected_samples += 1;
            eprintln!(
                "sample {sample}: {}/{} reachable from capital",
                reachable.len(),
                systems.len()
            );
        }
    }
    assert_eq!(
        disconnected_samples, 0,
        "expected every sample to be fully connected under FTL range {FTL_RANGE}, \
         but {disconnected_samples}/{SAMPLES} had unreachable systems"
    );
}

/// GalaxyParams.initial_ftl_range surfaces on `ctx.settings.initial_ftl_range`
/// as 10.0 by default (see `generate_galaxy` in `galaxy/generation.rs`). We
/// verify this indirectly via the Lua side: a probe hook records the value.
#[test]
fn test_initial_ftl_range_default_is_10() {
    let mut app = build_app_with_real_scripts();
    // Install a probe that records settings.initial_ftl_range into a global.
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                _probe_initial_ftl_range = -1
                on_after_phase_a(function(ctx)
                    _probe_initial_ftl_range = ctx.settings.initial_ftl_range
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    app.update();
    let engine = app.world().resource::<ScriptEngine>();
    let v: f64 = engine
        .lua()
        .globals()
        .get("_probe_initial_ftl_range")
        .unwrap();
    assert!((v - 10.0).abs() < 1e-9, "expected 10.0, got {v}");
}
