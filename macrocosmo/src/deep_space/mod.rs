//! # Deep-space structures + deliverable placement (#223)
//!
//! This module hosts the data model and systems for entities that live at
//! arbitrary galactic coordinates outside star systems:
//!
//! * `DeepSpaceStructure` — the minimal marker component for any such entity.
//! * `DeliverableDefinition` (aliased as `StructureDefinition`) — Lua-loaded
//!   schema describing capabilities, HP, prerequisites, optional upgrade
//!   graph (`upgrade_to` / `upgrade_from`), and the optional
//!   `DeliverableMetadata` that marks a definition as shipyard-buildable.
//! * `DeliverableRegistry` — one-per-App resource holding every definition,
//!   plus a cached `effective_edges` map combining explicit `upgrade_to`
//!   edges with self-declared `upgrade_from` edges for forward-ref isolation.
//! * `ConstructionPlatform` / `Scrapyard` / `LifetimeCost` — runtime
//!   components that track transitional states:
//!     - `ConstructionPlatform` gates capabilities while the structure is
//!       still assembling; `TransferToStructure` from a ship fills
//!       `accumulated`, and `tick_platform_upgrade` swaps
//!       `definition_id → target_id` once a target's cost is covered.
//!     - `Scrapyard` is installed by `dismantle_structure` and holds a
//!       `remaining = lifetime_cost × scrap_refund` pool. A ship's
//!       `LoadFromScrapyard` command drains it; when empty,
//!       `tick_scrapyard_despawn` removes the entity.
//!     - `LifetimeCost` accumulates every cost invested so far (initial
//!       deploy cost + every upgrade cost), used for scrap refund scaling.
//!
//! The shipyard → cargo → deploy path lives in `src/colony/building_queue.rs`
//! (`tick_build_queue` dispatching on `BuildKind::Deliverable`) and
//! `src/ship/deliverable_ops.rs` (command processors).
use std::collections::HashMap;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::condition::Condition;
use crate::ship::Owner;

/// A structure placed at arbitrary galactic coordinates, not attached to any star system.
#[derive(Component, Clone, Debug)]
pub struct DeepSpaceStructure {
    pub definition_id: String,
    pub name: String,
    pub owner: Owner,
}

/// #119: Direction for an `FTLCommRelay` pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommDirection {
    /// Both endpoints relay knowledge to each other.
    Bidirectional,
    /// Only this relay sends to its partner (one-way broadcaster).
    OneWay,
}

/// #119: Pair-state for an `FTLCommRelay` structure.
///
/// Two Relay entities reference each other via `paired_with`, forming an FTL
/// information channel. `direction` is interpreted from the perspective of
/// THIS relay:
///   - `Bidirectional` — data flows both ways (this ↔ paired_with)
///   - `OneWay` — data flows only from THIS relay to `paired_with` (the paired
///     endpoint still holds its own `FTLCommRelay` component; if its
///     `direction` is `OneWay`, it's likewise a sender-only, not a receiver).
///
/// When the partner entity is despawned, the component becomes "unpaired" —
/// `verify_relay_pairings_system` clears the component (entity reverts to a
/// plain deep-space structure) so no stale propagation occurs.
#[derive(Component, Clone, Copy, Debug)]
pub struct FTLCommRelay {
    pub paired_with: Entity,
    pub direction: CommDirection,
}

impl FTLCommRelay {
    pub fn new(paired_with: Entity, direction: CommDirection) -> Self {
        Self { paired_with, direction }
    }
}

/// #119: Pair two `FTLCommRelay`-capable entities.
///
/// Both entities must already be spawned with `DeepSpaceStructure` whose
/// definition exposes the `ftl_comm_relay` capability. This helper inserts
/// or replaces the `FTLCommRelay` component(s):
///
///   - `Bidirectional`: both endpoints get a `Bidirectional` component →
///     each relay sends to the other.
///   - `OneWay` from `a` to `b`: only `a` gets a `OneWay` component pointing
///     at `b`. `b` has any existing `FTLCommRelay` REMOVED (it becomes a
///     pure receiver — it cannot send via this pair). If `b` was previously
///     paired to a different relay, that pairing is lost; callers that need
///     complex multi-hop topologies should assemble multiple dedicated pairs
///     (chains emerge by placing multiple relays with independent pairs).
pub fn pair_relay_command(
    world: &mut bevy::ecs::world::World,
    a: Entity,
    b: Entity,
    direction: CommDirection,
) -> Result<(), &'static str> {
    if a == b {
        return Err("cannot pair a relay with itself");
    }
    if world.get::<DeepSpaceStructure>(a).is_none()
        || world.get::<DeepSpaceStructure>(b).is_none()
    {
        return Err("both entities must be DeepSpaceStructure");
    }
    match direction {
        CommDirection::Bidirectional => {
            world
                .entity_mut(a)
                .insert(FTLCommRelay::new(b, CommDirection::Bidirectional));
            world
                .entity_mut(b)
                .insert(FTLCommRelay::new(a, CommDirection::Bidirectional));
        }
        CommDirection::OneWay => {
            world
                .entity_mut(a)
                .insert(FTLCommRelay::new(b, CommDirection::OneWay));
            // Receiver-only: strip any existing relay component on b so it
            // cannot send back. The `verify_relay_pairings_system` also
            // handles dangling refs, so this only needs to clear b's own
            // sender state.
            world.entity_mut(b).remove::<FTLCommRelay>();
        }
    }
    Ok(())
}

/// Hitpoints for a deep-space structure.
#[derive(Component, Clone, Debug)]
pub struct StructureHitpoints {
    pub current: f64,
    pub max: f64,
}

/// Resource cost for building a structure.
#[derive(Clone, Debug, Default)]
pub struct ResourceCost {
    pub minerals: Amt,
    pub energy: Amt,
}

/// Parameters for a named capability (e.g. detection range for sensors).
#[derive(Clone, Debug, Default)]
pub struct CapabilityParams {
    pub range: f64,
    // Extensible: add more fields as needed.
}

/// #223: An edge in the deliverable upgrade graph — a platform kit can be
/// upgraded to `target_id` by spending `cost` over `build_time` hexadies.
#[derive(Clone, Debug)]
pub struct UpgradeEdge {
    pub target_id: String,
    pub cost: ResourceCost,
    pub build_time: i64,
}

/// #223: Shipyard-specific metadata. Present only when the deliverable was
/// declared via `define_deliverable` (i.e. can be built directly at a shipyard
/// and transported in a ship's Cargo). World-only structures declared via
/// `define_structure` leave this as `None`.
#[derive(Clone, Debug)]
pub struct DeliverableMetadata {
    pub cost: ResourceCost,
    pub build_time: i64,
    pub cargo_size: u32,
    /// 0.0..=1.0 — fraction of lifetime_cost returned as Scrapyard resources on dismantle.
    pub scrap_refund: f32,
}

