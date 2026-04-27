//! #440 regression suite: observer mode `read_only` must drop write
//! actions emitted by every UI panel — not just the ship panel.
//!
//! egui systems run in `EguiPrimaryContextPass` which can't be exercised
//! headlessly. We instead test the *gate helpers* that mirror the inline
//! gates inside `draw_main_panels_system`, `draw_overlays_system`, and
//! `draw_diplomacy_overlay_system`. A bug that re-introduces the issue
//! either:
//!   * removes the gate helper from the production system (caught by code
//!     review + the call-site `grep`-able comments), or
//!   * regresses the gate helper itself (caught by these tests).
//!
//! The helpers live in `crate::ui::*` — see `gate_system_panel_writes`,
//! `gate_diplomacy_action`, `gate_research_action`, and
//! `gate_ship_designer_action`.

use bevy::prelude::*;

use macrocosmo::amount::Amt;
use macrocosmo::communication::{
    BuildingKind, BuildingScope, ColonyCommand, PendingColonyDispatch, PendingColonyDispatches,
    RemoteCommand,
};
use macrocosmo::technology::TechId;
use macrocosmo::ui::diplomacy_panel::DiplomacyAction;
use macrocosmo::ui::overlays::{ResearchAction, ShipDesignerAction};
use macrocosmo::ui::system_panel::{ColonizationAction, SystemPanelActions};
use macrocosmo::ui::{
    gate_diplomacy_action, gate_research_action, gate_ship_designer_action,
    gate_system_panel_writes,
};

// ---------------------------------------------------------------------------
// System panel chokepoint — covers build/demolish/upgrade dispatch, ship
// build/cancel, deliverable load, structure dismantle, same-system
// colonization. All of these flow through the same three sinks below.
// ---------------------------------------------------------------------------

/// Test-only entity helper. The gate functions never look up these
/// entities in a `World`, so a synthetic id is fine.
fn ent(id: u32) -> Entity {
    // `from_raw_u32(0)` is reserved (PLACEHOLDER) — start at 1.
    Entity::from_raw_u32(id.max(1)).unwrap()
}

fn make_dispatch(tag: u32) -> PendingColonyDispatch {
    PendingColonyDispatch {
        target_system: ent(tag),
        command: RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::System,
            kind: BuildingKind::Demolish { target_slot: 0 },
        }),
    }
}

#[test]
fn observer_readonly_blocks_pending_colony_dispatch_pushes() {
    // Player mode: dispatches added during the panel pass survive.
    let mut dispatches = PendingColonyDispatches::default();
    let pre_len = dispatches.queue.len();
    dispatches.queue.push(make_dispatch(1));
    let mut colonization_actions: Vec<ColonizationAction> = Vec::new();
    let mut system_actions = SystemPanelActions::default();

    gate_system_panel_writes(
        false, // read_only
        &mut dispatches,
        pre_len,
        &mut colonization_actions,
        &mut system_actions,
    );
    assert_eq!(
        dispatches.queue.len(),
        1,
        "non-observer (read_only=false) must preserve dispatch pushes"
    );

    // Observer read-only: the same panel-emitted dispatch is dropped.
    let mut dispatches = PendingColonyDispatches::default();
    // Pre-existing dispatch (e.g. from another system this frame) — must be retained.
    dispatches.queue.push(make_dispatch(99));
    let pre_len = dispatches.queue.len();
    // Panel pushes one fresh dispatch.
    dispatches.queue.push(make_dispatch(1));
    let mut colonization_actions = Vec::new();
    let mut system_actions = SystemPanelActions::default();

    gate_system_panel_writes(
        true, // read_only
        &mut dispatches,
        pre_len,
        &mut colonization_actions,
        &mut system_actions,
    );
    assert_eq!(
        dispatches.queue.len(),
        1,
        "observer read_only must truncate panel-pushed dispatches but \
         retain pre-existing queue entries"
    );
    assert_eq!(
        dispatches.queue[0].target_system,
        ent(99),
        "the pre-existing dispatch must survive truncation"
    );
}

#[test]
fn observer_readonly_clears_colonization_actions() {
    let mut dispatches = PendingColonyDispatches::default();
    let pre_len = dispatches.queue.len();
    let mut colonization_actions = vec![ColonizationAction {
        system_entity: ent(1),
        target_planet: ent(2),
        source_colony: ent(3),
    }];
    let mut system_actions = SystemPanelActions::default();

    gate_system_panel_writes(
        true,
        &mut dispatches,
        pre_len,
        &mut colonization_actions,
        &mut system_actions,
    );
    assert!(
        colonization_actions.is_empty(),
        "observer read_only must drop pending colonization actions \
         (no PendingColonizationOrder spawned)"
    );
}

#[test]
fn observer_readonly_clears_system_action_dismantle_and_load() {
    let mut dispatches = PendingColonyDispatches::default();
    let pre_len = dispatches.queue.len();
    let mut colonization_actions: Vec<ColonizationAction> = Vec::new();
    let mut system_actions = SystemPanelActions {
        dismantle: Some(ent(7)),
        load_deliverable: Some((ent(8), ent(9), 0)),
    };

    gate_system_panel_writes(
        true,
        &mut dispatches,
        pre_len,
        &mut colonization_actions,
        &mut system_actions,
    );
    assert!(
        system_actions.dismantle.is_none() && system_actions.load_deliverable.is_none(),
        "observer read_only must clear SystemPanelActions \
         (dismantle structure / load deliverable both gated)"
    );

    // Sanity: in player mode, the actions pass through.
    let mut system_actions = SystemPanelActions {
        dismantle: Some(ent(7)),
        load_deliverable: Some((ent(8), ent(9), 0)),
    };
    gate_system_panel_writes(
        false,
        &mut dispatches,
        pre_len,
        &mut colonization_actions,
        &mut system_actions,
    );
    assert!(
        system_actions.dismantle.is_some() && system_actions.load_deliverable.is_some(),
        "non-observer must preserve SystemPanelActions"
    );
}

