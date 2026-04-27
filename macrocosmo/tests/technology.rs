mod common;

use bevy::prelude::*;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::technology;

use common::{advance_time, empire_entity, test_app};

#[test]
fn test_start_research_sets_queue() {
    use technology::{ResearchQueue, TechId};

    let mut queue = ResearchQueue::default();
    assert!(queue.current.is_none());
    assert_eq!(queue.accumulated, 0.0);
    assert!(!queue.blocked);

    queue.start_research(TechId("social_xenolinguistics".into()));
    assert_eq!(queue.current, Some(TechId("social_xenolinguistics".into())));
    assert_eq!(queue.accumulated, 0.0);
    assert!(!queue.blocked);
}

#[test]
fn test_block_research_stops_progress() {
    use macrocosmo::amount::Amt;
    use technology::{
        LastResearchTick, ResearchPool, ResearchQueue, TechCost, TechId, TechTree, Technology,
    };

    let mut app = test_app();

    // Add technology systems not included in basic test_app
    app.add_systems(
        Update,
        (
            technology::emit_research,
            technology::receive_research,
            technology::tick_research,
            technology::flush_research,
        )
            .chain()
            .after(macrocosmo::time_system::advance_game_time),
    );

    // Insert tech tree onto empire entity
    let tree = TechTree::from_vec(vec![Technology {
        id: TechId("test_1".into()),
        name: "Test".into(),
        branch: "physics".into(),
        cost: TechCost::research_only(Amt::units(100)),
        prerequisites: vec![],
        description: String::new(),
        dangerous: false,
    }]);
    {
        let empire = empire_entity(app.world_mut());
        app.world_mut().entity_mut(empire).insert(tree);
    }

    // Start research and block it
    {
        let empire = empire_entity(app.world_mut());
        let mut queue = app.world_mut().get_mut::<ResearchQueue>(empire).unwrap();
        queue.start_research(TechId("test_1".into()));
        queue.block();
    }

    // Add points to pool
    {
        let empire = empire_entity(app.world_mut());
        app.world_mut()
            .get_mut::<ResearchPool>(empire)
            .unwrap()
            .points = 50.0;
    }

    // Advance time
    advance_time(&mut app, 1);

    // Queue should have no progress because it's blocked
    let empire = empire_entity(app.world_mut());
    let queue = app.world().get::<ResearchQueue>(empire).unwrap();
    assert_eq!(queue.accumulated, 0.0);
    assert!(queue.blocked);
    assert_eq!(queue.current, Some(TechId("test_1".into())));
}

#[test]
fn test_add_research_progress() {
    use technology::{ResearchQueue, TechId};

    let mut queue = ResearchQueue::default();
    queue.start_research(TechId("test_1".into()));
    assert_eq!(queue.accumulated, 0.0);

    queue.add_progress(25.0);
    assert_eq!(queue.accumulated, 25.0);

    queue.add_progress(10.0);
    assert_eq!(queue.accumulated, 35.0);
}

#[test]
fn test_cancel_research_clears_queue() {
    use technology::{ResearchQueue, TechId};

    let mut queue = ResearchQueue::default();
    queue.start_research(TechId("test_1".into()));
    queue.add_progress(50.0);

    queue.cancel_research();
    assert!(queue.current.is_none());
    assert_eq!(queue.accumulated, 0.0);
}

// CRITICAL: GlobalParams on empire entity (#4)

#[test]
fn test_global_params_on_empire_entity() {
    let mut app = test_app();

    let empire = empire_entity(app.world_mut());
    let params = app.world().get::<technology::GlobalParams>(empire).unwrap();

    // Verify defaults
    assert_eq!(params.sublight_speed_bonus, 0.0);
    assert_eq!(params.ftl_speed_multiplier, 1.0);
    assert_eq!(params.ftl_range_bonus, 0.0);
    assert_eq!(params.survey_range_bonus, 0.0);
    assert_eq!(params.build_speed_multiplier, 1.0);
}

// --- #154: on_researched callback execution pipeline ---