/// #223: Unified definition covering both world-side structures and
/// shipyard-buildable deliverables. The `deliverable` field distinguishes them:
///   - `Some(_)` — shipyard-buildable (via `define_deliverable`)
///   - `None`    — world-spawn only / upgrade output (via `define_structure`)
#[derive(Clone, Debug)]
pub struct DeliverableDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub max_hp: f64,
    pub energy_drain: Amt,
    pub capabilities: HashMap<String, CapabilityParams>,
    pub prerequisites: Option<Condition>,

    /// `Some(_)` when the deliverable is shipyard-buildable.
    pub deliverable: Option<DeliverableMetadata>,

    /// Outgoing upgrade edges: starting from this deliverable, it can become
    /// one of these targets by paying the edge cost.
    pub upgrade_to: Vec<UpgradeEdge>,

    /// Optional self-declared inbound edge: "this deliverable can be reached
    /// from `source` by paying `cost`." Preserves referential churn isolation
    /// (a new target may self-declare without touching the platform def).
    ///
    /// When set, `target_id` holds the upstream source id and the edge cost/
    /// build_time describe the upstream→this transition. Cf. `upgrade_to`
    /// which describes this→downstream transitions.
    pub upgrade_from: Option<UpgradeEdge>,
}

impl DeliverableDefinition {
    /// Convenience: the direct shipyard build cost if this deliverable can be
    /// built at a shipyard; otherwise `None` (upgrade-only output).
    pub fn shipyard_cost(&self) -> Option<&ResourceCost> {
        self.deliverable.as_ref().map(|m| &m.cost)
    }

    /// Convenience: shipyard build time in hexadies (`None` if not buildable).
    pub fn shipyard_build_time(&self) -> Option<i64> {
        self.deliverable.as_ref().map(|m| m.build_time)
    }

    /// Returns `true` if this deliverable carries the `construction_platform`
    /// capability marker, meaning on-deploy it enters a waiting state until
    /// an upgrade target is selected and resources are delivered.
    pub fn is_construction_platform(&self) -> bool {
        self.capabilities.contains_key("construction_platform")
    }
}

/// Back-compat alias: existing code continues to use `StructureDefinition`.
pub type StructureDefinition = DeliverableDefinition;

/// Registry of all deliverable/structure definitions.
///
/// `effective_edges[source_id]` holds the merged outbound upgrade edges for
/// `source_id`, built once during `load_structure_definitions` by combining
/// each definition's `upgrade_to[*]` with any other definition's self-declared
/// `upgrade_from` that names `source_id`. `upgrade_to` wins on conflict.
#[derive(Resource, Default)]
pub struct DeliverableRegistry {
    pub definitions: HashMap<String, DeliverableDefinition>,
    pub effective_edges: HashMap<String, Vec<UpgradeEdge>>,
}

/// Back-compat alias.
pub type StructureRegistry = DeliverableRegistry;

impl DeliverableRegistry {
    /// Look up a structure/deliverable definition by id.
    pub fn get(&self, id: &str) -> Option<&DeliverableDefinition> {
        self.definitions.get(id)
    }

    /// Insert a definition, replacing any existing one with the same id.
    /// NOTE: This does NOT automatically rebuild `effective_edges`; call
    /// `rebuild_effective_edges` after batch-inserts.
    pub fn insert(&mut self, def: DeliverableDefinition) {
        self.definitions.insert(def.id.clone(), def);
    }

    /// Outbound upgrade edges effective at runtime for `source_id`. Uses the
    /// cached `effective_edges` table if populated, otherwise falls back to
    /// the definition's own `upgrade_to` list (which excludes inverse
    /// `upgrade_from` self-declarations).
    pub fn outgoing_edges(&self, source_id: &str) -> &[UpgradeEdge] {
        if let Some(v) = self.effective_edges.get(source_id) {
            return v.as_slice();
        }
        self.definitions
            .get(source_id)
            .map(|d| d.upgrade_to.as_slice())
            .unwrap_or(&[])
    }

    /// Rebuild `effective_edges` by merging `upgrade_to[*]` with inverse
    /// `upgrade_from` self-declarations from every other definition. On
    /// conflict (same source→target declared twice), the `upgrade_to`
    /// declaration wins; a warning is logged at the caller level.
    ///
    /// Returns a list of non-fatal validation warnings as `(source, target)`
    /// tuples describing conflicts, so the caller can `log::warn!` them.
    pub fn rebuild_effective_edges(&mut self) -> Vec<(String, String)> {
        let mut merged: HashMap<String, Vec<UpgradeEdge>> = HashMap::new();
        let mut conflicts: Vec<(String, String)> = Vec::new();

        // First pass: collect all `upgrade_to` edges.
        for (source_id, def) in &self.definitions {
            for edge in &def.upgrade_to {
                merged
                    .entry(source_id.clone())
                    .or_default()
                    .push(edge.clone());
            }
        }

        // Second pass: inverse `upgrade_from` (self-declared inbound edge on target).
        for (target_id, def) in &self.definitions {
            if let Some(uf) = &def.upgrade_from {
                let source_id = uf.target_id.clone(); // for upgrade_from, target_id holds the SOURCE.
                let inverse_edge = UpgradeEdge {
                    target_id: target_id.clone(),
                    cost: uf.cost.clone(),
                    build_time: uf.build_time,
                };
                let entry = merged.entry(source_id.clone()).or_default();
                // Does an `upgrade_to` edge for the same target already exist?
                if entry.iter().any(|e| e.target_id == *target_id) {
                    // Conflict — `upgrade_to` wins, we skip the inverse edge.
                    conflicts.push((source_id, target_id.clone()));
                } else {
                    entry.push(inverse_edge);
                }
            }
        }

        self.effective_edges = merged;
        conflicts
    }
}

