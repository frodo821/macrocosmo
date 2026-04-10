mod common;

use bevy::prelude::*;
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
    use technology::{ResearchQueue, ResearchPool, TechId, TechTree, Technology, TechBranch, TechCost, LastResearchTick};
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
        branch: TechBranch::Physics,
        cost: TechCost::research_only(Amt::units(100)),
        prerequisites: vec![],
        description: String::new(),
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
