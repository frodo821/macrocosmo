mod common;

use bevy::prelude::*;
use macrocosmo::galaxy::{Planet, StarSystem, StarTypeModifierSet, SystemModifiers};

#[test]
fn test_galaxy_generation_uses_types() {
    use macrocosmo::scripting::galaxy_api::{
        PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition,
        StarTypeRegistry,
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    // Insert registries with test data
    let mut star_reg = StarTypeRegistry::default();
    star_reg.types.push(StarTypeDefinition {
        id: "test_star".to_string(),
        name: "Test Star".to_string(),
        description: String::new(),
        color: [1.0, 1.0, 1.0],
        planet_lambda: 2.0,
        max_planets: 5,
        habitability_bonus: 0.0,
        weight: 1.0,
        modifiers: Vec::new(),
    });
    app.insert_resource(star_reg);

    let mut planet_reg = PlanetTypeRegistry::default();
    planet_reg.types.push(PlanetTypeDefinition {
        id: "test_planet".to_string(),
        name: "Test Planet".to_string(),
        description: String::new(),
        base_habitability: 0.7,
        base_slots: 4,
        resource_bias: ResourceBias {
            minerals: 1.0,
            energy: 1.0,
            research: 1.0,
        },
        weight: 1.0,
        default_biome: None,
    });
    app.insert_resource(planet_reg);

    // Run generate_galaxy as a one-shot system
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Verify all stars have star_type set
    let star_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    assert!(star_count > 0, "Should have generated star systems");

    for star in app.world_mut().query::<&StarSystem>().iter(app.world()) {
        assert_eq!(
            star.star_type, "test_star",
            "All stars should have star_type 'test_star'"
        );
    }

    // Verify all planets have planet_type set
    let planet_count = app
        .world_mut()
        .query::<&Planet>()
        .iter(app.world())
        .count();
    assert!(planet_count > 0, "Should have generated planets");

    for planet in app.world_mut().query::<&Planet>().iter(app.world()) {
        assert_eq!(
            planet.planet_type, "test_planet",
            "All planets should have planet_type 'test_planet'"
        );
    }
}

#[test]
fn test_system_modifiers_on_star_systems() {
    use macrocosmo::scripting::galaxy_api::{
        PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition,
        StarTypeRegistry,
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    let mut star_reg = StarTypeRegistry::default();
    star_reg.types.push(StarTypeDefinition {
        id: "test_star".to_string(),
        name: "Test Star".to_string(),
        description: String::new(),
        color: [1.0, 1.0, 1.0],
        planet_lambda: 2.0,
        max_planets: 3,
        habitability_bonus: 0.0,
        weight: 1.0,
        modifiers: Vec::new(),
    });
    app.insert_resource(star_reg);

    let mut planet_reg = PlanetTypeRegistry::default();
    planet_reg.types.push(PlanetTypeDefinition {
        id: "test_planet".to_string(),
        name: "Test Planet".to_string(),
        description: String::new(),
        base_habitability: 0.7,
        base_slots: 4,
        resource_bias: ResourceBias {
            minerals: 1.0,
            energy: 1.0,
            research: 1.0,
        },
        weight: 1.0,
        default_biome: None,
    });
    app.insert_resource(planet_reg);

    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Every star system should have a SystemModifiers component
    let star_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    assert!(star_count > 0, "Should have generated star systems");

    let modifiers_count = app
        .world_mut()
        .query::<(&StarSystem, &SystemModifiers)>()
        .iter(app.world())
        .count();
    assert_eq!(
        star_count, modifiers_count,
        "Every star system should have a SystemModifiers component"
    );
}