/// Default structure definitions used when Lua scripts are not available (e.g. in tests).
///
/// All three defaults are shipyard-buildable deliverables (they carry
/// `DeliverableMetadata`) so existing tests and fallback startup continue to
/// see the same practical behaviour as before #223.
pub fn default_structure_definitions() -> Vec<StructureDefinition> {
    use crate::condition::ConditionAtom;

    vec![
        StructureDefinition {
            id: "sensor_buoy".to_string(),
            name: "Sensor Buoy".to_string(),
            description: "Detects sublight vessel movements.".to_string(),
            max_hp: 20.0,
            capabilities: HashMap::from([(
                "detect_sublight".to_string(),
                CapabilityParams { range: 3.0 },
            )]),
            energy_drain: Amt::milli(100),
            prerequisites: None,
            deliverable: Some(DeliverableMetadata {
                cost: ResourceCost {
                    minerals: Amt::units(50),
                    energy: Amt::units(30),
                },
                build_time: 15,
                cargo_size: 1,
                scrap_refund: 0.5,
            }),
            upgrade_to: Vec::new(),
            upgrade_from: None,
        },
        StructureDefinition {
            id: "ftl_comm_relay".to_string(),
            name: "FTL Comm Relay".to_string(),
            description: "Enables faster-than-light communication across systems.".to_string(),
            max_hp: 50.0,
            capabilities: HashMap::from([(
                "ftl_comm_relay".to_string(),
                CapabilityParams { range: 5.0 },
            )]),
            energy_drain: Amt::milli(500),
            prerequisites: Some(Condition::Atom(ConditionAtom::has_tech(
                "ftl_communications",
            ))),
            deliverable: Some(DeliverableMetadata {
                cost: ResourceCost {
                    minerals: Amt::units(200),
                    energy: Amt::units(150),
                },
                build_time: 30,
                cargo_size: 2,
                scrap_refund: 0.4,
            }),
            upgrade_to: Vec::new(),
            upgrade_from: None,
        },
        StructureDefinition {
            id: "interdictor".to_string(),
            name: "Interdictor".to_string(),
            description: "Disrupts FTL travel within its interdiction range.".to_string(),
            max_hp: 80.0,
            capabilities: HashMap::from([(
                "ftl_interdiction".to_string(),
                CapabilityParams { range: 5.0 },
            )]),
            energy_drain: Amt::units(1),
            prerequisites: Some(Condition::Atom(ConditionAtom::has_tech(
                "ftl_interdiction_tech",
            ))),
            deliverable: Some(DeliverableMetadata {
                cost: ResourceCost {
                    minerals: Amt::units(300),
                    energy: Amt::units(200),
                },
                build_time: 45,
                cargo_size: 3,
                scrap_refund: 0.3,
            }),
            upgrade_to: Vec::new(),
            upgrade_from: None,
        },
    ]
}

/// #223: Tracks the total resource cost ever invested into a `DeepSpaceStructure`
/// entity. Starts at the deliverable's `cost` when deployed; is incremented by
/// `UpgradeEdge.cost` every time the structure is upgraded via a
/// `ConstructionPlatform`. When the structure is dismantled, the resulting
/// `Scrapyard.remaining = lifetime_cost * scrap_refund`.
#[derive(Component, Clone, Debug, Default)]
pub struct LifetimeCost(pub ResourceCost);

/// #223: Marker component on a freshly-deployed construction platform that is
/// awaiting upgrade resources. While this is present, the structure's
/// capabilities are gated OFF (it's still "under construction"). Once enough
/// resources accumulate, the structure upgrades to `target_id` and this
/// component is removed.
///
/// `target_id.is_none()` means the player hasn't yet chosen which upgrade edge
/// to pursue; transfers are refused until a target is selected. UI can default
/// `target_id` to the sole edge when `upgrade_to` has exactly one element.
#[derive(Component, Clone, Debug, Default)]
pub struct ConstructionPlatform {
    pub target_id: Option<String>,
    pub accumulated: ResourceCost,
}

/// #223: Marker component on a dismantled structure. While present, the
/// structure's capabilities are gated OFF. A co-located ship can drain the
/// `remaining` pool into its own Cargo via `QueuedCommand::LoadFromScrapyard`.
/// When `remaining.is_zero()`, the entity is despawned next tick by
/// `tick_scrapyard_despawn`.
#[derive(Component, Clone, Debug)]
pub struct Scrapyard {
    pub remaining: ResourceCost,
    pub original_definition_id: String,
}

impl ResourceCost {
    pub fn is_zero(&self) -> bool {
        self.minerals == Amt::ZERO && self.energy == Amt::ZERO
    }

    /// Saturating add of `other` into self (consumes `other`).
    pub fn add_assign_saturating(&mut self, other: &ResourceCost) {
        self.minerals = self.minerals.add(other.minerals);
        self.energy = self.energy.add(other.energy);
    }

    /// Returns true iff `self` covers (≥) every component of `other`.
    pub fn covers(&self, other: &ResourceCost) -> bool {
        self.minerals >= other.minerals && self.energy >= other.energy
    }

    /// Multiply by a scalar in 0..=1 (used for scrap refund).
    pub fn scale(&self, factor: f32) -> ResourceCost {
        let f = factor.clamp(0.0, 1.0) as f64;
        ResourceCost {
            minerals: Amt::from_f64(self.minerals.to_f64() * f),
            energy: Amt::from_f64(self.energy.to_f64() * f),
        }
    }
}

/// #223: Spawn a new `DeepSpaceStructure` entity at the given position with the
/// given owner, according to the definition identified by `definition_id`.
///
/// Behaviour:
///   - The entity always gets: `DeepSpaceStructure`, `Position`,
///     `StructureHitpoints` (at max_hp), `LifetimeCost` (initial = def's
///     shipyard cost if present, else zero).
///   - If the definition has the `construction_platform` capability marker,
///     a `ConstructionPlatform` component is added. The `target_id` defaults
///     to the single `upgrade_to` edge's target when there is exactly one;
///     otherwise it is `None` (awaiting player selection).
///   - Otherwise the entity is fully active: its capabilities fire on the
///     next tick.
pub fn spawn_deliverable_entity(
    commands: &mut Commands,
    definition_id: &str,
    position: [f64; 3],
    owner: Owner,
    registry: &StructureRegistry,
) -> Option<Entity> {
    let def = registry.get(definition_id)?;

    let initial_cost = def
        .deliverable
        .as_ref()
        .map(|m| m.cost.clone())
        .unwrap_or_default();

    let mut ent = commands.spawn((
        DeepSpaceStructure {
            definition_id: definition_id.to_string(),
            name: def.name.clone(),
            owner,
        },
        crate::components::Position::from(position),
        StructureHitpoints {
            current: def.max_hp,
            max: def.max_hp,
        },
        LifetimeCost(initial_cost),
    ));

    if def.is_construction_platform() {
        let target_id = if def.upgrade_to.len() == 1 {
            Some(def.upgrade_to[0].target_id.clone())
        } else {
            None
        };
        ent.insert(ConstructionPlatform {
            target_id,
            accumulated: ResourceCost::default(),
        });
    }

    Some(ent.id())
}

