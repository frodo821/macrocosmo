//! Integration tests for the player choice system (#152).

mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::choice::{
    apply_pending_choice_selection, drain_pending_choices, evaluate_choice_availability,
    PendingChoice, PendingChoiceSelection,
};
use macrocosmo::colony::ResourceStockpile;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::galaxy::StarSystem;
use macrocosmo::player::PlayerEmpire;
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::technology::{GameFlags, GlobalParams, TechTree};
use macrocosmo::time_system::GameSpeed;

/// Helper: minimal App with just the resources the drain + apply systems need.
/// Does NOT use `test_app` because we want a tight, choice-focused fixture.
fn choice_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(ScriptEngine::new().unwrap());
    app.init_resource::<PendingChoice>();
    app.init_resource::<PendingChoiceSelection>();
    // Start unpaused so we can observe drain auto-pausing.
    app.insert_resource(GameSpeed {
        hexadies_per_second: 1.0,
        previous_speed: 1.0,
    });
    app
}

/// Spawn a player empire with the components apply_pending_choice_selection
/// needs to mutate (GameFlags, ScopedFlags, GlobalParams).
fn spawn_player_empire(world: &mut World) -> Entity {
    world
        .spawn((
            PlayerEmpire,
            GameFlags::default(),
            ScopedFlags::default(),
            GlobalParams::default(),
        ))
        .id()
}

/// Spawn a capital system with a stockpile large enough to cover test costs.
fn spawn_capital_system(world: &mut World, minerals: u64, energy: u64) -> Entity {
    world
        .spawn((
            StarSystem {
                name: "Capital".into(),
                surveyed: true,
                is_capital: true,
                star_type: "default".into(),
            },
            ResourceStockpile {
                minerals: Amt::units(minerals),
                energy: Amt::units(energy),
                research: Amt::ZERO,
                food: Amt::ZERO,
                authority: Amt::ZERO,
            },
        ))
        .id()
}

#[test]
fn show_choice_lua_call_populates_pending_queue() {
    let engine = ScriptEngine::new().unwrap();
    let lua = engine.lua();

    lua.load(
        r#"
        show_choice {
            title = "Ancient Ruins",
            description = "A team of surveyors has uncovered strange ruins.",
            options = {
                { label = "Study the ruins", cost = { minerals = 100 } },
                { label = "Leave the ruins alone" },
            },
        }
        "#,
    )
    .exec()
    .unwrap();

    let pending: mlua::Table = lua.globals().get("_pending_choices").unwrap();
    assert_eq!(pending.len().unwrap(), 1);
    let entry: mlua::Table = pending.get(1).unwrap();
    assert_eq!(entry.get::<String>("title").unwrap(), "Ancient Ruins");
    let opts: mlua::Table = entry.get("options").unwrap();
    assert_eq!(opts.len().unwrap(), 2);
}

#[test]
fn show_choice_returns_choice_reference_table() {
    let engine = ScriptEngine::new().unwrap();
    let lua = engine.lua();

    let ref_table: mlua::Table = lua
        .load(
            r#"
            return show_choice {
                title = "Dilemma",
                description = "Pick one",
                options = { { label = "A" } },
            }
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(ref_table.get::<String>("_def_type").unwrap(), "choice");
    let id = ref_table.get::<String>("id").unwrap();
    assert!(!id.is_empty(), "choice id should be non-empty");
}

#[test]
fn drain_enqueues_active_choice_and_pauses_game() {
    let mut app = choice_app();

    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                show_choice {
                    title = "Choose",
                    description = "",
                    options = { { label = "Yes" }, { label = "No" } },
                }
                "#,
            )
            .exec()
            .unwrap();
    }
    assert!(!app.world().resource::<GameSpeed>().is_paused());

    app.add_systems(Update, drain_pending_choices);
    app.update();

    let pending = app.world().resource::<PendingChoice>();
    assert!(pending.is_active());
    let active = pending.current.as_ref().unwrap();
    assert_eq!(active.title, "Choose");
    assert_eq!(active.options.len(), 2);

    let speed = app.world().resource::<GameSpeed>();
    assert!(speed.is_paused(), "drain should auto-pause when a choice is active");
}

#[test]
fn evaluate_marks_unmet_condition_and_cost() {
    // Build a choice with a tech-gated option and a too-expensive option.
    let engine = ScriptEngine::new().unwrap();
    let lua = engine.lua();
    lua.load(
        r#"
        show_choice {
            title = "T",
            description = "",
            options = {
                { label = "Need tech", condition = has_tech("nonexistent_tech") },
                { label = "Too pricey", cost = { minerals = 1000 } },
                { label = "Available" },
            },
        }
        "#,
    )
    .exec()
    .unwrap();

    // Pull the pending entry and parse through the full drain path. We use
    // `drain_pending_choices` indirectly via a tiny app so we don't need to
    // expose the internal parser.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(engine);
    app.init_resource::<PendingChoice>();
    app.insert_resource(GameSpeed::default());
    app.add_systems(Update, drain_pending_choices);
    app.update();

    let mut pending = app.world_mut().resource_mut::<PendingChoice>();
    let active = pending.current.as_mut().expect("choice should be active");

    let tech = TechTree::default();
    let game_flags = GameFlags::default();
    let scoped = ScopedFlags::default();
    evaluate_choice_availability(
        active,
        &tech,
        &game_flags,
        &scoped,
        Some((Amt::units(100), Amt::units(100))),
    );

    assert!(active.options[0].condition_unmet, "tech-gated option must be unmet");
    assert!(!active.options[0].cost_unmet);
    assert!(active.options[1].cost_unmet, "expensive option must be unaffordable");
    assert!(active.options[1].unmet_reason.contains("minerals"));
    assert!(!active.options[2].condition_unmet);
    assert!(!active.options[2].cost_unmet);
}