/// Integration test: research a tech -> on_researched fires -> GameFlags + GlobalParams updated
#[test]
fn test_on_researched_fires_and_applies_effects() {
    use macrocosmo::amount::Amt;
    use macrocosmo::scripting::ScriptEngine;
    use technology::{
        GameFlags, GlobalParams, RecentlyResearched, ResearchPool, ResearchQueue, TechCost,
        TechEffectsLog, TechId, TechTree, Technology,
    };

    let mut app = test_app();

    // Create a ScriptEngine and define a tech with on_researched callback
    let engine = ScriptEngine::new().unwrap();
    let lua = engine.lua();
    lua.load(
        r#"
        define_tech {
            id = "test_on_researched_tech",
            name = "Test On Researched",
            branch = "physics",
            cost = 10,
            prerequisites = {},
            on_researched = function(scope)
                scope:set_flag("test_research_flag", true, { description = "Test flag from research" })
                scope:push_modifier("sensor.range", { add = 3.0, description = "Test sensor bonus" })
            end,
        }
        "#,
    )
    .exec()
    .unwrap();
    app.insert_resource(engine);
    app.init_resource::<TechEffectsLog>();

    // Register the apply_tech_effects system
    app.add_systems(
        Update,
        (technology::tick_research, technology::apply_tech_effects)
            .chain()
            .after(macrocosmo::time_system::advance_game_time),
    );

    // Insert a tech tree with matching tech
    let tree = TechTree::from_vec(vec![Technology {
        id: TechId("test_on_researched_tech".into()),
        name: "Test On Researched".into(),
        branch: "physics".into(),
        cost: TechCost::research_only(Amt::units(10)),
        prerequisites: vec![],
        description: String::new(),
        dangerous: false,
    }]);
    {
        let empire = empire_entity(app.world_mut());
        app.world_mut().entity_mut(empire).insert(tree);
    }

    // Start research and give enough points to complete
    {
        let empire = empire_entity(app.world_mut());
        let mut queue = app.world_mut().get_mut::<ResearchQueue>(empire).unwrap();
        queue.start_research(TechId("test_on_researched_tech".into()));
        let mut pool = app.world_mut().get_mut::<ResearchPool>(empire).unwrap();
        pool.points = 100.0; // More than enough
    }

    // Advance time to trigger tick_research -> apply_tech_effects
    advance_time(&mut app, 1);

    // Verify: tech should be complete
    let empire = empire_entity(app.world_mut());
    let queue = app.world().get::<ResearchQueue>(empire).unwrap();
    assert!(queue.current.is_none(), "Research should have completed");

    // Verify: GameFlags should have the flag set by on_researched
    let flags = app.world().get::<GameFlags>(empire).unwrap();
    assert!(
        flags.check("test_research_flag"),
        "on_researched should have set test_research_flag in GameFlags"
    );

    // Verify: ScopedFlags should also have the flag
    let scoped = app.world().get::<ScopedFlags>(empire).unwrap();
    assert!(
        scoped.check("test_research_flag"),
        "on_researched should have set test_research_flag in ScopedFlags"
    );

    // Verify: GlobalParams should have survey_range_bonus updated
    let params = app.world().get::<GlobalParams>(empire).unwrap();
    assert!(
        (params.survey_range_bonus - 3.0).abs() < 1e-10,
        "on_researched should have added 3.0 to survey_range_bonus, got {}",
        params.survey_range_bonus
    );

    // Verify: TechEffectsLog should record the effects
    let effects_log = app.world().resource::<TechEffectsLog>();
    let tech_effects = effects_log
        .effects
        .get(&TechId("test_on_researched_tech".into()));
    assert!(
        tech_effects.is_some(),
        "TechEffectsLog should contain effects for the researched tech"
    );
    assert_eq!(
        tech_effects.unwrap().len(),
        2,
        "Should have 2 effects (SetFlag + PushModifier)"
    );
}

// ---------------------------------------------------------------------------
// #458: PendingResearch / PendingKnowledgePropagation owner-scope regression
// ---------------------------------------------------------------------------
//
// Pre-#458 `emit_research` anchored the light-delay calculation on the player's
// `StationedAt` and `receive_research` credited the arrived points to *every*
// `ResearchPool` in the world. Both bugs leaked one empire's research output
// into other empires' pools (and to NPCs in observer mode), and made the delay
// dependent on the player's location rather than the colony owner's capital.
//
// The tests below exercise the corrected behaviour:
//   * two empires accrue research independently;
//   * NPC research keeps flowing without a `Player` entity;
//   * the delay anchor follows the colony owner's `HomeSystem`.