/// #223: Apply player-scheduled upgrades: for each `ConstructionPlatform` that
/// has `accumulated >= target.cost`, swap the definition_id to the target,
/// bump the `LifetimeCost`, and remove the `ConstructionPlatform`.
pub fn tick_platform_upgrade(
    mut commands: Commands,
    registry: Res<StructureRegistry>,
    clock: Res<crate::time_system::GameClock>,
    mut events: MessageWriter<crate::events::GameEvent>,
    mut platforms: Query<(
        Entity,
        &mut DeepSpaceStructure,
        &mut LifetimeCost,
        &mut ConstructionPlatform,
    )>,
) {
    for (entity, mut structure, mut lifetime, mut platform) in platforms.iter_mut() {
        let Some(target_id) = platform.target_id.clone() else {
            continue;
        };
        // Find the edge: from structure.definition_id → target_id.
        let edges = registry.outgoing_edges(&structure.definition_id);
        let Some(edge) = edges.iter().find(|e| e.target_id == target_id) else {
            continue;
        };

        if !platform.accumulated.covers(&edge.cost) {
            continue;
        }

        // Upgrade! Consume edge.cost from accumulated; the remainder stays so
        // a subsequent upgrade (rare today, but possible) is not penalised.
        platform.accumulated.minerals = platform.accumulated.minerals.sub(edge.cost.minerals);
        platform.accumulated.energy = platform.accumulated.energy.sub(edge.cost.energy);

        // Update the structure identity.
        if let Some(new_def) = registry.get(&target_id) {
            let old_name = structure.name.clone();
            structure.definition_id = target_id.clone();
            structure.name = new_def.name.clone();
            // Bump lifetime cost by the edge cost.
            lifetime.0.add_assign_saturating(&edge.cost);
            events.write(crate::events::GameEvent {
                timestamp: clock.elapsed,
                kind: crate::events::GameEventKind::ShipBuilt,
                description: format!(
                    "Platform upgraded: {} → {}",
                    old_name,
                    new_def.name,
                ),
                related_system: None,
            });
            info!(
                "Deep-space structure {:?} upgraded → {}",
                entity, new_def.name,
            );
        }

        // Remove the construction marker so capabilities fire next tick.
        commands.entity(entity).remove::<ConstructionPlatform>();
    }
}

/// #223: Despawn any `Scrapyard` whose resources have been drained by a ship.
pub fn tick_scrapyard_despawn(
    mut commands: Commands,
    scrapyards: Query<(Entity, &Scrapyard)>,
) {
    for (entity, scrap) in &scrapyards {
        if scrap.remaining.is_zero() {
            commands.entity(entity).despawn();
        }
    }
}

pub struct DeepSpacePlugin;

impl Plugin for DeepSpacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StructureRegistry>()
            .add_systems(
                Startup,
                load_structure_definitions.after(crate::scripting::load_all_scripts),
            )
            .add_systems(
                Update,
                (
                    sensor_buoy_detect_system,
                    verify_relay_pairings_system,
                    relay_knowledge_propagate_system,
                    tick_platform_upgrade,
                    tick_scrapyard_despawn,
                )
                    .after(crate::time_system::advance_game_time)
                    .after(crate::ship::sublight_movement_system)
                    .after(crate::ship::process_ftl_travel),
            );
    }
}

/// #118: Sensor Buoy detection.
///
/// Each `DeepSpaceStructure` whose definition exposes the `detect_sublight`
/// capability scans for sublight-traveling and idle (docked/surveying/etc.)
/// ships within `range` light-years of its own `Position`. FTL ships are
/// invisible to base sensor buoys (FTL wake detection is gated behind a
/// future tech, #120).
///
/// Detection events are pushed into the player empire's `KnowledgeStore`
/// as `ShipSnapshot`s with an `observed_at` timestamp delayed by
/// `distance(buoy → player) / c`. Existing snapshots with newer
/// `observed_at` are preserved by `KnowledgeStore::update_ship`. When a
/// courier with a `KnowledgeRelay` route visits the buoy's owner system
/// (#117), the relay delivers the same data faster than light — both
/// pathways coexist and the freshest observation wins.
pub fn sensor_buoy_detect_system(
    clock: Res<crate::time_system::GameClock>,
    registry: Res<StructureRegistry>,
    structures: Query<
        (&DeepSpaceStructure, &crate::components::Position),
        (Without<ConstructionPlatform>, Without<Scrapyard>),
    >,
    ships: Query<(
        Entity,
        &crate::ship::Ship,
        &crate::ship::ShipState,
        &crate::components::Position,
        &crate::ship::ShipHitpoints,
    )>,
    player_q: Query<&crate::player::StationedAt, With<crate::player::Player>>,
    positions: Query<&crate::components::Position>,
    mut empire_q: Query<&mut crate::knowledge::KnowledgeStore, With<crate::player::PlayerEmpire>>,
) {
    use crate::knowledge::{ObservationSource, ShipSnapshot, ShipSnapshotState};

    let Ok(mut store) = empire_q.single_mut() else {
        return;
    };
    let Some(stationed) = player_q.iter().next() else {
        return;
    };
    let Ok(player_pos) = positions.get(stationed.system) else {
        return;
    };

    for (structure, buoy_pos) in &structures {
        let Some(def) = registry.get(&structure.definition_id) else {
            continue;
        };
        let Some(cap) = def.capabilities.get("detect_sublight") else {
            continue;
        };
        let detect_range = cap.range;
        if detect_range <= 0.0 {
            continue;
        }

        // Light-speed delay from buoy to player observer.
        let buoy_to_player = crate::physics::distance_ly(buoy_pos, player_pos);
        let delay = crate::physics::light_delay_hexadies(buoy_to_player);
        let observed_at = clock.elapsed - delay;
        if observed_at < 0 {
            continue;
        }

        for (ship_entity, ship, state, ship_pos, hp) in &ships {
            // FTL ships are invisible to baseline sensor buoys (#120 future).
            if matches!(state, crate::ship::ShipState::InFTL { .. }) {
                continue;
            }

            // Range check: distance from buoy to ship.
            let dist = crate::physics::distance_ly(buoy_pos, ship_pos);
            if dist > detect_range {
                continue;
            }

            // Skip if existing knowledge is at least as fresh.
            if store
                .get_ship(ship_entity)
                .is_some_and(|existing| existing.observed_at >= observed_at)
            {
                continue;
            }

            let (snapshot_state, last_system) = match state {
                crate::ship::ShipState::Docked { system } => {
                    (ShipSnapshotState::Docked, Some(*system))
                }
                crate::ship::ShipState::SubLight { target_system, .. } => {
                    (ShipSnapshotState::InTransit, *target_system)
                }
                crate::ship::ShipState::InFTL {
                    destination_system, ..
                } => (ShipSnapshotState::InTransit, Some(*destination_system)),
                crate::ship::ShipState::Surveying { target_system, .. } => {
                    (ShipSnapshotState::Surveying, Some(*target_system))
                }
                crate::ship::ShipState::Settling { system, .. } => {
                    (ShipSnapshotState::Settling, Some(*system))
                }
                crate::ship::ShipState::Refitting { system, .. } => {
                    (ShipSnapshotState::Refitting, Some(*system))
                }
                // #185: Loitering ship — encode position in snapshot state.
                crate::ship::ShipState::Loitering { position } => (
                    ShipSnapshotState::Loitering { position: *position },
                    None,
                ),
            };

            store.update_ship(ShipSnapshot {
                entity: ship_entity,
                name: ship.name.clone(),
                design_id: ship.design_id.clone(),
                last_known_state: snapshot_state,
                last_known_system: last_system,
                observed_at,
                hp: hp.hull,
                hp_max: hp.hull_max,
                source: ObservationSource::Direct,
            });
        }
    }
}

