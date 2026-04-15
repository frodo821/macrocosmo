//! #336: `ColonyView.owner_empire_id` must be resolved by a direct
//! `FactionOwner` component read, not via the legacy
//! `colony -> planet -> system -> Sovereignty` chain.
//!
//! Background: plan-297 (PR #330) attached `FactionOwner` to every
//! colony spawn path (capital, colonization queue, faction on_game_start,
//! settling), making a dedicated `Colony.Owner` component redundant.
//! The plan agent for #336 concluded this issue is **refactor-only, no
//! new component required** — these tests pin the new behaviour and
//! regression-guard against the old chain lookup creeping back in.
//!
//! See `docs/plan-336-colony-owner-component.md` for the full rationale.

use bevy::prelude::*;

use macrocosmo::colony::Colony;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::{Planet, Sovereignty, StarSystem};
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::gamestate_scope::{GamestateMode, dispatch_with_gamestate};
use macrocosmo::ship::Owner;
use macrocosmo::time_system::GameClock;

mod common;
use common::fixture::load_fixture;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run a Lua chunk with a `dispatch_with_gamestate` payload exposed as
/// `_evt`. Mirrors the shared helper in `tests/lua_view_types.rs`; kept
/// local to this file to keep the #336 scope self-contained.
fn with_gamestate<R, F>(world: &mut World, mode: GamestateMode, f: F) -> R
where
    F: FnOnce(&mlua::Lua) -> R,
    R: Default,
{
    let out = std::cell::RefCell::new(R::default());
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        let payload = lua.create_table().unwrap();
        dispatch_with_gamestate(lua, world, &payload, mode, |lua_inner, p| {
            lua_inner.globals().set("_evt", p.clone())?;
            *out.borrow_mut() = f(lua_inner);
            Ok(())
        })
        .unwrap();
    });
    out.into_inner()
}

/// Return the `owner_empire_id` field (if any) that a Lua-side
/// `ColonyView` reports for `colony`. Returns the bit-packed entity id
/// (`Entity::to_bits`) or `None` when the view omits the field.
fn colony_view_owner_bits(world: &mut World, colony: Entity) -> Option<u64> {
    let id = colony.to_bits();
    with_gamestate(world, GamestateMode::ReadOnly, |lua| {
        lua.globals().set("_colony_id", id).unwrap();
        lua.load(
            r#"
            local view = _evt.gamestate:colony(_colony_id)
            return view.owner_empire_id
            "#,
        )
        .eval::<Option<u64>>()
        .unwrap()
    })
}

/// Spawn the minimum Bevy `World` needed to exercise `build_colony_view`
/// through `dispatch_with_gamestate`. The `ScriptEngine` resource is
/// required by `with_gamestate`.
fn fresh_world() -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(1));
    world.insert_resource(ScriptEngine::new().unwrap());
    world
}

fn spawn_empire(world: &mut World, name: &str) -> Entity {
    world
        .spawn((Empire { name: name.into() }, PlayerEmpire))
        .id()
}

fn spawn_system(world: &mut World) -> Entity {
    world
        .spawn(StarSystem {
            name: "TestSys".into(),
            surveyed: true,
            is_capital: false,
            star_type: "yellow_dwarf".into(),
        })
        .id()
}

fn spawn_planet(world: &mut World, system: Entity) -> Entity {
    world
        .spawn(Planet {
            name: "TestPlanet".into(),
            system,
            planet_type: "terrestrial".into(),
        })
        .id()
}

