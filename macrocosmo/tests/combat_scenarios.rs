//! Fixed-balance fleet combat scenario tests.
//!
//! These tests pin every balance input (weapon damage, hostile strength/HP/evasion,
//! ship hull/shield/armor) locally inside the test so production balance changes
//! do NOT alter expected values. The goal is to detect combat regressions that
//! span multiple subsystems (faction gating, ROE, damage flow, hostile
//! retaliation, despawn ordering) in a way that single-ship combat unit tests
//! cannot.
//!
//! **Regression motivation (#308).** Merging #308 removed `HostilePresence` and
//! the `attach_hostile_faction_owners` backfill system; at the same time 20
//! raw hostile spawn sites lost their `FactionOwner` components. Existing
//! unit tests only asserted "HP moved" and so all passed, but the combat gate
//! was silently bypassed. These scenario tests assert **exact numeric HP**
//! after fixed turn counts with two fleets, so the same class of silent
//! gating regression fails loudly.
//!
//! ## Balance fixture
//!
//! A local helper `install_scenario_weapon` overrides the `ModuleRegistry` in
//! place with a deterministic weapon:
//!
//! - `track = 1000.0, precision = 1.0, evasion = 0.0` → `hit_chance = 1.0`
//!   (every shot hits; `rng.random::<f64>()` returns < 1.0 so `< chance` is
//!   always true).
//! - `cooldown = 12` → 1 shot per ship per hexadies (12 combat turns / 12).
//! - `shield_piercing = 0.0, armor_piercing = 0.0` but ships spawn with
//!   `shield = 0, armor = 0`, so damage flows straight to the hull phase —
//!   no RNG is consulted for defense layers.
//! - All `*_damage_div = 0.0` → damage is the exact `*_damage` value; no
//!   random jitter.
//!
//! The hostile retaliation path (`apply_flat_damage_to_ship`) is fully
//! deterministic: `total_damage = strength * combat_turns` divided evenly
//! across docked ships.

mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::faction::{FactionRelations, FactionView, HostileFactions, RelationState};
use macrocosmo::galaxy::HostileHitpoints;
use macrocosmo::player::{Faction, PlayerEmpire};
use macrocosmo::ship::*;
use macrocosmo::ship_design::{ModuleDefinition, ModuleRegistry, WeaponStats};

use macrocosmo::ai::convert::to_ai_faction;
use macrocosmo::ai::schema::ids::evidence;
use macrocosmo::ai::AiBusResource;

use common::{advance_time, spawn_test_system, test_app};

// ---- Fixed balance constants ----
//
// All combat numbers below are derived from these. Change them and the
// scenario expectations below change accordingly.
const SCENARIO_WEAPON_ID: &str = "scenario_laser_fixed";
/// Weapon cooldown in combat turns. 12 turns/hexadies → `cooldown = 12` means
/// exactly 1 shot per ship per hexadies.
const WEAPON_COOLDOWN: i64 = 12;
/// Per-shot hull damage. No shield/armor on scenario ships → flows to hull.
const WEAPON_HULL_DAMAGE: f64 = 10.0;

/// Install the scenario weapon into the already-populated `ModuleRegistry`
/// built by `create_test_module_registry()`. Overwrites any existing entry
/// with the same id so the balance is unambiguous.
fn install_scenario_weapon(app: &mut App) {
    let mut module_reg = app.world_mut().resource_mut::<ModuleRegistry>();
    module_reg.insert(ModuleDefinition {
        id: SCENARIO_WEAPON_ID.to_string(),
        name: "Scenario Laser".to_string(),
        description: String::new(),
        slot_type: "weapon".to_string(),
        modifiers: Vec::new(),
        weapon: Some(WeaponStats {
            track: 1000.0,
            precision: 1.0,
            cooldown: WEAPON_COOLDOWN,
            range: 100.0,
            // All layers deal the same damage; only hull phase is reached
            // because scenario ships have zero shield/armor.
            shield_damage: WEAPON_HULL_DAMAGE,
            shield_damage_div: 0.0,
            shield_piercing: 0.0,
            armor_damage: WEAPON_HULL_DAMAGE,
            armor_damage_div: 0.0,
            armor_piercing: 0.0,
            hull_damage: WEAPON_HULL_DAMAGE,
            hull_damage_div: 0.0,
        }),
        cost_minerals: Amt::ZERO,
        cost_energy: Amt::ZERO,
        build_time: 0,
        power_cost: 0,
        power_output: 0,
        size: macrocosmo::ship_design::ModuleSize::Small,
        prerequisites: None,
        upgrade_to: Vec::new(),
    });
}