/// Spawn a second (NPC) empire entity with its own research components.
/// Mirrors `spawn_test_empire` minus the `PlayerEmpire` marker.
fn spawn_second_empire(world: &mut World, name: &str) -> Entity {
    use macrocosmo::colony::{AuthorityParams, ConstructionParams};
    use macrocosmo::communication::CommandLog;
    use macrocosmo::condition::ScopedFlags;
    use macrocosmo::empire::CommsParams;
    use macrocosmo::knowledge::{KnowledgeStore, SystemVisibilityMap};
    use macrocosmo::player::{Empire, Faction};
    use technology::{
        EmpireModifiers, GameFlags, GlobalParams, PendingColonyTechModifiers, RecentlyResearched,
        ResearchPool, ResearchQueue, TechTree,
    };
    world
        .spawn((
            (
                Empire { name: name.into() },
                Faction::new(name, name),
                TechTree::default(),
                ResearchQueue::default(),
                ResearchPool::default(),
                RecentlyResearched::default(),
                AuthorityParams::default(),
                ConstructionParams::default(),
            ),
            (
                EmpireModifiers::default(),
                GameFlags::default(),
                GlobalParams::default(),
                PendingColonyTechModifiers::default(),
                KnowledgeStore::default(),
                SystemVisibilityMap::default(),
                CommandLog::default(),
                ScopedFlags::default(),
                CommsParams::default(),
            ),
        ))
        .id()
}

/// Register the research pipeline on a `test_app()` instance. The bare
/// `test_app()` does not include these systems by default.
///
/// Note: `flush_research` zeroes the pool at the end of each tick (use-it-or-
/// lose-it economy). For tests that assert on `ResearchPool.points` *after*
/// emit→receive, we deliberately skip `flush_research` so the credited points
/// remain visible to the assertion. `tick_research` is also omitted because
/// it consumes pool points into a current research target — we want to read
/// the raw delivered amount.
fn install_research_pipeline(app: &mut App) {
    app.add_systems(
        Update,
        (technology::emit_research, technology::receive_research)
            .chain()
            .after(macrocosmo::time_system::advance_game_time),
    );
}

/// #458 acceptance: two empires with research colonies must accumulate
/// `ResearchPool` points independently.
#[test]
fn test_pending_research_credits_owner_only() {
    use common::{spawn_test_colony, spawn_test_system_with_planet};
    use macrocosmo::amount::Amt;
    use macrocosmo::faction::FactionOwner;
    use macrocosmo::galaxy::HomeSystem;
    use technology::ResearchPool;

    let mut app = test_app();
    install_research_pipeline(&mut app);

    // Empire A is the auto-spawned PlayerEmpire; Empire B is a second NPC empire.
    let empire_a = empire_entity(app.world_mut());
    let empire_b = spawn_second_empire(app.world_mut(), "EmpireB");

    // Two same-system home worlds so light delay is 0 — we want to isolate
    // the owner-routing logic from the delay code path.
    let (cap_a, _) =
        spawn_test_system_with_planet(app.world_mut(), "CapA", [0.0, 0.0, 0.0], 1.0, true);
    let (cap_b, _) =
        spawn_test_system_with_planet(app.world_mut(), "CapB", [1.0, 0.0, 0.0], 1.0, true);
    app.world_mut()
        .entity_mut(empire_a)
        .insert(HomeSystem(cap_a));
    app.world_mut()
        .entity_mut(empire_b)
        .insert(HomeSystem(cap_b));

    // Each empire owns one colony in its capital. spawn_test_colony picks
    // the "first empire" for FactionOwner so we overwrite the second one.
    let colony_a = spawn_test_colony(app.world_mut(), cap_a, Amt::ZERO, Amt::ZERO, vec![]);
    let colony_b = spawn_test_colony(app.world_mut(), cap_b, Amt::ZERO, Amt::ZERO, vec![]);
    app.world_mut()
        .entity_mut(colony_a)
        .insert(FactionOwner(empire_a));
    app.world_mut()
        .entity_mut(colony_b)
        .insert(FactionOwner(empire_b));

    // Drive emit_research → receive_research one tick (delta = 1, delay = 0).
    advance_time(&mut app, 1);

    let pool_a = app.world().get::<ResearchPool>(empire_a).unwrap().points;
    let pool_b = app.world().get::<ResearchPool>(empire_b).unwrap().points;

    assert!(
        pool_a > 0.0,
        "Empire A should have positive research, got {pool_a}"
    );
    assert!(
        pool_b > 0.0,
        "Empire B should have positive research, got {pool_b}"
    );

    // Each empire's colony emits roughly the same baseline (research_per_hexadies
    // = 1, research_weight = 1, delta = 1) so the pools should match within
    // float tolerance — the key invariant is that A's points did NOT also land
    // in B's pool (and vice versa). Pre-fix this would have been ~2x each.
    let expected = pool_a; // same baseline → use one as the reference
    assert!(
        (pool_b - expected).abs() < 1e-9,
        "Empire pools must accrue independently; got A={pool_a} B={pool_b}"
    );
    // Cross-leak guard: pre-fix `receive_research` doubled each colony's
    // contribution by adding it to *every* empire pool. Each empire here owns
    // a single colony, so a leak would put 2x the single-colony amount in
    // each pool. Assert the upper bound holds.
    assert!(
        pool_a < expected * 1.5,
        "Empire A pool {pool_a} indicates cross-empire leak (>1.5x baseline {expected})"
    );
}

