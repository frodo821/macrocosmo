use bevy::prelude::*;
use rand::Rng;

use crate::components::Position;
use crate::ship::Owner;

pub struct GalaxyPlugin;

impl Plugin for GalaxyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, generate_galaxy);
    }
}

/// A star system in the galaxy
#[derive(Component)]
pub struct StarSystem {
    pub name: String,
    /// Whether this system has been surveyed (precise data available)
    pub surveyed: bool,
    /// Whether this system is colonized
    pub colonized: bool,
    /// Whether this system is the capital
    pub is_capital: bool,
}

/// Physical and economic attributes of a star system.
#[derive(Component, Clone)]
pub struct SystemAttributes {
    pub habitability: Habitability,
    pub mineral_richness: ResourceLevel,
    pub energy_potential: ResourceLevel,
    pub research_potential: ResourceLevel,
    pub max_building_slots: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Habitability {
    Ideal,
    Adequate,
    Marginal,
    Barren,
    GasGiant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceLevel {
    Rich,
    Moderate,
    Poor,
    None,
}

/// Sovereignty status of a star system
#[derive(Component, Default)]
pub struct Sovereignty {
    pub owner: Option<Owner>,
    pub control_score: f64,
}

/// Marker for systems obscured by interstellar gas
#[derive(Component)]
pub struct ObscuredByGas;

/// Marker for systems that have port facilities
#[derive(Component)]
pub struct PortFacility {
    /// The other star system entity this port connects to
    pub partner: Entity,
}

fn random_habitability(rng: &mut impl Rng) -> Habitability {
    let roll: f32 = rng.random_range(0.0..1.0);
    if roll < 0.10 {
        Habitability::Ideal
    } else if roll < 0.35 {
        Habitability::Adequate
    } else if roll < 0.65 {
        Habitability::Marginal
    } else if roll < 0.90 {
        Habitability::Barren
    } else {
        Habitability::GasGiant
    }
}

fn random_resource_level(rng: &mut impl Rng) -> ResourceLevel {
    let roll: f32 = rng.random_range(0.0..1.0);
    if roll < 0.20 {
        ResourceLevel::Rich
    } else if roll < 0.55 {
        ResourceLevel::Moderate
    } else if roll < 0.80 {
        ResourceLevel::Poor
    } else {
        ResourceLevel::None
    }
}

fn building_slots_for(hab: Habitability, rng: &mut impl Rng) -> u8 {
    match hab {
        Habitability::Ideal => rng.random_range(5..=8),
        Habitability::Adequate => rng.random_range(3..=6),
        Habitability::Marginal => rng.random_range(2..=4),
        Habitability::Barren => rng.random_range(1..=2),
        Habitability::GasGiant => 0,
    }
}

fn random_attributes(rng: &mut impl Rng) -> SystemAttributes {
    let habitability = random_habitability(rng);
    SystemAttributes {
        habitability,
        mineral_richness: random_resource_level(rng),
        energy_potential: random_resource_level(rng),
        research_potential: random_resource_level(rng),
        max_building_slots: building_slots_for(habitability, rng),
    }
}

fn capital_attributes(rng: &mut impl Rng) -> SystemAttributes {
    SystemAttributes {
        habitability: Habitability::Ideal,
        mineral_richness: at_least_moderate(random_resource_level(rng)),
        energy_potential: at_least_moderate(random_resource_level(rng)),
        research_potential: at_least_moderate(random_resource_level(rng)),
        max_building_slots: building_slots_for(Habitability::Ideal, rng),
    }
}

fn at_least_moderate(level: ResourceLevel) -> ResourceLevel {
    match level {
        ResourceLevel::Poor | ResourceLevel::None => ResourceLevel::Moderate,
        other => other,
    }
}

fn is_habitable(h: Habitability) -> bool {
    !matches!(h, Habitability::GasGiant)
}

pub fn generate_galaxy(mut commands: Commands) {
    let mut rng = rand::rng();
    let num_systems = 50;

    let mut systems: Vec<(String, [f64; 3])> = Vec::new();
    for i in 0..num_systems {
        let r = rng.random_range(1.0_f64..100.0).sqrt() * 10.0;
        let theta = rng.random_range(0.0..std::f64::consts::TAU);
        let z = rng.random_range(-2.0_f64..2.0);

        let x = r * theta.cos();
        let y = r * theta.sin();
        let name = format!("System-{:03}", i);

        systems.push((name, [x, y, z]));
    }

    // Generate attributes
    let mut attributes: Vec<SystemAttributes> = Vec::with_capacity(num_systems);
    for i in 0..num_systems {
        if i == 0 {
            attributes.push(capital_attributes(&mut rng));
        } else {
            attributes.push(random_attributes(&mut rng));
        }
    }

    // Ensure at least one habitable neighbour near capital
    let capital_pos = systems[0].1;
    let mut neighbours: Vec<(usize, f64)> = (1..num_systems)
        .map(|i| {
            let p = systems[i].1;
            let dx = p[0] - capital_pos[0];
            let dy = p[1] - capital_pos[1];
            let dz = p[2] - capital_pos[2];
            (i, dx * dx + dy * dy + dz * dz)
        })
        .collect();
    neighbours.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let nearby: Vec<usize> = neighbours.iter().take(5).map(|(i, _)| *i).collect();

    if !nearby.iter().any(|&i| is_habitable(attributes[i].habitability)) {
        let closest = nearby[0];
        attributes[closest].habitability = Habitability::Adequate;
        attributes[closest].max_building_slots =
            building_slots_for(Habitability::Adequate, &mut rng);
    }

    // Gas obscured systems (15%)
    let gas_indices: Vec<usize> = (0..num_systems)
        .filter(|_| rng.random_range(0.0_f32..1.0) < 0.15)
        .collect();

    for (i, (name, position)) in systems.iter().enumerate() {
        let is_capital = i == 0;
        let star = StarSystem {
            name: name.clone(),
            surveyed: is_capital,
            colonized: is_capital,
            is_capital,
        };

        let sovereignty = if is_capital {
            Sovereignty {
                owner: Some(Owner::Player),
                control_score: 100.0,
            }
        } else {
            Sovereignty::default()
        };

        let entity = commands.spawn((star, Position::from(*position), attributes[i].clone(), sovereignty));
        let entity_id = entity.id();

        if gas_indices.contains(&i) && !is_capital {
            commands.entity(entity_id).insert(ObscuredByGas);
        }
    }

    info!("Galaxy generated: {} star systems", num_systems);
}