/// Spawn a bare-bones combat-ready ship at `sys` with the scenario weapon
/// equipped. Ship has `Owner::Neutral`; `advance_time` auto-migrates Neutral
/// ships onto the test empire when hostiles are present. `attach_faction`
/// controls whether `FactionOwner` is attached immediately (used by the
/// bug-injection test to simulate the #308 regression).
///
/// Returns the ship entity.
fn spawn_scenario_ship(
    app: &mut App,
    name: &str,
    sys: Entity,
    hull: f64,
    roe: RulesOfEngagement,
) -> Entity {
    app.world_mut()
        .spawn((
            Ship {
                name: name.to_string(),
                design_id: "explorer_mk1".to_string(),
                hull_id: "corvette".to_string(),
                modules: vec![EquippedModule {
                    slot_type: "weapon".to_string(),
                    module_id: SCENARIO_WEAPON_ID.to_string(),
                }],
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: Entity::PLACEHOLDER,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: sys },
            Position::from([0.0, 0.0, 0.0]),
            ShipHitpoints {
                hull,
                hull_max: hull,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
            roe,
        ))
        .id()
}

// =============================================================================
// Scenario 1 — 2-ship fleet vs 1 space-creature, Aggressive ROE
// =============================================================================

/// Two-ship player fleet at Aggressive ROE engaging a single space-creature
/// hostile. Verifies exact HP values after each of 5 hexadies. Covers:
///
/// - Faction gate: ships auto-migrate to the test empire; default Neutral/-100
///   relations make `can_attack_aggressive()` return true.
/// - Combined damage output: 2 ships × 1 shot × 10 hull-damage = 20/hexadies.
/// - Hostile retaliation split: `strength * 12 / 2` per ship per hexadies.
/// - Despawn gate: when the hostile dies in the weapon phase, retaliation is
///   skipped that tick — the final tick leaves ship HP untouched.
#[test]
fn scenario_two_ship_fleet_vs_space_creature_aggressive() {
    // --- Fixed balance for this scenario ---
    // Hostile
    const HOSTILE_START_HP: f64 = 100.0;
    const HOSTILE_STRENGTH: f64 = 4.0;
    const HOSTILE_EVASION: f64 = 0.0;
    // Ships
    const SHIP_HULL: f64 = 100.0;

    // Derived per-hexadies numbers (see module docstring for the formula):
    //   damage_to_hostile_per_hex = 2 ships * 1 shot * WEAPON_HULL_DAMAGE = 20
    //   damage_to_each_ship_per_hex = HOSTILE_STRENGTH * 12 / 2 ships = 24
    const DMG_TO_HOSTILE_PER_HEX: f64 = 20.0;
    const DMG_TO_EACH_SHIP_PER_HEX: f64 = 24.0;

    let mut app = test_app();
    install_scenario_weapon(&mut app);

    let sys = spawn_test_system(
        app.world_mut(),
        "Scenario-1",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn hostile BEFORE ships — spawn_raw_hostile auto-initializes
    // HostileFactions and attaches FactionOwner.
    let hostile = common::spawn_raw_hostile(
        app.world_mut(),
        sys,
        HOSTILE_START_HP,
        HOSTILE_START_HP,
        HOSTILE_STRENGTH,
        HOSTILE_EVASION,
        "space_creature",
    );

    let ship_a = spawn_scenario_ship(
        &mut app,
        "Alpha",
        sys,
        SHIP_HULL,
        RulesOfEngagement::Aggressive,
    );
    let ship_b = spawn_scenario_ship(
        &mut app,
        "Bravo",
        sys,
        SHIP_HULL,
        RulesOfEngagement::Aggressive,
    );

    // --- Verify faction gate setup ---
    // advance_time's auto-migration (triggered by the hostile + neutral ships)
    // re-homes both ships onto the test empire; we inspect relations here.
    // Trigger migration with a zero-length advance so ship owners flip
    // before assertions but no combat turns pass.
    advance_time(&mut app, 0);

    let empire = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerEmpire>>();
        q.single(app.world()).expect("empire")
    };
    let hostile_faction = app
        .world()
        .resource::<HostileFactions>()
        .space_creature
        .expect("space_creature faction");
    let view = app
        .world()
        .resource::<FactionRelations>()
        .get_or_default(empire, hostile_faction);
    assert_eq!(view.state, RelationState::Neutral);
    assert_eq!(view.standing, -100.0);
    assert!(
        view.can_attack_aggressive(),
        "Neutral/-100 must enable Aggressive engagement — this is the #308 regression surface",
    );

    // Ownership should be Empire(empire) after migration.
    for &s in &[ship_a, ship_b] {
        let ship = app.world().get::<Ship>(s).unwrap();
        assert!(
            matches!(ship.owner, Owner::Empire(e) if e == empire),
            "Ship {} should have been re-homed onto the empire",
            ship.name,
        );
    }

    // --- Tick 1..=4: hostile survives, retaliation applies ---
    let mut expected_hostile_hp = HOSTILE_START_HP;
    let mut expected_ship_hp = SHIP_HULL;
    for tick in 1..=4 {
        advance_time(&mut app, 1);
        expected_hostile_hp -= DMG_TO_HOSTILE_PER_HEX;
        expected_ship_hp -= DMG_TO_EACH_SHIP_PER_HEX;

        let hostile_hp = app.world().get::<HostileHitpoints>(hostile).unwrap().hp;
        assert_eq!(
            hostile_hp, expected_hostile_hp,
            "tick {}: hostile HP mismatch",
            tick,
        );
        for &s in &[ship_a, ship_b] {
            let hp = app.world().get::<ShipHitpoints>(s).unwrap().hull;
            assert_eq!(
                hp, expected_ship_hp,
                "tick {}: ship hull mismatch on {:?}",
                tick, s,
            );
        }
    }
    // After 4 ticks: hostile 100 - 80 = 20, ships 100 - 96 = 4
    assert_eq!(expected_hostile_hp, 20.0);
    assert_eq!(expected_ship_hp, 4.0);

    // --- Tick 5: hostile destroyed during weapon phase, no retaliation ---
    advance_time(&mut app, 1);
    assert!(
        app.world().get_entity(hostile).is_err(),
        "hostile must be despawned after weapon phase pushes HP to 0",
    );
    // Ship HP stays at 4.0 — hostile retaliation is gated behind the
    // destruction check and must NOT apply this tick.
    for &s in &[ship_a, ship_b] {
        let hp = app.world().get::<ShipHitpoints>(s).unwrap().hull;
        assert_eq!(
            hp, 4.0,
            "ship hull must be preserved on the kill tick (no retaliation)",
        );
    }
}

// =============================================================================
// Scenario 2 — 2-ship fleet vs 2 hostiles (space_creature + ancient_defense)
// =============================================================================

/// Two-ship fleet simultaneously engaging one space-creature and one
/// ancient-defense hostile, both Aggressive. Verifies:
///
/// - Both hostile factions' Neutral/-100 default relations engage.
/// - Within a single hexadies, the ships fight both hostiles. Per the
///   combat loop, hostile_A resolves first (damage out → retaliation) then
///   hostile_B (damage out → retaliation), so ships take retaliation damage
///   from BOTH hostiles when both survive the weapon phase.
/// - On the kill tick, neither surviving hostile retaliates.
#[test]
fn scenario_two_ship_fleet_vs_two_hostiles_symmetric() {
    // --- Balance (symmetric across hostiles) ---
    const HOSTILE_START_HP: f64 = 30.0;
    const HOSTILE_STRENGTH: f64 = 2.0;
    const SHIP_HULL: f64 = 200.0;

    // Derived (see module docstring):
    //   per-hex dmg to a hostile: 2 ships * 1 shot * 10 = 20
    //   per-hex dmg to each ship from ONE hostile: 2 * 12 / 2 = 12
    //   per-hex dmg to each ship from BOTH hostiles (when both alive): 24

    let mut app = test_app();
    install_scenario_weapon(&mut app);

    let sys = spawn_test_system(
        app.world_mut(),
        "Scenario-2",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let hostile_sc = common::spawn_raw_hostile(
        app.world_mut(),
        sys,
        HOSTILE_START_HP,
        HOSTILE_START_HP,
        HOSTILE_STRENGTH,
        0.0,
        "space_creature",
    );
    let hostile_ad = common::spawn_raw_hostile(
        app.world_mut(),
        sys,
        HOSTILE_START_HP,
        HOSTILE_START_HP,
        HOSTILE_STRENGTH,
        0.0,
        "ancient_defense",
    );

    let ship_a = spawn_scenario_ship(
        &mut app,
        "Alpha-2",
        sys,
        SHIP_HULL,
        RulesOfEngagement::Aggressive,
    );
    let ship_b = spawn_scenario_ship(
        &mut app,
        "Bravo-2",
        sys,
        SHIP_HULL,
        RulesOfEngagement::Aggressive,
    );

    // --- Verify relation setup for BOTH hostile factions ---
    advance_time(&mut app, 0);
    let empire = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerEmpire>>();
        q.single(app.world()).expect("empire")
    };
    let hf = *app.world().resource::<HostileFactions>();
    for (label, f) in [
        ("space_creature", hf.space_creature.unwrap()),
        ("ancient_defense", hf.ancient_defense.unwrap()),
    ] {
        let view: FactionView = app
            .world()
            .resource::<FactionRelations>()
            .get_or_default(empire, f);
        assert!(
            view.can_attack_aggressive(),
            "{}: Neutral/-100 must enable Aggressive engagement",
            label,
        );
    }

    // --- Tick 1: both hostiles alive → both retaliate ---
    advance_time(&mut app, 1);
    // Hostile_sc: 30 - 20 = 10 (survives, retaliates)
    // Hostile_ad: 30 - 20 = 10 (survives, retaliates)
    assert_eq!(
        app.world().get::<HostileHitpoints>(hostile_sc).unwrap().hp,
        10.0
    );
    assert_eq!(
        app.world().get::<HostileHitpoints>(hostile_ad).unwrap().hp,
        10.0
    );
    // Each ship absorbs 12 from sc + 12 from ad = 24; 200 - 24 = 176
    for &s in &[ship_a, ship_b] {
        let hp = app.world().get::<ShipHitpoints>(s).unwrap().hull;
        assert_eq!(hp, 176.0, "ship {:?} hull after tick 1", s);
    }

    // --- Tick 2: both hostiles drop to 0 in the weapon phase → no retaliation ---
    advance_time(&mut app, 1);
    assert!(
        app.world().get_entity(hostile_sc).is_err(),
        "space_creature must be destroyed",
    );
    assert!(
        app.world().get_entity(hostile_ad).is_err(),
        "ancient_defense must be destroyed",
    );
    // Ship HP unchanged — no retaliation from either hostile this tick.
    for &s in &[ship_a, ship_b] {
        let hp = app.world().get::<ShipHitpoints>(s).unwrap().hull;
        assert_eq!(
            hp, 176.0,
            "ship {:?} hull must be preserved on double-kill tick",
            s,
        );
    }
}

// =============================================================================
// Scenario 3 — Bug-injection / #308-style regression detector
// =============================================================================

/// Simulates the #308 regression by spawning a hostile *without* `FactionOwner`
/// and verifying the combat gate blocks engagement.
///
/// This test is an inversion of Scenario 1: identical balance and fleet
/// layout, but the hostile has no diplomatic identity, so
/// `resolve_combat` must skip it (`let Some(hostile_faction) = *hostile_faction
/// else { continue; };`).
///
/// Expected behavior: hostile HP stays at full after several hexadies; ships
/// also take zero damage because combat was never entered for this hostile.
/// If a future change accidentally reintroduces combat against
/// FactionOwner-less hostiles (or a spawn helper stops attaching FactionOwner
/// the way #308 did), this test fails.
#[test]
fn scenario_factionless_hostile_is_inert_regression_guard() {
    use macrocosmo::galaxy::{AtSystem, Hostile, HostileStats};

    let mut app = test_app();
    install_scenario_weapon(&mut app);

    let sys = spawn_test_system(
        app.world_mut(),
        "Scenario-3",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Ensure HostileFactions + FactionRelations are populated so the empire
    // *would* have attackable standing if a FactionOwner were present. We
    // explicitly spawn the raw hostile WITHOUT FactionOwner.
    let _ = common::setup_test_hostile_factions(app.world_mut());

    let hostile = app
        .world_mut()
        .spawn((
            AtSystem(sys),
            HostileHitpoints {
                hp: 100.0,
                max_hp: 100.0,
            },
            HostileStats {
                strength: 4.0,
                evasion: 0.0,
            },
            Hostile,
            // NOTE: intentionally NO FactionOwner — simulates #308 regression
        ))
        .id();

    let ship_a = spawn_scenario_ship(
        &mut app,
        "GuardAlpha",
        sys,
        100.0,
        RulesOfEngagement::Aggressive,
    );
    let ship_b = spawn_scenario_ship(
        &mut app,
        "GuardBravo",
        sys,
        100.0,
        RulesOfEngagement::Aggressive,
    );

    // Note: advance_time's auto-migration detects the FactionOwner-less
    // hostile and calls setup_test_hostile_factions — but that only attaches
    // FactionOwner to hostiles at *spawn* time via spawn_raw_hostile; a bare
    // Hostile spawned here stays unowned, exactly like the 20 sites in #308.
    for _ in 0..5 {
        advance_time(&mut app, 1);
    }

    // Hostile must be inert — full HP, still alive.
    assert!(
        app.world().get_entity(hostile).is_ok(),
        "FactionOwner-less hostile must not be destroyed",
    );
    assert_eq!(
        app.world().get::<HostileHitpoints>(hostile).unwrap().hp,
        100.0,
        "FactionOwner-less hostile must take zero damage (combat gate)",
    );
    // Ships must take zero damage — hostile did not retaliate.
    for &s in &[ship_a, ship_b] {
        let hp = app.world().get::<ShipHitpoints>(s).unwrap().hull;
        assert_eq!(
            hp, 100.0,
            "ship {:?} must be untouched — combat was never entered",
            s,
        );
    }
}

// =============================================================================
// Scenario — Ship-vs-ship combat emits standing evidence to AI bus
// =============================================================================

/// Two factions at war with ships in the same system. After combat runs,
/// the AI bus must contain `direct_attack`, `hostile_engagement`, and
/// `fleet_loss` evidence entries. Verifies the standing/threat pipeline
/// receives evidence from combat events.
#[test]
fn ship_vs_ship_combat_emits_standing_evidence() {
    // Use low hull so ships get destroyed quickly.
    const SHIP_HULL: f64 = 5.0;

    let mut app = test_app();
    install_scenario_weapon(&mut app);

    let sys = spawn_test_system(
        app.world_mut(),
        "Evidence-System",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Get the player faction entity.
    let player_faction = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(app.world()).next().expect("player empire must exist")
    };

    // Spawn a second faction.
    let enemy_faction = app
        .world_mut()
        .spawn(Faction::new("enemy_evidence_faction", "Enemy Evidence Faction"))
        .id();

    // Set both sides to War.
    {
        let mut rel = app.world_mut().resource_mut::<FactionRelations>();
        rel.set(
            player_faction,
            enemy_faction,
            FactionView::new(RelationState::War, -80.0),
        );
        rel.set(
            enemy_faction,
            player_faction,
            FactionView::new(RelationState::War, -80.0),
        );
    }

    // Spawn a player ship (owner = player faction).
    app.world_mut().spawn((
        Ship {
            name: "PlayerWarship".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule {
                slot_type: "weapon".to_string(),
                module_id: SCENARIO_WEAPON_ID.to_string(),
            }],
            owner: Owner::Empire(player_faction),
            sublight_speed: 0.75,
            ftl_range: 0.0,
            ruler_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::InSystem { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: SHIP_HULL,
            hull_max: SHIP_HULL,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
        RulesOfEngagement::Aggressive,
    ));

    // Spawn an enemy ship (owner = enemy faction).
    app.world_mut().spawn((
        Ship {
            name: "EnemyWarship".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule {
                slot_type: "weapon".to_string(),
                module_id: SCENARIO_WEAPON_ID.to_string(),
            }],
            owner: Owner::Empire(enemy_faction),
            sublight_speed: 0.75,
            ftl_range: 0.0,
            ruler_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::InSystem { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: SHIP_HULL,
            hull_max: SHIP_HULL,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
        RulesOfEngagement::Aggressive,
    ));

    // Run one tick of combat.
    advance_time(&mut app, 1);

    // --- Verify evidence was emitted ---
    let bus = app.world().resource::<AiBusResource>();
    let ai_player = to_ai_faction(player_faction);
    let ai_enemy = to_ai_faction(enemy_faction);

    // Player should see direct_attack evidence from the enemy.
    let player_evidence: Vec<_> = bus
        .evidence_for(ai_player, 100, 200)
        .filter(|e| e.target == ai_enemy)
        .collect();
    assert!(
        !player_evidence.is_empty(),
        "player faction should have evidence against enemy after combat"
    );

    // Check specific evidence kinds are present for the player.
    let has_direct_attack = player_evidence
        .iter()
        .any(|e| e.kind == evidence::direct_attack());
    let has_hostile_engagement = player_evidence
        .iter()
        .any(|e| e.kind == evidence::hostile_engagement());
    assert!(
        has_direct_attack,
        "player should observe direct_attack from enemy"
    );
    assert!(
        has_hostile_engagement,
        "player should observe hostile_engagement from enemy"
    );

    // Enemy should also see evidence from the player.
    let enemy_evidence: Vec<_> = bus
        .evidence_for(ai_enemy, 100, 200)
        .filter(|e| e.target == ai_player)
        .collect();
    assert!(
        !enemy_evidence.is_empty(),
        "enemy faction should have evidence against player after combat"
    );
    let enemy_has_direct_attack = enemy_evidence
        .iter()
        .any(|e| e.kind == evidence::direct_attack());
    let enemy_has_hostile_engagement = enemy_evidence
        .iter()
        .any(|e| e.kind == evidence::hostile_engagement());
    assert!(
        enemy_has_direct_attack,
        "enemy should observe direct_attack from player"
    );
    assert!(
        enemy_has_hostile_engagement,
        "enemy should observe hostile_engagement from player"
    );

    // With low hull (5.0) and weapon damage of 10.0, ships should be
    // destroyed. Check that fleet_loss evidence was emitted for at least
    // one side.
    let all_evidence: Vec<_> = bus
        .evidence_for(ai_player, 100, 200)
        .chain(bus.evidence_for(ai_enemy, 100, 200))
        .collect();
    let has_fleet_loss = all_evidence
        .iter()
        .any(|e| e.kind == evidence::fleet_loss());
    assert!(
        has_fleet_loss,
        "fleet_loss evidence should be emitted when ships are destroyed in combat"
    );
}