fn spawn_colony_on(world: &mut World, planet: Entity) -> Entity {
    world
        .spawn(Colony {
            planet,
            population: 100.0,
            growth_rate: 0.01,
        })
        .id()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Happy path: a Colony tagged with `FactionOwner(empire)` resolves to
/// that empire's entity id on the Lua view — even when the parent system
/// has no `Sovereignty` component at all. This is the main behavioural
/// change vs. the pre-#336 chain lookup.
#[test]
fn colony_view_owner_reads_faction_owner_directly() {
    let mut world = fresh_world();
    let empire = spawn_empire(&mut world, "Terran");
    let system = spawn_system(&mut world);
    let planet = spawn_planet(&mut world, system);
    let colony = spawn_colony_on(&mut world, planet);
    world.entity_mut(colony).insert(FactionOwner(empire));
    // Note: no Sovereignty on `system`. Old chain lookup returned nil here.

    let owner = colony_view_owner_bits(&mut world, colony);
    assert_eq!(
        owner,
        Some(empire.to_bits()),
        "ColonyView.owner_empire_id must come from FactionOwner, not the system Sovereignty chain"
    );
}

/// Negative case: a bare Colony with no `FactionOwner` (neutral /
/// test-only spawn) reports `nil`. The refactor intentionally drops the
/// system-Sovereignty fallback (plan-336 §3.1, user decision 1): if
/// `FactionOwner` is missing we return nothing rather than invent an
/// owner from Sovereignty, avoiding two sources of truth.
#[test]
fn colony_view_owner_nil_without_faction_owner() {
    let mut world = fresh_world();
    let _empire = spawn_empire(&mut world, "Terran");
    let system = spawn_system(&mut world);
    let planet = spawn_planet(&mut world, system);
    let colony = spawn_colony_on(&mut world, planet);
    // Deliberately do NOT attach FactionOwner.

    let owner = colony_view_owner_bits(&mut world, colony);
    assert_eq!(
        owner, None,
        "ColonyView.owner_empire_id must be nil when the Colony lacks FactionOwner"
    );
}

/// Regression pin for plan-336 §3.1 / §6.1 #3 (Sovereignty-independence):
/// even when `system.Sovereignty.owner = Empire(B)` — e.g. because
/// empire B's Core ship is transiently present — the administrative
/// owner reported by `ColonyView.owner_empire_id` must follow the
/// Colony's `FactionOwner`, which is empire A. The pre-#336 chain
/// lookup confused these two axes and would return B.
#[test]
fn colony_view_owner_ignores_sovereignty() {
    let mut world = fresh_world();
    let empire_a = spawn_empire(&mut world, "Alpha");
    let empire_b = world
        .spawn(Empire {
            name: "Beta".into(),
        })
        .id();
    let system = spawn_system(&mut world);
    world.entity_mut(system).insert(Sovereignty {
        owner: Some(Owner::Empire(empire_b)),
        control_score: 1.0,
    });
    let planet = spawn_planet(&mut world, system);
    let colony = spawn_colony_on(&mut world, planet);
    world.entity_mut(colony).insert(FactionOwner(empire_a));

    let owner = colony_view_owner_bits(&mut world, colony);
    assert_eq!(
        owner,
        Some(empire_a.to_bits()),
        "ColonyView.owner_empire_id must track FactionOwner (admin), not Sovereignty (military presence)"
    );
}

/// Save-format-stability guard: loading the committed `minimal_game.bin`
/// fixture (which has a Colony *without* `FactionOwner` — its seed world
/// attaches `FactionOwner` only to the StarSystem) still produces a
/// well-formed `ColonyView`. The colony view exposes `population` and
/// `planet_id` as before; `owner_empire_id` is `nil` because the saved
/// colony carries no `FactionOwner`. This ensures the refactor does not
/// break round-tripping older content and that no `SAVE_VERSION` bump
/// is required.
#[test]
fn colony_view_owner_minimal_game_fixture_load_compatible() {
    let mut app = load_fixture("minimal_game.bin");
    // Fixture has exactly one Colony (Earth under Sol). See
    // `tests/fixtures_smoke.rs::build_seed_world`.
    let colony_entity = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<Colony>>();
        let colonies: Vec<_> = q.iter(world).collect();
        assert_eq!(
            colonies.len(),
            1,
            "minimal_game fixture is expected to have exactly one Colony"
        );
        colonies[0]
    };

    // Ensure the test app carries a ScriptEngine; load_fixture only
    // adds MinimalPlugins and persistence resources.
    if app.world().get_resource::<ScriptEngine>().is_none() {
        app.world_mut()
            .insert_resource(ScriptEngine::new().unwrap());
    }

    let owner = colony_view_owner_bits(app.world_mut(), colony_entity);
    // The fixture's colony has no FactionOwner, so nil is expected.
    // The important invariant is that the view builds without panicking
    // and that the field's shape matches the post-refactor contract.
    assert_eq!(
        owner, None,
        "minimal_game fixture Colony has no FactionOwner; owner_empire_id must be nil"
    );
}
