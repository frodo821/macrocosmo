//! #223: End-to-end integration tests for the deliverable placement pipeline.
//!
//! Covers:
//!   - Direct deliverable flow: build at shipyard → stockpile → ship cargo →
//!     deploy at coordinates → DeepSpaceStructure entity spawned.
//!   - Platform upgrade flow: deploy ConstructionPlatform kit → ship pours
//!     resources via TransferToStructure → threshold crossed → capabilities
//!     active on upgraded structure.
//!   - Scrapyard / dismantle flow: dismantle active structure → Scrapyard
//!     present with refund pool → LoadFromScrapyard drains resources →
//!     entity despawns when empty.

#![cfg(test)]
#![allow(clippy::missing_docs_in_private_items)]

use std::collections::HashMap;

use bevy::prelude::*;

use macrocosmo::amount::Amt;
use macrocosmo::colony::DeliverableStockpile;
use macrocosmo::components::Position;
use macrocosmo::deep_space::{
    CapabilityParams, ConstructionPlatform, DeepSpaceStructure, DeliverableMetadata, LifetimeCost,
    ResourceCost, Scrapyard, StructureDefinition, StructureRegistry, UpgradeEdge,
};
use macrocosmo::ship::{
    Cargo, CargoItem, CommandQueue, Owner, QueuedCommand, ShipModifiers, ShipState,
    deliverable_ops::dismantle_structure,
};

mod common;

/// Register a sensor_buoy (direct deliverable) and a defense_platform_kit →
/// defense_platform (upgrade edge) in the StructureRegistry.
fn install_test_deliverables(app: &mut App) {
    let mut registry = app
        .world_mut()
        .get_resource_mut::<StructureRegistry>()
        .expect("StructureRegistry not initialized in test_app");
    // Direct deliverable with a live capability.
    registry.insert(StructureDefinition {
        id: "sensor_buoy".into(),
        name: "Sensor Buoy".into(),
        description: String::new(),
        max_hp: 20.0,
        capabilities: HashMap::from([(
            "detect_sublight".to_string(),
            CapabilityParams { range: 3.0 },
        )]),
        energy_drain: Amt::ZERO,
        prerequisites: None,
        deliverable: Some(DeliverableMetadata {
            cost: ResourceCost {
                minerals: Amt::units(50),
                energy: Amt::units(30),
            },
            build_time: 15,
            cargo_size: 1,
            scrap_refund: 0.5,
            spawns_as_ship: None,
        }),
        upgrade_to: Vec::new(),
        upgrade_from: None,
        on_built: None,
        on_upgraded: None,
    });
    // Platform kit with a single upgrade edge.
    registry.insert(StructureDefinition {
        id: "defense_platform_kit".into(),
        name: "Defense Platform Kit".into(),
        description: String::new(),
        max_hp: 80.0,
        capabilities: HashMap::from([(
            "construction_platform".to_string(),
            CapabilityParams::default(),
        )]),
        energy_drain: Amt::ZERO,
        prerequisites: None,
        deliverable: Some(DeliverableMetadata {
            cost: ResourceCost {
                minerals: Amt::units(200),
                energy: Amt::units(100),
            },
            build_time: 20,
            cargo_size: 3,
            scrap_refund: 0.3,
            spawns_as_ship: None,
        }),
        upgrade_to: vec![UpgradeEdge {
            target_id: "defense_platform".into(),
            cost: ResourceCost {
                minerals: Amt::units(500),
                energy: Amt::units(200),
            },
            build_time: 60,
        }],
        upgrade_from: None,
        on_built: None,
        on_upgraded: None,
    });
    // Upgrade target — finished defense platform.
    registry.insert(StructureDefinition {
        id: "defense_platform".into(),
        name: "Defense Platform".into(),
        description: String::new(),
        max_hp: 200.0,
        capabilities: HashMap::from([(
            "detect_sublight".to_string(),
            CapabilityParams { range: 10.0 },
        )]),
        energy_drain: Amt::ZERO,
        prerequisites: None,
        deliverable: None, // upgrade-only
        upgrade_to: Vec::new(),
        upgrade_from: None,
        on_built: None,
        on_upgraded: None,
    });
    // Rebuild the effective-edges cache.
    registry.rebuild_effective_edges();
}

/// Give a test ship enough cargo capacity to hold one item (via ShipModifiers).
fn grant_cargo_capacity(app: &mut App, ship: Entity, capacity_units: u64) {
    let mut mods = app
        .world_mut()
        .get_mut::<ShipModifiers>(ship)
        .expect("ship must have ShipModifiers");
    mods.cargo_capacity.set_base(Amt::units(capacity_units));
}