#[test]
fn apply_selection_runs_on_chosen_and_unpauses() {
    let mut app = choice_app();
    let _empire = spawn_player_empire(app.world_mut());
    let _capital = spawn_capital_system(app.world_mut(), 500, 500);

    app.add_systems(
        Update,
        (drain_pending_choices, apply_pending_choice_selection).chain(),
    );

    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                show_choice {
                    title = "Ruins",
                    description = "",
                    options = {
                        {
                            label = "Study",
                            on_chosen = function(scope)
                                return {
                                    scope:set_flag("ruins_studied", true, { description = "Studied the ruins" }),
                                }
                            end,
                        },
                    },
                }
                "#,
            )
            .exec()
            .unwrap();
    }

    // Tick once to drain & parse the choice.
    app.update();
    assert!(app.world().resource::<PendingChoice>().is_active());
    assert!(app.world().resource::<GameSpeed>().is_paused());

    // Stage the selection (option 1) and tick again.
    app.world_mut().resource_mut::<PendingChoiceSelection>().pick = Some(1);
    app.update();

    // Selection consumed, choice cleared, game unpaused.
    let pending = app.world().resource::<PendingChoice>();
    assert!(!pending.is_active(), "pending choice should be cleared");
    let speed = app.world().resource::<GameSpeed>();
    assert!(!speed.is_paused(), "game should be unpaused after resolving");

    // DescriptiveEffect::SetFlag should have mirrored into GameFlags/ScopedFlags.
    let mut q = app.world_mut().query_filtered::<&GameFlags, With<PlayerEmpire>>();
    let flags = q.single(app.world()).unwrap();
    assert!(flags.flags.contains("ruins_studied"));
}

#[test]
fn apply_selection_deducts_cost_from_capital_stockpile() {
    let mut app = choice_app();
    let _empire = spawn_player_empire(app.world_mut());
    let capital = spawn_capital_system(app.world_mut(), 500, 500);

    app.add_systems(
        Update,
        (drain_pending_choices, apply_pending_choice_selection).chain(),
    );

    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                show_choice {
                    title = "Pricey",
                    description = "",
                    options = { { label = "Pay", cost = { minerals = 150, energy = 50 } } },
                }
                "#,
            )
            .exec()
            .unwrap();
    }

    app.update();

    // The dialog system would normally call evaluate_choice_availability before
    // exposing the option; here we call it manually so apply_pending_choice_selection
    // sees cost_unmet = false.
    {
        let tech = TechTree::default();
        let game_flags = GameFlags::default();
        let scoped = ScopedFlags::default();
        let capital_stock: Option<(Amt, Amt)> = {
            let sp = app.world().get::<ResourceStockpile>(capital).unwrap();
            Some((sp.minerals, sp.energy))
        };
        let mut pending = app.world_mut().resource_mut::<PendingChoice>();
        if let Some(active) = pending.current.as_mut() {
            evaluate_choice_availability(active, &tech, &game_flags, &scoped, capital_stock);
        }
    }

    app.world_mut().resource_mut::<PendingChoiceSelection>().pick = Some(1);
    app.update();

    let stockpile = app.world().get::<ResourceStockpile>(capital).unwrap();
    assert_eq!(stockpile.minerals, Amt::units(350), "minerals should be debited by 150");
    assert_eq!(stockpile.energy, Amt::units(450), "energy should be debited by 50");
}

#[test]
fn apply_selection_rejects_unavailable_option() {
    // Ensure clicking an unavailable option is a no-op: pending choice stays
    // active, game stays paused, no effect applied.
    let mut app = choice_app();
    let _empire = spawn_player_empire(app.world_mut());
    // Capital has only 10 minerals; choice requires 1000.
    let capital = spawn_capital_system(app.world_mut(), 10, 10);

    app.add_systems(
        Update,
        (drain_pending_choices, apply_pending_choice_selection).chain(),
    );

    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                show_choice {
                    title = "Impossible",
                    description = "",
                    options = {
                        {
                            label = "Pay 1000",
                            cost = { minerals = 1000 },
                            on_chosen = function(scope)
                                return { scope:set_flag("paid", true, {}) }
                            end,
                        },
                    },
                }
                "#,
            )
            .exec()
            .unwrap();
    }

    app.update();

    // Mark the cost as unavailable by evaluating availability (as the UI does).
    {
        let tech = TechTree::default();
        let game_flags = GameFlags::default();
        let scoped = ScopedFlags::default();
        let capital_stock: Option<(Amt, Amt)> = {
            let sp = app.world().get::<ResourceStockpile>(capital).unwrap();
            Some((sp.minerals, sp.energy))
        };
        let mut pending = app.world_mut().resource_mut::<PendingChoice>();
        if let Some(active) = pending.current.as_mut() {
            evaluate_choice_availability(active, &tech, &game_flags, &scoped, capital_stock);
            assert!(active.options[0].cost_unmet);
        }
    }

    // Stage the "unavailable" pick and tick.
    app.world_mut().resource_mut::<PendingChoiceSelection>().pick = Some(1);
    app.update();

    // Choice must still be active.
    assert!(
        app.world().resource::<PendingChoice>().is_active(),
        "unavailable pick should not clear the choice"
    );
    // Stockpile untouched.
    let stockpile = app.world().get::<ResourceStockpile>(capital).unwrap();
    assert_eq!(stockpile.minerals, Amt::units(10));
    // Flag must not have been set.
    let mut q = app.world_mut().query_filtered::<&GameFlags, With<PlayerEmpire>>();
    let flags = q.single(app.world()).unwrap();
    assert!(!flags.flags.contains("paid"));
}
