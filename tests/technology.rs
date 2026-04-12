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
    use technology::{ResearchQueue, ResearchPool, TechId, TechTree, Technology, TechCost, LastResearchTick};
    use macrocosmo::amount::Amt;

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
        app.world_mut().get_mut::<ResearchPool>(empire).unwrap().points = 50.0;
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
        GameFlags, GlobalParams, RecentlyResearched, ResearchPool, ResearchQueue,
        TechCost, TechEffectsLog, TechId, TechTree, Technology,
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
        (
            technology::tick_research,
            technology::apply_tech_effects,
        )
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