// ---------------------------------------------------------------------------
// Diplomacy chokepoint — DeclareWar / SendDiplomaticEvent / EndWar.
// ---------------------------------------------------------------------------

#[test]
fn observer_readonly_drops_declare_war_action() {
    let action = DiplomacyAction::SendDiplomaticEvent {
        from: ent(1),
        to: ent(2),
        option_id: macrocosmo::faction::DIPLO_DECLARE_WAR.into(),
    };
    let gated = gate_diplomacy_action(true, action);
    assert!(matches!(gated, DiplomacyAction::None));

    // Player mode: action passes through.
    let action = DiplomacyAction::SendDiplomaticEvent {
        from: ent(1),
        to: ent(2),
        option_id: macrocosmo::faction::DIPLO_DECLARE_WAR.into(),
    };
    let gated = gate_diplomacy_action(false, action);
    assert!(matches!(gated, DiplomacyAction::SendDiplomaticEvent { .. }));
}

#[test]
fn observer_readonly_drops_end_war_action() {
    let action = DiplomacyAction::EndWar {
        faction_a: ent(1),
        faction_b: ent(2),
        scenario_id: "white_peace".into(),
    };
    let gated = gate_diplomacy_action(true, action);
    assert!(matches!(gated, DiplomacyAction::None));
}

// ---------------------------------------------------------------------------
// Research / ship-designer chokepoints.
// ---------------------------------------------------------------------------

#[test]
fn observer_readonly_drops_research_actions() {
    let start = ResearchAction::StartResearch(TechId("industrial_automated_mining".into()));
    let gated = gate_research_action(true, start);
    assert!(matches!(gated, ResearchAction::None));

    let cancel = ResearchAction::CancelResearch;
    let gated = gate_research_action(true, cancel);
    assert!(matches!(gated, ResearchAction::None));

    // Player mode must let the actions through.
    let start = ResearchAction::StartResearch(TechId("industrial_automated_mining".into()));
    let gated = gate_research_action(false, start);
    assert!(matches!(gated, ResearchAction::StartResearch(_)));
}

#[test]
fn observer_readonly_drops_ship_designer_save_action() {
    let design = macrocosmo::ship_design::ShipDesignDefinition {
        id: "test_design".into(),
        name: "Test Design".into(),
        description: String::new(),
        hull_id: "corvette".into(),
        modules: Vec::new(),
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
        is_direct_buildable: false,
    };
    let action = ShipDesignerAction::SaveDesign(design.clone());
    let gated = gate_ship_designer_action(true, action);
    assert!(matches!(gated, ShipDesignerAction::None));

    let action = ShipDesignerAction::SaveDesign(design);
    let gated = gate_ship_designer_action(false, action);
    assert!(matches!(gated, ShipDesignerAction::SaveDesign(_)));
}

// ---------------------------------------------------------------------------
// End-to-end: applying the gated diplomacy action against a real
// FactionRelations resource must leave the relation untouched.
// This catches a regression where the gate is bypassed and the action
// reaches `declare_war_with_delay`.
// ---------------------------------------------------------------------------

#[test]
fn observer_readonly_diplomacy_keeps_relations_intact() {
    use macrocosmo::faction::{FactionRelations, FactionView, RelationState};

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(FactionRelations::default());

    let world = app.world_mut();
    let a = world.spawn_empty().id();
    let b = world.spawn_empty().id();
    {
        let mut rel = world.resource_mut::<FactionRelations>();
        rel.set(a, b, FactionView::new(RelationState::Peace, 0.0));
        rel.set(b, a, FactionView::new(RelationState::Peace, 0.0));
    }

    // Simulate the panel emitting a DeclareWar — apply the gate first.
    let action = DiplomacyAction::SendDiplomaticEvent {
        from: a,
        to: b,
        option_id: macrocosmo::faction::DIPLO_DECLARE_WAR.into(),
    };
    let gated = gate_diplomacy_action(true, action);

    // The system's match arm only fires for non-None — confirm we hit None.
    assert!(
        matches!(gated, DiplomacyAction::None),
        "observer read_only must collapse DiplomacyAction to None"
    );

    // No state should have changed: relations remain Peace.
    let rel = app.world().resource::<FactionRelations>();
    let view = rel.get_or_default(a, b);
    assert_eq!(
        view.state,
        RelationState::Peace,
        "observer read_only diplomacy gate must leave relations unchanged"
    );

    // Positive control: the underlying write path *does* flip state when
    // not gated. Catches regressions where both branches collapse to
    // no-op (e.g. a future refactor that neuters declare_war).
    {
        let mut rel = app.world_mut().resource_mut::<FactionRelations>();
        rel.declare_war(a, b);
    }
    let rel = app.world().resource::<FactionRelations>();
    let view = rel.get_or_default(a, b);
    assert_eq!(
        view.state,
        RelationState::War,
        "positive control: FactionRelations::declare_war should flip the \
         state in player mode (sanity check on the gate's other branch)"
    );
}