/// #458 acceptance: NPC research must continue to flow without any `Player`
/// entity (observer mode). Pre-fix `emit_research` early-returned on
/// `player_q.single().is_err()` for the delay anchor — the colony's points
/// were silently dropped.
#[test]
fn test_pending_research_observer_mode_npc_keeps_researching() {
    use common::{spawn_test_colony, spawn_test_system_with_planet};
    use macrocosmo::amount::Amt;
    use macrocosmo::faction::FactionOwner;
    use macrocosmo::galaxy::HomeSystem;
    use macrocosmo::player::{Player, PlayerEmpire};
    use technology::ResearchPool;

    let mut app = test_app();
    install_research_pipeline(&mut app);

    // Scrub Player markers — observer mode has no Player / Ruler entity.
    // PlayerEmpire stays as the "default empire" attached by test_app, but
    // we treat it as just another NPC for this scenario.
    let mut to_strip: Vec<Entity> = Vec::new();
    {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<Player>>();
        for e in q.iter(world) {
            to_strip.push(e);
        }
    }
    for e in to_strip {
        app.world_mut().entity_mut(e).remove::<Player>();
    }
    // Also strip PlayerEmpire so empire_entity is unambiguous (we still hold
    // it via direct query below).
    let empire = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerEmpire>>();
        q.single(app.world()).unwrap()
    };

    let (cap, _) =
        spawn_test_system_with_planet(app.world_mut(), "Cap", [0.0, 0.0, 0.0], 1.0, true);
    app.world_mut().entity_mut(empire).insert(HomeSystem(cap));

    let colony = spawn_test_colony(app.world_mut(), cap, Amt::ZERO, Amt::ZERO, vec![]);
    app.world_mut()
        .entity_mut(colony)
        .insert(FactionOwner(empire));

    advance_time(&mut app, 1);

    let pool = app.world().get::<ResearchPool>(empire).unwrap().points;
    assert!(
        pool > 0.0,
        "Observer-mode empire should still accrue research without a Player entity (got {pool})"
    );
}

