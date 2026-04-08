use bevy::prelude::*;
use rand::Rng;

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
    /// Position in light-years (x, y, z)
    pub position: [f64; 3],
    /// Whether this system has been surveyed (precise data available)
    pub surveyed: bool,
    /// Whether this system is colonized
    pub colonized: bool,
    /// Whether this system is the capital
    pub is_capital: bool,
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

pub fn generate_galaxy(mut commands: Commands) {
    let mut rng = rand::rng();
    let num_systems = 50;

    // Generate star positions in a disk-like distribution
    // Radius ~100 light-years, thin disk
    let mut systems: Vec<(String, [f64; 3])> = Vec::new();
    for i in 0..num_systems {
        let r = rng.random_range(1.0_f64..100.0).sqrt() * 10.0; // sqrt for uniform disk distribution
        let theta = rng.random_range(0.0..std::f64::consts::TAU);
        let z = rng.random_range(-2.0_f64..2.0); // Thin disk

        let x = r * theta.cos();
        let y = r * theta.sin();
        let name = format!("System-{:03}", i);

        systems.push((name, [x, y, z]));
    }

    // Determine which systems are obscured by gas (simplified: random 15%)
    let gas_indices: Vec<usize> = (0..num_systems)
        .filter(|_| rng.random_range(0.0_f32..1.0) < 0.15)
        .collect();

    // First system is the capital (player start)
    for (i, (name, position)) in systems.iter().enumerate() {
        let is_capital = i == 0;
        let star = StarSystem {
            name: name.clone(),
            position: *position,
            surveyed: is_capital, // Only capital is surveyed at start
            colonized: is_capital,
            is_capital,
        };

        let entity = commands.spawn(star);
        let entity_id = entity.id();

        if gas_indices.contains(&i) && !is_capital {
            commands.entity(entity_id).insert(ObscuredByGas);
        }
    }

    info!("Galaxy generated: {} star systems", num_systems);
}