/// #119: Clear stale `FTLCommRelay` components whose `paired_with` entity has
/// been despawned (e.g. the partner relay was destroyed in combat).
///
/// This keeps the propagation system simple — it can assume every remaining
/// `FTLCommRelay` points to a live entity. The stripped entity remains a
/// `DeepSpaceStructure` but no longer relays knowledge until re-paired.
pub fn verify_relay_pairings_system(
    mut commands: Commands,
    relays: Query<(Entity, &FTLCommRelay)>,
    structures: Query<(), With<DeepSpaceStructure>>,
) {
    for (entity, relay) in &relays {
        if structures.get(relay.paired_with).is_err() {
            // Partner is gone — unpair this relay.
            commands.entity(entity).remove::<FTLCommRelay>();
        }
    }
}

/// #119: Relay ship knowledge between paired FTL Comm Relays at FTL speed
/// (effectively instant — `observed_at = clock.elapsed`).
///
/// For each paired relay `(source, target)`:
///   - If `source` has direction `OneWay`, it relays from source → target.
///   - If `source` has direction `Bidirectional`, it relays from source →
///     target (the target's own component will handle target → source on the
///     same tick because every relay iterates independently).
///
/// "Relaying" means: for every ship within the `source` relay's `range_ly`
/// (`ftl_comm_relay` capability range), IF the player is within the `target`
/// relay's `range_ly`, write a fresh `ShipSnapshot` into the player empire's
/// `KnowledgeStore` with `observed_at = clock.elapsed`. Existing snapshots
/// with newer `observed_at` are preserved by `KnowledgeStore::update_ship`
/// ("newer wins"), so chains (A→B→C) emerge naturally: if A relays to B and
/// B relays to C, and the player is in C's range, the player hears about
/// ships in A's range via the two independent pair runs.
///
/// If `range_ly == 0`, the relay's capability is considered infinite on that
/// side (matches the default Lua value until operators configure a real
/// range). Callers should set a sensible range in Lua.
pub fn relay_knowledge_propagate_system(
    clock: Res<crate::time_system::GameClock>,
    registry: Res<StructureRegistry>,
    // All relays, paired with their position + structure data.
    // #223: Relays gated behind a ConstructionPlatform or a Scrapyard do not
    // propagate knowledge — they're in a non-operational transitional state.
    relays: Query<
        (Entity, &DeepSpaceStructure, &crate::components::Position, &FTLCommRelay),
        (Without<ConstructionPlatform>, Without<Scrapyard>),
    >,
    // Any relay (incl. un-sending side) — used to look up partner position.
    relay_positions: Query<&crate::components::Position, With<DeepSpaceStructure>>,
    // Partner's structure definition for its range.
    partner_structures: Query<&DeepSpaceStructure>,
    ships: Query<(
        Entity,
        &crate::ship::Ship,
        &crate::ship::ShipState,
        &crate::components::Position,
        &crate::ship::ShipHitpoints,
    )>,
    player_q: Query<&crate::player::StationedAt, With<crate::player::Player>>,
    positions: Query<&crate::components::Position>,
    mut empire_q: Query<&mut crate::knowledge::KnowledgeStore, With<crate::player::PlayerEmpire>>,
    // #145: Forbidden regions that block FTL comm propagation.
    ftl_comm_blocking_regions: Query<&crate::galaxy::ForbiddenRegion>,
) {
    // Build region blockers (pairs segment check); only regions carrying the
    // `blocks_ftl_comm` capability matter here.
    let comm_blockers: Vec<crate::galaxy::RegionBlockSnapshot> = ftl_comm_blocking_regions
        .iter()
        .filter(|r| r.has_capability("blocks_ftl_comm"))
        .map(crate::galaxy::RegionBlockSnapshot::from_region)
        .collect();
    use crate::knowledge::{ObservationSource, ShipSnapshot, ShipSnapshotState};

    let Ok(mut store) = empire_q.single_mut() else {
        return;
    };
    let Some(stationed) = player_q.iter().next() else {
        return;
    };
    let Ok(player_pos) = positions.get(stationed.system) else {
        return;
    };

    // Helper: extract the ftl_comm_relay range for a given structure.
    let relay_range_for = |structure: &DeepSpaceStructure| -> Option<f64> {
        let def = registry.get(&structure.definition_id)?;
        let cap = def.capabilities.get("ftl_comm_relay")?;
        Some(cap.range)
    };

    for (_source_entity, source_structure, source_pos, relay) in &relays {
        // Every FTLCommRelay component SENDS from the holder to `paired_with`.
        // `direction` is informational for UI/future ROE; both variants send.
        // Strict OneWay semantics are enforced by `pair_relay_command` at
        // setup time (the receiver has its FTLCommRelay removed), so this
        // loop needs no special-casing beyond the component's presence.
        let _ = relay.direction;

        let Some(source_range) = relay_range_for(source_structure) else {
            continue;
        };

        let partner_entity = relay.paired_with;
        let Ok(partner_structure) = partner_structures.get(partner_entity) else {
            // Dangling pair — verify_relay_pairings_system will clean up.
            continue;
        };
        let Ok(partner_pos) = relay_positions.get(partner_entity) else {
            continue;
        };
        let Some(partner_range) = relay_range_for(partner_structure) else {
            continue;
        };

        // Check player-in-partner-range.
        let player_to_partner = crate::physics::distance_ly(player_pos, partner_pos);
        if partner_range > 0.0 && player_to_partner > partner_range {
            continue;
        }

        // #145: If any forbidden region with `blocks_ftl_comm` intersects the
        // pair segment, skip this pair entirely — knowledge falls back to
        // light-speed propagation (handled elsewhere, not here).
        let source_arr = source_pos.as_array();
        let partner_arr = partner_pos.as_array();
        if comm_blockers
            .iter()
            .any(|b| b.blocks_segment(source_arr, partner_arr))
        {
            continue;
        }

        // For each ship, check it's in the source relay's range and snapshot it.
        for (ship_entity, ship, state, ship_pos, hp) in &ships {
            let dist = crate::physics::distance_ly(source_pos, ship_pos);
            if source_range > 0.0 && dist > source_range {
                continue;
            }

            // FTL speed ~ instant: observed_at = clock.elapsed.
            let observed_at = clock.elapsed;

            // Skip if existing knowledge is at least as fresh.
            if store
                .get_ship(ship_entity)
                .is_some_and(|existing| existing.observed_at >= observed_at)
            {
                continue;
            }

            let (snapshot_state, last_system) = match state {
                crate::ship::ShipState::Docked { system } => {
                    (ShipSnapshotState::Docked, Some(*system))
                }
                crate::ship::ShipState::SubLight { target_system, .. } => {
                    (ShipSnapshotState::InTransit, *target_system)
                }
                crate::ship::ShipState::InFTL {
                    destination_system, ..
                } => (ShipSnapshotState::InTransit, Some(*destination_system)),
                crate::ship::ShipState::Surveying { target_system, .. } => {
                    (ShipSnapshotState::Surveying, Some(*target_system))
                }
                crate::ship::ShipState::Settling { system, .. } => {
                    (ShipSnapshotState::Settling, Some(*system))
                }
                crate::ship::ShipState::Refitting { system, .. } => {
                    (ShipSnapshotState::Refitting, Some(*system))
                }
                crate::ship::ShipState::Loitering { position } => (
                    ShipSnapshotState::Loitering { position: *position },
                    None,
                ),
            };

            store.update_ship(ShipSnapshot {
                entity: ship_entity,
                name: ship.name.clone(),
                design_id: ship.design_id.clone(),
                last_known_state: snapshot_state,
                last_known_system: last_system,
                observed_at,
                hp: hp.hull,
                hp_max: hp.hull_max,
                source: ObservationSource::Relay,
            });
        }
    }
}