#[test]
fn test_deliverable_full_pipeline_direct() {
    let mut app = common::test_app();
    install_test_deliverables(&mut app);

    let system =
        common::spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, true);

    // Place the ship directly at the system's position (docked).
    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Hauler-1",
        "courier_mk1",
        system,
        [0.0, 0.0, 0.0],
    );
    // Run one tick so sync_ship_module_modifiers fires on the newly-spawned
    // ship (its Changed<Ship> filter will reset cargo_capacity). Then grant
    // capacity after the reset.
    app.update();
    grant_cargo_capacity(&mut app, ship, 10);

    // Seed the stockpile with a pre-built sensor buoy.
    app.world_mut()
        .entity_mut(system)
        .insert(DeliverableStockpile {
            items: vec![CargoItem::Deliverable {
                definition_id: "sensor_buoy".into(),
            }],
        });

    // 1) Load command
    {
        let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
        q.commands.push(QueuedCommand::LoadDeliverable {
            system,
            stockpile_index: 0,
        });
    }
    common::advance_time(&mut app, 1);

    // After load: cargo has the item, stockpile empty.
    assert!(
        matches!(
            app.world().get::<Cargo>(ship).unwrap().items.first(),
            Some(CargoItem::Deliverable { definition_id }) if definition_id == "sensor_buoy"
        ),
        "cargo should have sensor_buoy after load"
    );
    let stockpile_empty = app
        .world()
        .get::<DeliverableStockpile>(system)
        .map(|s| s.items.is_empty())
        .unwrap_or(false);
    assert!(
        stockpile_empty,
        "system stockpile should be empty after load"
    );

    // 2) Deploy command at (5,0,0)
    let target = [5.0, 0.0, 0.0];
    {
        let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
        q.commands.push(QueuedCommand::DeployDeliverable {
            position: target,
            item_index: 0,
        });
    }

    // Run several ticks to allow the auto-injected MoveToCoordinates to travel.
    for _ in 0..200 {
        common::advance_time(&mut app, 10);
        // Check for deployment.
        let spawned = app
            .world_mut()
            .query::<(&DeepSpaceStructure, &Position)>()
            .iter(app.world())
            .any(|(ds, pos)| ds.definition_id == "sensor_buoy" && (pos.x - target[0]).abs() < 0.1);
        if spawned {
            break;
        }
    }

    // Deploy should have spawned a DeepSpaceStructure at target.
    let spawned_at: Vec<[f64; 3]> = app
        .world_mut()
        .query::<(&DeepSpaceStructure, &Position)>()
        .iter(app.world())
        .filter(|(ds, _)| ds.definition_id == "sensor_buoy")
        .map(|(_, p)| p.as_array())
        .collect();
    assert!(
        spawned_at.iter().any(|p| (p[0] - target[0]).abs() < 0.1
            && (p[1] - target[1]).abs() < 0.1
            && (p[2] - target[2]).abs() < 0.1),
        "expected a sensor_buoy at {:?}, got {:?}",
        target,
        spawned_at
    );

    // Cargo item consumed.
    let cargo = app.world().get::<Cargo>(ship).unwrap();
    assert!(
        cargo.items.is_empty(),
        "ship cargo should be empty after deploy, got {:?}",
        cargo.items
    );
}