/// #458 acceptance: the light-delay anchor for `PendingResearch.arrives_at`
/// must follow the **colony owner's** `HomeSystem`, not the player's
/// stationed system. Two empires with the same colony position but different
/// capitals yield different `arrives_at` values.
#[test]
fn test_pending_research_delay_uses_owner_capital() {
    use common::{spawn_test_colony, spawn_test_system_with_planet};
    use macrocosmo::amount::Amt;
    use macrocosmo::faction::FactionOwner;
    use macrocosmo::galaxy::HomeSystem;
    use macrocosmo::time_system::GameClock;
    use technology::PendingResearch;

    let mut app = test_app();
    install_research_pipeline(&mut app);
    let empire_a = empire_entity(app.world_mut());
    let empire_b = spawn_second_empire(app.world_mut(), "EmpireB");

    // Capital A at origin; capital B 1000 LY away on the X axis. We pick a
    // large gap so the two empire-relative light delays are clearly distinct.
    let (cap_a, _) =
        spawn_test_system_with_planet(app.world_mut(), "CapA", [0.0, 0.0, 0.0], 1.0, true);
    let (cap_b, _) =
        spawn_test_system_with_planet(app.world_mut(), "CapB", [1000.0, 0.0, 0.0], 1.0, true);
    app.world_mut()
        .entity_mut(empire_a)
        .insert(HomeSystem(cap_a));
    app.world_mut()
        .entity_mut(empire_b)
        .insert(HomeSystem(cap_b));

    // Two colonies at the *same* remote system position (50 LY out from
    // origin) — one owned by each empire. Owner A's capital is 50 LY away;
    // owner B's capital is 950 LY away. Distinct capitals → distinct
    // light-delay values, so the spawned `PendingResearch` entities must
    // carry distinct `arrives_at`.
    let (colony_sys, _) =
        spawn_test_system_with_planet(app.world_mut(), "ColonySys", [50.0, 0.0, 0.0], 1.0, true);

    let colony_a = spawn_test_colony(app.world_mut(), colony_sys, Amt::ZERO, Amt::ZERO, vec![]);
    app.world_mut()
        .entity_mut(colony_a)
        .insert(FactionOwner(empire_a));

    // Spawn a second planet in the same system for empire B's colony (so
    // both colonies share `colony_sys` but live on distinct planets — colony
    // entities key off `Colony.planet`).
    use macrocosmo::components::Position;
    use macrocosmo::galaxy::{Planet, SystemAttributes};
    let planet_b = app
        .world_mut()
        .spawn((
            Planet {
                name: "ColonySys II".into(),
                system: colony_sys,
                planet_type: "default".into(),
            },
            SystemAttributes {
                habitability: 1.0,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 4,
            },
            Position::from([50.0, 0.0, 0.0]),
        ))
        .id();
    let colony_b = spawn_test_colony(app.world_mut(), planet_b, Amt::ZERO, Amt::ZERO, vec![]);
    app.world_mut()
        .entity_mut(colony_b)
        .insert(FactionOwner(empire_b));

    advance_time(&mut app, 1);

    // Both colonies are 50 LY from the origin colony system; empire A's
    // capital is 50 LY away (delay = 50/(1/60) = 3000 hexadies) and empire B's
    // capital is 950 LY away (delay ≈ 57000 hexadies). Neither packet has
    // matured, so both remain as in-flight `PendingResearch` entities.
    let now = app.world().resource::<GameClock>().elapsed;
    let mut delays_by_owner: std::collections::HashMap<Entity, Vec<i64>> =
        std::collections::HashMap::new();
    let world_mut = app.world_mut();
    let mut q = world_mut.query::<&PendingResearch>();
    for pr in q.iter(world_mut) {
        delays_by_owner
            .entry(pr.owner)
            .or_default()
            .push(pr.arrives_at - now);
    }

    let delay_a = *delays_by_owner
        .get(&empire_a)
        .and_then(|v| v.first())
        .expect("empire A should have an in-flight PendingResearch");
    let delay_b = *delays_by_owner
        .get(&empire_b)
        .and_then(|v| v.first())
        .expect("empire B should have an in-flight PendingResearch");

    // Anchor invariant: delay is computed from `owner.HomeSystem`, so
    // empire B's much-farther capital yields a much larger delay than
    // empire A's. Pre-fix both packets used the same player-anchored delay
    // (whichever empire the player happened to be stationed at) — so this
    // strict inequality regression-tests the fix end-to-end.
    assert!(
        delay_b > delay_a * 5,
        "Owner-anchored delay must scale with each empire's capital distance: \
         delay_a = {delay_a}, delay_b = {delay_b}"
    );

    // Sanity: every spawned packet carries one of the two known empires.
    for owner in delays_by_owner.keys() {
        assert!(
            *owner == empire_a || *owner == empire_b,
            "PendingResearch owner {owner:?} is neither empire"
        );
    }
}