/// #223: Validation outcome for a loaded deliverable registry.
#[derive(Debug, Default, Clone)]
pub struct DeliverableValidationReport {
    /// Warnings: conflicts or style issues; do NOT fail loading.
    pub warnings: Vec<String>,
    /// Errors: fatal — the offending definition should be rejected.
    pub errors: Vec<String>,
}

impl DeliverableValidationReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// #223: Run registry-wide validation on the deliverable graph per the 5 rules
/// from issue #223:
///
/// 1. Both-sides conflict warning: target T declares `upgrade_from` AND some X
///    references T in `upgrade_to[*]` — `upgrade_to` wins, emit a warning.
/// 2. Cost mismatch warning: the two declarations of the same edge disagree on
///    `cost` or `build_time` — warn; `upgrade_to` wins.
/// 3. Dangling reference (target or source id not in registry) — ERROR.
/// 4. Unreachable node (no shipyard cost AND no inbound edge) — ERROR.
/// 5. `construction_platform` capability without any outgoing edge (from its
///    own `upgrade_to` or any other definition's `upgrade_from`) — ERROR.
///
/// Also (re)builds `effective_edges` on the registry as a side effect so that
/// runtime callers can read merged outbound edges from a single table.
pub fn validate_and_build_edges(
    registry: &mut DeliverableRegistry,
) -> DeliverableValidationReport {
    let mut report = DeliverableValidationReport::default();

    let ids: std::collections::HashSet<String> = registry.definitions.keys().cloned().collect();

    // Rule 3: dangling refs (collected first so rule 2 can skip edges with
    // missing endpoints without misreporting them as "conflict").
    for (src_id, def) in &registry.definitions {
        for edge in &def.upgrade_to {
            if !ids.contains(&edge.target_id) {
                report.errors.push(format!(
                    "deliverable '{src_id}' upgrade_to references unknown target '{}'",
                    edge.target_id
                ));
            }
        }
        if let Some(uf) = &def.upgrade_from {
            if !ids.contains(&uf.target_id) {
                report.errors.push(format!(
                    "deliverable '{src_id}' upgrade_from references unknown source '{}'",
                    uf.target_id
                ));
            }
        }
    }

    // Rule 1 + 2: conflict detection between upgrade_to and inverse upgrade_from.
    // For every target T with `upgrade_from` set, look up the claimed source X.
    // If X also has an `upgrade_to` pointing to T, it's a style conflict; if
    // the cost or build_time disagree, append a cost-mismatch warning.
    for (target_id, t_def) in &registry.definitions {
        let Some(uf) = &t_def.upgrade_from else {
            continue;
        };
        let source_id = &uf.target_id;
        let Some(s_def) = registry.definitions.get(source_id) else {
            continue; // rule 3 already reported the dangling source
        };
        if let Some(to_edge) = s_def
            .upgrade_to
            .iter()
            .find(|e| e.target_id == *target_id)
        {
            report.warnings.push(format!(
                "upgrade edge '{source_id}' -> '{target_id}' declared on BOTH sides \
                 (upgrade_to and upgrade_from); using upgrade_to values"
            ));
            if to_edge.cost.minerals != uf.cost.minerals
                || to_edge.cost.energy != uf.cost.energy
                || to_edge.build_time != uf.build_time
            {
                report.warnings.push(format!(
                    "upgrade edge '{source_id}' -> '{target_id}' has mismatched cost/build_time \
                     between upgrade_to and upgrade_from; upgrade_to wins"
                ));
            }
        }
    }

    // Rule 4: unreachable nodes.
    // A deliverable is "reachable" if it has shipyard cost OR any inbound edge
    // (via someone's upgrade_to, or via its own upgrade_from).
    let mut inbound: std::collections::HashSet<String> = std::collections::HashSet::new();
    for def in registry.definitions.values() {
        for e in &def.upgrade_to {
            inbound.insert(e.target_id.clone());
        }
    }
    for (id, def) in &registry.definitions {
        let shipyard_buildable = def.deliverable.is_some();
        let has_upgrade_from = def.upgrade_from.is_some();
        let has_upstream = inbound.contains(id);
        if !shipyard_buildable && !has_upgrade_from && !has_upstream {
            report.errors.push(format!(
                "deliverable '{id}' is unreachable: has no shipyard cost and no inbound upgrade edge"
            ));
        }
    }

    // Rule 5: construction_platform capability requires >=1 outgoing edge.
    // Outgoing = own upgrade_to OR any other def's self-declared upgrade_from
    // pointing at us.
    let mut outbound_from: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for def in registry.definitions.values() {
        if !def.upgrade_to.is_empty() {
            outbound_from.insert(def.id.clone());
        }
    }
    for def in registry.definitions.values() {
        if let Some(uf) = &def.upgrade_from {
            outbound_from.insert(uf.target_id.clone());
        }
    }
    for (id, def) in &registry.definitions {
        if def.is_construction_platform() && !outbound_from.contains(id) {
            report.errors.push(format!(
                "deliverable '{id}' has construction_platform capability but no outgoing \
                 upgrade edge — it cannot be upgraded to anything"
            ));
        }
    }

    // Build `effective_edges` regardless; callers check `report.is_ok()`.
    let conflicts = registry.rebuild_effective_edges();
    for (s, t) in conflicts {
        // rebuild_effective_edges may also detect conflicts via both-sides
        // declaration; surface them here only if we haven't already above.
        let msg = format!(
            "upgrade edge '{s}' -> '{t}' declared on BOTH sides (upgrade_to and upgrade_from); using upgrade_to values"
        );
        if !report.warnings.iter().any(|w| w == &msg) {
            report.warnings.push(msg);
        }
    }

    report
}