#[test]
fn test_deliverable_full_pipeline_platform() {
    let mut app = common::test_app();
    install_test_deliverables(&mut app);

    let system =
        common::spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Hauler-1",
        "courier_mk1",
        system,
        [0.0, 0.0, 0.0],
    );
    // Tick once so sync clears, then set cap (large enough for kit + resources).
    app.update();
    grant_cargo_capacity(&mut app, ship, 10_000);

    // Give the ship plenty of bulk resources for Transfer.
    {
        let mut cargo = app.world_mut().get_mut::<Cargo>(ship).unwrap();
        cargo.minerals = Amt::units(1000);
        cargo.energy = Amt::units(1000);
    }

    // Deploy a platform_kit.
    app.world_mut()
        .entity_mut(system)
        .insert(DeliverableStockpile {
            items: vec![CargoItem::Deliverable {
                definition_id: "defense_platform_kit".into(),
            }],
        });
    {
        let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
        q.commands.push(QueuedCommand::LoadDeliverable {
            system,
            stockpile_index: 0,
        });
    }
    common::advance_time(&mut app, 1);

    let deploy_pos = [3.0, 0.0, 0.0];
    {
        let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
        q.commands.push(QueuedCommand::DeployDeliverable {
            position: deploy_pos,
            item_index: 0,
        });
    }
    // Let the ship travel + deploy.
    for _ in 0..500 {
        common::advance_time(&mut app, 1);
        let deployed = app
            .world_mut()
            .query::<(&DeepSpaceStructure, &ConstructionPlatform)>()
            .iter(app.world())
            .any(|(ds, _)| ds.definition_id == "defense_platform_kit");
        if deployed {
            break;
        }
    }

    // Find the platform entity.
    let platform_entity: Entity = app
        .world_mut()
        .query_filtered::<Entity, With<ConstructionPlatform>>()
        .iter(app.world())
        .next()
        .expect("platform entity should exist after deployment");

    // Ship pours resources via TransferToStructure (multiple events).
    for _ in 0..3 {
        let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
        q.commands.push(QueuedCommand::TransferToStructure {
            structure: platform_entity,
            minerals: Amt::units(200),
            energy: Amt::units(100),
        });
    }
    // Run ticks; each transfer is processed one at a time.
    for _ in 0..20 {
        common::advance_time(&mut app, 1);
        // Stop once the platform has been upgraded (ConstructionPlatform gone).
        if app
            .world()
            .get::<ConstructionPlatform>(platform_entity)
            .is_none()
        {
            break;
        }
    }

    // The platform should now be upgraded to defense_platform.
    let ds = app
        .world()
        .get::<DeepSpaceStructure>(platform_entity)
        .expect("structure entity still present after upgrade");
    assert_eq!(
        ds.definition_id, "defense_platform",
        "definition should have flipped to the upgrade target"
    );
    assert!(
        app.world()
            .get::<ConstructionPlatform>(platform_entity)
            .is_none(),
        "ConstructionPlatform component should be removed after upgrade"
    );

    // Lifetime cost should now equal initial kit cost + upgrade edge cost.
    let lifetime = app
        .world()
        .get::<LifetimeCost>(platform_entity)
        .expect("LifetimeCost should be present");
    assert_eq!(
        lifetime.0.minerals,
        Amt::units(200 + 500),
        "lifetime minerals = kit + upgrade edge"
    );
    assert_eq!(
        lifetime.0.energy,
        Amt::units(100 + 200),
        "lifetime energy = kit + upgrade edge"
    );
}

#[test]
fn test_dismantle_and_scrapyard_drain() {
    let mut app = common::test_app();
    install_test_deliverables(&mut app);

    let system =
        common::spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, true);

    // Spawn an active sensor_buoy directly.
    let buoy_pos = [2.0, 0.0, 0.0];
    let buoy = app
        .world_mut()
        .spawn((
            DeepSpaceStructure {
                definition_id: "sensor_buoy".into(),
                name: "Buoy A".into(),
                owner: Owner::Neutral,
            },
            Position::from(buoy_pos),
            macrocosmo::deep_space::StructureHitpoints {
                current: 20.0,
                max: 20.0,
            },
            LifetimeCost(ResourceCost {
                minerals: Amt::units(50),
                energy: Amt::units(30),
            }),
        ))
        .id();

    // Spawn a ship adjacent to the buoy.
    let ship = common::spawn_test_ship(app.world_mut(), "Tug", "courier_mk1", system, buoy_pos);
    // Tick once so Changed<Ship> fires the sync, then set cap.
    app.update();
    grant_cargo_capacity(&mut app, ship, 100);
    // Transition the ship to Loitering at buoy's position.
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() =
        ShipState::Loitering { position: buoy_pos };

    // Dismantle via world API.
    app.world_mut().commands().queue(move |world: &mut World| {
        dismantle_structure(world, buoy).expect("dismantle should succeed");
    });
    app.update();

    // Structure should now have a Scrapyard with 50 % refund.
    let scrap = app
        .world()
        .get::<Scrapyard>(buoy)
        .expect("Scrapyard component should be present");
    assert_eq!(scrap.remaining.minerals, Amt::units(25));
    assert_eq!(scrap.remaining.energy, Amt::units(15));

    // LoadFromScrapyard drains into ship.
    {
        let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
        q.commands
            .push(QueuedCommand::LoadFromScrapyard { structure: buoy });
    }
    common::advance_time(&mut app, 1);

    let cargo = app.world().get::<Cargo>(ship).unwrap();
    assert!(
        cargo.minerals > Amt::ZERO || cargo.energy > Amt::ZERO,
        "ship cargo should have drained resources"
    );

    // Second tick: Scrapyard becomes empty and entity despawns.
    // Trigger another load to be sure.
    if app
        .world()
        .get::<Scrapyard>(buoy)
        .map(|s| !s.remaining.is_zero())
        .unwrap_or(false)
    {
        let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
        q.commands
            .push(QueuedCommand::LoadFromScrapyard { structure: buoy });
        common::advance_time(&mut app, 1);
    }
    common::advance_time(&mut app, 1); // tick_scrapyard_despawn will despawn next frame.

    let still_exists = app.world().get_entity(buoy).is_ok();
    assert!(
        !still_exists,
        "scrapyard entity should be despawned after full drain"
    );
}