/// Parse structure definitions from Lua accumulators, falling back to defaults.
/// Scripts are loaded by `load_all_scripts`; this system only parses the results.
pub fn load_structure_definitions(
    engine: Res<crate::scripting::ScriptEngine>,
    mut registry: ResMut<StructureRegistry>,
) {
    let insert_defaults = |registry: &mut StructureRegistry| {
        for def in default_structure_definitions() {
            registry.insert(def);
        }
    };

    match crate::scripting::structure_api::parse_structure_definitions(engine.lua()) {
        Ok(defs) if !defs.is_empty() => {
            let count = defs.len();
            for def in defs {
                registry.insert(def);
            }
            info!("Structure registry loaded with {} definitions", count);
        }
        Ok(_) => {
            info!("No structure definitions found in scripts; using defaults");
            insert_defaults(&mut registry);
        }
        Err(e) => {
            warn!("Failed to parse structure definitions: {e}; using defaults");
            insert_defaults(&mut registry);
        }
    }

    // #223: Validate the loaded graph and (re)build effective_edges.
    let report = validate_and_build_edges(&mut registry);
    for w in &report.warnings {
        warn!("Deliverable registry: {w}");
    }
    for e in &report.errors {
        error!("Deliverable registry: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Position;
    use crate::condition::ConditionAtom;

    #[test]
    fn test_default_structure_definitions() {
        let defs = default_structure_definitions();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].id, "sensor_buoy");
        assert_eq!(defs[1].id, "ftl_comm_relay");
        assert_eq!(defs[2].id, "interdictor");
    }

    #[test]
    fn test_structure_registry_lookup() {
        let mut registry = StructureRegistry::default();
        assert!(registry.get("sensor_buoy").is_none());

        for def in default_structure_definitions() {
            registry.insert(def);
        }

        let buoy = registry.get("sensor_buoy").unwrap();
        assert_eq!(buoy.name, "Sensor Buoy");
        assert_eq!(buoy.max_hp, 20.0);
        assert!(buoy.capabilities.contains_key("detect_sublight"));
        assert_eq!(buoy.capabilities["detect_sublight"].range, 3.0);

        let relay = registry.get("ftl_comm_relay").unwrap();
        assert_eq!(relay.name, "FTL Comm Relay");
        assert!(relay.capabilities.contains_key("ftl_comm_relay"));

        let interdictor = registry.get("interdictor").unwrap();
        assert_eq!(interdictor.name, "Interdictor");
        assert!(interdictor.capabilities.contains_key("ftl_interdiction"));
        assert_eq!(interdictor.capabilities["ftl_interdiction"].range, 5.0);
    }

    #[test]
    fn test_structure_registry_replace() {
        let mut registry = StructureRegistry::default();
        for def in default_structure_definitions() {
            registry.insert(def);
        }

        // Replace sensor_buoy with updated values
        registry.insert(StructureDefinition {
            id: "sensor_buoy".to_string(),
            name: "Advanced Sensor Buoy".to_string(),
            description: "Enhanced sensor buoy.".to_string(),
            max_hp: 40.0,
            capabilities: HashMap::from([
                ("detect_sublight".to_string(), CapabilityParams { range: 5.0 }),
                ("detect_ftl".to_string(), CapabilityParams { range: 3.0 }),
            ]),
            energy_drain: Amt::milli(200),
            prerequisites: Some(Condition::Atom(ConditionAtom::has_tech(
                "advanced_sensors",
            ))),
            deliverable: Some(DeliverableMetadata {
                cost: ResourceCost {
                    minerals: Amt::units(100),
                    energy: Amt::units(60),
                },
                build_time: 20,
                cargo_size: 1,
                scrap_refund: 0.5,
            }),
            upgrade_to: Vec::new(),
            upgrade_from: None,
        });

        assert_eq!(registry.definitions.len(), 3);
        let buoy = registry.get("sensor_buoy").unwrap();
        assert_eq!(buoy.name, "Advanced Sensor Buoy");
        assert_eq!(buoy.max_hp, 40.0);
        assert_eq!(buoy.capabilities["detect_sublight"].range, 5.0);
    }

    #[test]
    fn test_spawn_structure_entity() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let entity = app.world_mut().spawn((
            DeepSpaceStructure {
                definition_id: "sensor_buoy".to_string(),
                name: "Buoy Alpha".to_string(),
                owner: Owner::Neutral,
            },
            StructureHitpoints {
                current: 20.0,
                max: 20.0,
            },
            Position {
                x: 10.0,
                y: 5.0,
                z: 0.0,
            },
        )).id();

        let world = app.world();
        let dss = world.get::<DeepSpaceStructure>(entity).unwrap();
        assert_eq!(dss.definition_id, "sensor_buoy");
        assert_eq!(dss.name, "Buoy Alpha");

        let hp = world.get::<StructureHitpoints>(entity).unwrap();
        assert_eq!(hp.current, 20.0);
        assert_eq!(hp.max, 20.0);

        let pos = world.get::<Position>(entity).unwrap();
        assert!((pos.x - 10.0).abs() < f64::EPSILON);
        assert!((pos.y - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_pair_relay_command_bidirectional_sets_both_sides() {
        let mut world = bevy::ecs::world::World::new();
        let a = world
            .spawn(DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "A".into(),
                owner: Owner::Neutral,
            })
            .id();
        let b = world
            .spawn(DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "B".into(),
                owner: Owner::Neutral,
            })
            .id();

        pair_relay_command(&mut world, a, b, CommDirection::Bidirectional).unwrap();

        let ra = world.get::<FTLCommRelay>(a).expect("A has relay");
        assert_eq!(ra.paired_with, b);
        assert_eq!(ra.direction, CommDirection::Bidirectional);
        let rb = world.get::<FTLCommRelay>(b).expect("B has relay");
        assert_eq!(rb.paired_with, a);
        assert_eq!(rb.direction, CommDirection::Bidirectional);
    }

    #[test]
    fn test_pair_relay_command_oneway_strips_receiver() {
        let mut world = bevy::ecs::world::World::new();
        let a = world
            .spawn(DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "A".into(),
                owner: Owner::Neutral,
            })
            .id();
        let b = world
            .spawn(DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "B".into(),
                owner: Owner::Neutral,
            })
            .id();

        pair_relay_command(&mut world, a, b, CommDirection::OneWay).unwrap();

        let ra = world.get::<FTLCommRelay>(a).expect("A is the sender");
        assert_eq!(ra.paired_with, b);
        assert_eq!(ra.direction, CommDirection::OneWay);
        assert!(
            world.get::<FTLCommRelay>(b).is_none(),
            "OneWay receiver must not have an FTLCommRelay component"
        );
    }

    #[test]
    fn test_pair_relay_command_rejects_self_pair() {
        let mut world = bevy::ecs::world::World::new();
        let a = world
            .spawn(DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "A".into(),
                owner: Owner::Neutral,
            })
            .id();
        assert!(pair_relay_command(&mut world, a, a, CommDirection::Bidirectional).is_err());
    }

    #[test]
    fn test_pair_relay_command_requires_structure() {
        let mut world = bevy::ecs::world::World::new();
        let a = world
            .spawn(DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "A".into(),
                owner: Owner::Neutral,
            })
            .id();
        // B is not a DeepSpaceStructure
        let b = world.spawn_empty().id();
        assert!(pair_relay_command(&mut world, a, b, CommDirection::Bidirectional).is_err());
    }

    // --- #223: Validation tests ---

    fn mk_def(id: &str) -> StructureDefinition {
        StructureDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            max_hp: 10.0,
            capabilities: HashMap::new(),
            energy_drain: Amt::ZERO,
            prerequisites: None,
            deliverable: None,
            upgrade_to: Vec::new(),
            upgrade_from: None,
        }
    }

    fn mk_buildable(id: &str, minerals: u64) -> StructureDefinition {
        let mut def = mk_def(id);
        def.deliverable = Some(DeliverableMetadata {
            cost: ResourceCost {
                minerals: Amt::units(minerals),
                energy: Amt::ZERO,
            },
            build_time: 10,
            cargo_size: 1,
            scrap_refund: 0.5,
        });
        def
    }

    fn mk_platform(id: &str) -> StructureDefinition {
        let mut def = mk_buildable(id, 100);
        def.capabilities.insert(
            "construction_platform".to_string(),
            CapabilityParams::default(),
        );
        def
    }

    #[test]
    fn test_validate_dangling_target_errors() {
        let mut reg = DeliverableRegistry::default();
        let mut src = mk_buildable("kit", 100);
        src.upgrade_to.push(UpgradeEdge {
            target_id: "ghost".to_string(),
            cost: ResourceCost::default(),
            build_time: 10,
        });
        reg.insert(src);
        let report = validate_and_build_edges(&mut reg);
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| e.contains("ghost")));
    }

    #[test]
    fn test_validate_unreachable_errors() {
        let mut reg = DeliverableRegistry::default();
        // isolated: not buildable, no inbound edge.
        reg.insert(mk_def("orphan"));
        let report = validate_and_build_edges(&mut reg);
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| e.contains("orphan")));
    }

    #[test]
    fn test_validate_construction_platform_without_edges_errors() {
        let mut reg = DeliverableRegistry::default();
        reg.insert(mk_platform("platform"));
        let report = validate_and_build_edges(&mut reg);
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| e.contains("construction_platform")));
    }

    #[test]
    fn test_validate_upgrade_conflict_warning() {
        let mut reg = DeliverableRegistry::default();
        let mut kit = mk_platform("kit");
        kit.upgrade_to.push(UpgradeEdge {
            target_id: "active".to_string(),
            cost: ResourceCost {
                minerals: Amt::units(100),
                energy: Amt::ZERO,
            },
            build_time: 30,
        });
        reg.insert(kit);

        let mut active = mk_def("active");
        active.upgrade_from = Some(UpgradeEdge {
            target_id: "kit".to_string(),
            cost: ResourceCost {
                minerals: Amt::units(100),
                energy: Amt::ZERO,
            },
            build_time: 30,
        });
        reg.insert(active);

        let report = validate_and_build_edges(&mut reg);
        assert!(report.is_ok());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("declared on BOTH sides"))
        );
    }

    #[test]
    fn test_validate_cost_mismatch_warning() {
        let mut reg = DeliverableRegistry::default();
        let mut kit = mk_platform("kit");
        kit.upgrade_to.push(UpgradeEdge {
            target_id: "active".to_string(),
            cost: ResourceCost {
                minerals: Amt::units(100),
                energy: Amt::ZERO,
            },
            build_time: 30,
        });
        reg.insert(kit);

        let mut active = mk_def("active");
        active.upgrade_from = Some(UpgradeEdge {
            target_id: "kit".to_string(),
            cost: ResourceCost {
                minerals: Amt::units(200), // mismatch!
                energy: Amt::ZERO,
            },
            build_time: 30,
        });
        reg.insert(active);

        let report = validate_and_build_edges(&mut reg);
        assert!(report.is_ok());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("mismatched cost/build_time"))
        );
    }

    #[test]
    fn test_effective_edges_upgrade_to_wins_on_conflict() {
        let mut reg = DeliverableRegistry::default();
        let mut kit = mk_platform("kit");
        kit.upgrade_to.push(UpgradeEdge {
            target_id: "active".to_string(),
            cost: ResourceCost {
                minerals: Amt::units(100),
                energy: Amt::ZERO,
            },
            build_time: 30,
        });
        reg.insert(kit);

        let mut active = mk_def("active");
        active.upgrade_from = Some(UpgradeEdge {
            target_id: "kit".to_string(),
            cost: ResourceCost {
                minerals: Amt::units(999), // loser
                energy: Amt::ZERO,
            },
            build_time: 99,
        });
        reg.insert(active);

        let _ = validate_and_build_edges(&mut reg);
        let edges = reg.outgoing_edges("kit");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, "active");
        assert_eq!(edges[0].cost.minerals, Amt::units(100));
        assert_eq!(edges[0].build_time, 30);
    }

    #[test]
    fn test_effective_edges_inverse_upgrade_from_merged() {
        // If no `upgrade_to` declares the edge, the inverse `upgrade_from`
        // should be promoted into `effective_edges` for routing.
        let mut reg = DeliverableRegistry::default();
        reg.insert(mk_platform("kit"));
        let mut active = mk_def("active");
        active.upgrade_from = Some(UpgradeEdge {
            target_id: "kit".to_string(),
            cost: ResourceCost {
                minerals: Amt::units(500),
                energy: Amt::ZERO,
            },
            build_time: 42,
        });
        reg.insert(active);
        let _ = validate_and_build_edges(&mut reg);
        let edges = reg.outgoing_edges("kit");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, "active");
        assert_eq!(edges[0].cost.minerals, Amt::units(500));
        assert_eq!(edges[0].build_time, 42);
    }

    #[test]
    fn test_verify_relay_pairings_unpair_on_partner_despawn() {
        use crate::components::Position;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, verify_relay_pairings_system);

        let a = app.world_mut().spawn((
            DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "A".into(),
                owner: Owner::Neutral,
            },
            Position { x: 0.0, y: 0.0, z: 0.0 },
        )).id();
        let b = app.world_mut().spawn((
            DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "B".into(),
                owner: Owner::Neutral,
            },
            Position { x: 5.0, y: 0.0, z: 0.0 },
        )).id();

        pair_relay_command(app.world_mut(), a, b, CommDirection::Bidirectional).unwrap();

        // Despawn partner and run one frame.
        app.world_mut().despawn(b);
        app.update();

        // A's FTLCommRelay should have been removed.
        assert!(
            app.world().get::<FTLCommRelay>(a).is_none(),
            "A must become unpaired after its partner despawns"
        );
    }
}
