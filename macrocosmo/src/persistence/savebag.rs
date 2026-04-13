//! Wire-format "saved component bag" for Phase A save/load (#247).
//!
//! Each live ECS component is mirrored by a `Saved*` wire struct which is
//! `Serialize + Deserialize`-able via postcard. Entity references are encoded
//! as `u64` save ids (via [`EntityMap`]) and translated back to live
//! `Entity`s on load via [`RemapEntities`].
//!
//! Phase A scope: only the core state required by the round-trip test.
//! Ship/colony/deep-space/knowledge extension types are explicitly deferred to
//! Phase B/C per issue #247.

use bevy::prelude::Entity;
use serde::{Deserialize, Serialize};

use crate::amount::Amt;
use crate::colony::{Colony, ResourceCapacity, ResourceStockpile};
use crate::components::{MovementState, Position};
use crate::faction::{FactionOwner, FactionView, RelationState};
use crate::galaxy::{
    HostilePresence, HostileType, Planet, PortFacility, Sovereignty, StarSystem, SystemAttributes,
};
use crate::player::{AboardShip, Empire, Faction, Player, StationedAt};
use crate::ship::{Cargo, CargoItem, Owner, Ship, ShipHitpoints, ShipState};

use super::remap::{EntityMap, RemapEntities};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Translate an encoded save-id `u64` back to a live `Entity`, falling back to
/// `Entity::PLACEHOLDER` if the id is unknown (corrupt save). Phase A uses a
/// best-effort strategy so a stray missing reference doesn't crash the game.
fn remap_entity(bits: u64, map: &EntityMap) -> Entity {
    map.entity(bits).unwrap_or(Entity::PLACEHOLDER)
}

// ---------------------------------------------------------------------------
// MovementState
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedMovementState {
    Docked { system_bits: u64 },
    SubLight {
        origin: Position,
        destination: Position,
        speed_fraction: f64,
        departed_at: i64,
    },
    FTL {
        destination_bits: u64,
        departed_at: i64,
        arrives_at: i64,
    },
}

impl SavedMovementState {
    pub fn from_live(v: &MovementState) -> Self {
        match v {
            MovementState::Docked { system } => Self::Docked {
                system_bits: system.to_bits(),
            },
            MovementState::SubLight {
                origin,
                destination,
                speed_fraction,
                departed_at,
            } => Self::SubLight {
                origin: *origin,
                destination: *destination,
                speed_fraction: *speed_fraction,
                departed_at: *departed_at,
            },
            MovementState::FTL {
                destination,
                departed_at,
                arrives_at,
            } => Self::FTL {
                destination_bits: destination.to_bits(),
                departed_at: *departed_at,
                arrives_at: *arrives_at,
            },
        }
    }

    pub fn into_live(self, map: &EntityMap) -> MovementState {
        match self {
            Self::Docked { system_bits } => MovementState::Docked {
                system: remap_entity(system_bits, map),
            },
            Self::SubLight {
                origin,
                destination,
                speed_fraction,
                departed_at,
            } => MovementState::SubLight {
                origin,
                destination,
                speed_fraction,
                departed_at,
            },
            Self::FTL {
                destination_bits,
                departed_at,
                arrives_at,
            } => MovementState::FTL {
                destination: remap_entity(destination_bits, map),
                departed_at,
                arrives_at,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Owner
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SavedOwner {
    Empire { entity_bits: u64 },
    Neutral,
}

impl SavedOwner {
    pub fn from_live(v: &Owner) -> Self {
        match v {
            Owner::Empire(e) => Self::Empire {
                entity_bits: e.to_bits(),
            },
            Owner::Neutral => Self::Neutral,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> Owner {
        match self {
            Self::Empire { entity_bits } => Owner::Empire(remap_entity(entity_bits, map)),
            Self::Neutral => Owner::Neutral,
        }
    }
}

// ---------------------------------------------------------------------------
// Galaxy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedStarSystem {
    pub name: String,
    pub surveyed: bool,
    pub is_capital: bool,
    pub star_type: String,
}

impl SavedStarSystem {
    pub fn from_live(v: &StarSystem) -> Self {
        Self {
            name: v.name.clone(),
            surveyed: v.surveyed,
            is_capital: v.is_capital,
            star_type: v.star_type.clone(),
        }
    }
    pub fn into_live(self) -> StarSystem {
        StarSystem {
            name: self.name,
            surveyed: self.surveyed,
            is_capital: self.is_capital,
            star_type: self.star_type,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPlanet {
    pub name: String,
    pub system_bits: u64,
    pub planet_type: String,
}

impl SavedPlanet {
    pub fn from_live(v: &Planet) -> Self {
        Self {
            name: v.name.clone(),
            system_bits: v.system.to_bits(),
            planet_type: v.planet_type.clone(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> Planet {
        Planet {
            name: self.name,
            system: remap_entity(self.system_bits, map),
            planet_type: self.planet_type,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSystemAttributes {
    pub habitability: f64,
    pub mineral_richness: f64,
    pub energy_potential: f64,
    pub research_potential: f64,
    pub max_building_slots: u8,
}

impl SavedSystemAttributes {
    pub fn from_live(v: &SystemAttributes) -> Self {
        Self {
            habitability: v.habitability,
            mineral_richness: v.mineral_richness,
            energy_potential: v.energy_potential,
            research_potential: v.research_potential,
            max_building_slots: v.max_building_slots,
        }
    }
    pub fn into_live(self) -> SystemAttributes {
        SystemAttributes {
            habitability: self.habitability,
            mineral_richness: self.mineral_richness,
            energy_potential: self.energy_potential,
            research_potential: self.research_potential,
            max_building_slots: self.max_building_slots,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSovereignty {
    pub owner: Option<SavedOwner>,
    pub control_score: f64,
}

impl SavedSovereignty {
    pub fn from_live(v: &Sovereignty) -> Self {
        Self {
            owner: v.owner.as_ref().map(SavedOwner::from_live),
            control_score: v.control_score,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> Sovereignty {
        Sovereignty {
            owner: self.owner.map(|o| o.into_live(map)),
            control_score: self.control_score,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedHostileType {
    SpaceCreature,
    AncientDefense,
}

impl From<&HostileType> for SavedHostileType {
    fn from(v: &HostileType) -> Self {
        match v {
            HostileType::SpaceCreature => Self::SpaceCreature,
            HostileType::AncientDefense => Self::AncientDefense,
        }
    }
}
impl From<SavedHostileType> for HostileType {
    fn from(v: SavedHostileType) -> Self {
        match v {
            SavedHostileType::SpaceCreature => Self::SpaceCreature,
            SavedHostileType::AncientDefense => Self::AncientDefense,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedHostilePresence {
    pub system_bits: u64,
    pub strength: f64,
    pub hp: f64,
    pub max_hp: f64,
    pub hostile_type: SavedHostileType,
    pub evasion: f64,
}

impl SavedHostilePresence {
    pub fn from_live(v: &HostilePresence) -> Self {
        Self {
            system_bits: v.system.to_bits(),
            strength: v.strength,
            hp: v.hp,
            max_hp: v.max_hp,
            hostile_type: (&v.hostile_type).into(),
            evasion: v.evasion,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> HostilePresence {
        HostilePresence {
            system: remap_entity(self.system_bits, map),
            strength: self.strength,
            hp: self.hp,
            max_hp: self.max_hp,
            hostile_type: self.hostile_type.into(),
            evasion: self.evasion,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPortFacility {
    pub partner_bits: u64,
}

impl SavedPortFacility {
    pub fn from_live(v: &PortFacility) -> Self {
        Self {
            partner_bits: v.partner.to_bits(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> PortFacility {
        PortFacility {
            partner: remap_entity(self.partner_bits, map),
        }
    }
}

// ---------------------------------------------------------------------------
// Colony / Resources
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedColony {
    pub planet_bits: u64,
    pub population: f64,
    pub growth_rate: f64,
}

impl SavedColony {
    pub fn from_live(v: &Colony) -> Self {
        Self {
            planet_bits: v.planet.to_bits(),
            population: v.population,
            growth_rate: v.growth_rate,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> Colony {
        Colony {
            planet: remap_entity(self.planet_bits, map),
            population: self.population,
            growth_rate: self.growth_rate,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedResourceStockpile {
    pub minerals: Amt,
    pub energy: Amt,
    pub research: Amt,
    pub food: Amt,
    pub authority: Amt,
}

impl SavedResourceStockpile {
    pub fn from_live(v: &ResourceStockpile) -> Self {
        Self {
            minerals: v.minerals,
            energy: v.energy,
            research: v.research,
            food: v.food,
            authority: v.authority,
        }
    }
    pub fn into_live(self) -> ResourceStockpile {
        ResourceStockpile {
            minerals: self.minerals,
            energy: self.energy,
            research: self.research,
            food: self.food,
            authority: self.authority,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedResourceCapacity {
    pub minerals: Amt,
    pub energy: Amt,
    pub food: Amt,
    pub authority: Amt,
}

impl SavedResourceCapacity {
    pub fn from_live(v: &ResourceCapacity) -> Self {
        Self {
            minerals: v.minerals,
            energy: v.energy,
            food: v.food,
            authority: v.authority,
        }
    }
    pub fn into_live(self) -> ResourceCapacity {
        ResourceCapacity {
            minerals: self.minerals,
            energy: self.energy,
            food: self.food,
            authority: self.authority,
        }
    }
}

// ---------------------------------------------------------------------------
// Ship
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedShip {
    pub name: String,
    pub design_id: String,
    pub hull_id: String,
    pub modules: Vec<SavedEquippedModule>,
    pub owner: SavedOwner,
    pub sublight_speed: f64,
    pub ftl_range: f64,
    pub player_aboard: bool,
    pub home_port_bits: u64,
    pub design_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedEquippedModule {
    pub slot_type: String,
    pub module_id: String,
}

impl SavedShip {
    pub fn from_live(v: &Ship) -> Self {
        Self {
            name: v.name.clone(),
            design_id: v.design_id.clone(),
            hull_id: v.hull_id.clone(),
            modules: v
                .modules
                .iter()
                .map(|m| SavedEquippedModule {
                    slot_type: m.slot_type.clone(),
                    module_id: m.module_id.clone(),
                })
                .collect(),
            owner: SavedOwner::from_live(&v.owner),
            sublight_speed: v.sublight_speed,
            ftl_range: v.ftl_range,
            player_aboard: v.player_aboard,
            home_port_bits: v.home_port.to_bits(),
            design_revision: v.design_revision,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> Ship {
        Ship {
            name: self.name,
            design_id: self.design_id,
            hull_id: self.hull_id,
            modules: self
                .modules
                .into_iter()
                .map(|m| crate::ship::EquippedModule {
                    slot_type: m.slot_type,
                    module_id: m.module_id,
                })
                .collect(),
            owner: self.owner.into_live(map),
            sublight_speed: self.sublight_speed,
            ftl_range: self.ftl_range,
            player_aboard: self.player_aboard,
            home_port: remap_entity(self.home_port_bits, map),
            design_revision: self.design_revision,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedShipState {
    Docked {
        system_bits: u64,
    },
    SubLight {
        origin: [f64; 3],
        destination: [f64; 3],
        target_system_bits: Option<u64>,
        departed_at: i64,
        arrival_at: i64,
    },
    InFTL {
        origin_system_bits: u64,
        destination_system_bits: u64,
        departed_at: i64,
        arrival_at: i64,
    },
    Surveying {
        target_system_bits: u64,
        started_at: i64,
        completes_at: i64,
    },
    Settling {
        system_bits: u64,
        planet_bits: Option<u64>,
        started_at: i64,
        completes_at: i64,
    },
    Refitting {
        system_bits: u64,
        started_at: i64,
        completes_at: i64,
        new_modules: Vec<SavedEquippedModule>,
        target_revision: u64,
    },
    Loitering {
        position: [f64; 3],
    },
    Scouting {
        target_system_bits: u64,
        origin_system_bits: u64,
        started_at: i64,
        completes_at: i64,
        report_mode_ftl: bool,
    },
}

impl SavedShipState {
    pub fn from_live(v: &ShipState) -> Self {
        use crate::ship::ReportMode;
        match v {
            ShipState::Docked { system } => Self::Docked {
                system_bits: system.to_bits(),
            },
            ShipState::SubLight {
                origin,
                destination,
                target_system,
                departed_at,
                arrival_at,
            } => Self::SubLight {
                origin: *origin,
                destination: *destination,
                target_system_bits: target_system.map(|e| e.to_bits()),
                departed_at: *departed_at,
                arrival_at: *arrival_at,
            },
            ShipState::InFTL {
                origin_system,
                destination_system,
                departed_at,
                arrival_at,
            } => Self::InFTL {
                origin_system_bits: origin_system.to_bits(),
                destination_system_bits: destination_system.to_bits(),
                departed_at: *departed_at,
                arrival_at: *arrival_at,
            },
            ShipState::Surveying {
                target_system,
                started_at,
                completes_at,
            } => Self::Surveying {
                target_system_bits: target_system.to_bits(),
                started_at: *started_at,
                completes_at: *completes_at,
            },
            ShipState::Settling {
                system,
                planet,
                started_at,
                completes_at,
            } => Self::Settling {
                system_bits: system.to_bits(),
                planet_bits: planet.map(|e| e.to_bits()),
                started_at: *started_at,
                completes_at: *completes_at,
            },
            ShipState::Refitting {
                system,
                started_at,
                completes_at,
                new_modules,
                target_revision,
            } => Self::Refitting {
                system_bits: system.to_bits(),
                started_at: *started_at,
                completes_at: *completes_at,
                new_modules: new_modules
                    .iter()
                    .map(|m| SavedEquippedModule {
                        slot_type: m.slot_type.clone(),
                        module_id: m.module_id.clone(),
                    })
                    .collect(),
                target_revision: *target_revision,
            },
            ShipState::Loitering { position } => Self::Loitering {
                position: *position,
            },
            ShipState::Scouting {
                target_system,
                origin_system,
                started_at,
                completes_at,
                report_mode,
            } => Self::Scouting {
                target_system_bits: target_system.to_bits(),
                origin_system_bits: origin_system.to_bits(),
                started_at: *started_at,
                completes_at: *completes_at,
                report_mode_ftl: matches!(report_mode, ReportMode::FtlComm),
            },
        }
    }
    pub fn into_live(self, map: &EntityMap) -> ShipState {
        use crate::ship::ReportMode;
        match self {
            Self::Docked { system_bits } => ShipState::Docked {
                system: remap_entity(system_bits, map),
            },
            Self::SubLight {
                origin,
                destination,
                target_system_bits,
                departed_at,
                arrival_at,
            } => ShipState::SubLight {
                origin,
                destination,
                target_system: target_system_bits.map(|b| remap_entity(b, map)),
                departed_at,
                arrival_at,
            },
            Self::InFTL {
                origin_system_bits,
                destination_system_bits,
                departed_at,
                arrival_at,
            } => ShipState::InFTL {
                origin_system: remap_entity(origin_system_bits, map),
                destination_system: remap_entity(destination_system_bits, map),
                departed_at,
                arrival_at,
            },
            Self::Surveying {
                target_system_bits,
                started_at,
                completes_at,
            } => ShipState::Surveying {
                target_system: remap_entity(target_system_bits, map),
                started_at,
                completes_at,
            },
            Self::Settling {
                system_bits,
                planet_bits,
                started_at,
                completes_at,
            } => ShipState::Settling {
                system: remap_entity(system_bits, map),
                planet: planet_bits.map(|b| remap_entity(b, map)),
                started_at,
                completes_at,
            },
            Self::Refitting {
                system_bits,
                started_at,
                completes_at,
                new_modules,
                target_revision,
            } => ShipState::Refitting {
                system: remap_entity(system_bits, map),
                started_at,
                completes_at,
                new_modules: new_modules
                    .into_iter()
                    .map(|m| crate::ship::EquippedModule {
                        slot_type: m.slot_type,
                        module_id: m.module_id,
                    })
                    .collect(),
                target_revision,
            },
            Self::Loitering { position } => ShipState::Loitering { position },
            Self::Scouting {
                target_system_bits,
                origin_system_bits,
                started_at,
                completes_at,
                report_mode_ftl,
            } => ShipState::Scouting {
                target_system: remap_entity(target_system_bits, map),
                origin_system: remap_entity(origin_system_bits, map),
                started_at,
                completes_at,
                report_mode: if report_mode_ftl {
                    ReportMode::FtlComm
                } else {
                    ReportMode::Return
                },
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedShipHitpoints {
    pub hull: f64,
    pub hull_max: f64,
    pub armor: f64,
    pub armor_max: f64,
    pub shield: f64,
    pub shield_max: f64,
    pub shield_regen: f64,
}

impl SavedShipHitpoints {
    pub fn from_live(v: &ShipHitpoints) -> Self {
        Self {
            hull: v.hull,
            hull_max: v.hull_max,
            armor: v.armor,
            armor_max: v.armor_max,
            shield: v.shield,
            shield_max: v.shield_max,
            shield_regen: v.shield_regen,
        }
    }
    pub fn into_live(self) -> ShipHitpoints {
        ShipHitpoints {
            hull: self.hull,
            hull_max: self.hull_max,
            armor: self.armor,
            armor_max: self.armor_max,
            shield: self.shield,
            shield_max: self.shield_max,
            shield_regen: self.shield_regen,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedCargoItem {
    Deliverable { definition_id: String },
}

impl From<&CargoItem> for SavedCargoItem {
    fn from(v: &CargoItem) -> Self {
        match v {
            CargoItem::Deliverable { definition_id } => Self::Deliverable {
                definition_id: definition_id.clone(),
            },
        }
    }
}
impl From<SavedCargoItem> for CargoItem {
    fn from(v: SavedCargoItem) -> Self {
        match v {
            SavedCargoItem::Deliverable { definition_id } => Self::Deliverable { definition_id },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCargo {
    pub minerals: Amt,
    pub energy: Amt,
    pub items: Vec<SavedCargoItem>,
}

impl SavedCargo {
    pub fn from_live(v: &Cargo) -> Self {
        Self {
            minerals: v.minerals,
            energy: v.energy,
            items: v.items.iter().map(Into::into).collect(),
        }
    }
    pub fn into_live(self) -> Cargo {
        Cargo {
            minerals: self.minerals,
            energy: self.energy,
            items: self.items.into_iter().map(Into::into).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Faction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SavedFactionOwner {
    pub entity_bits: u64,
}

impl SavedFactionOwner {
    pub fn from_live(v: &FactionOwner) -> Self {
        Self {
            entity_bits: v.0.to_bits(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> FactionOwner {
        FactionOwner(remap_entity(self.entity_bits, map))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFaction {
    pub id: String,
    pub name: String,
}

impl SavedFaction {
    pub fn from_live(v: &Faction) -> Self {
        Self {
            id: v.id.clone(),
            name: v.name.clone(),
        }
    }
    pub fn into_live(self) -> Faction {
        Faction {
            id: self.id,
            name: self.name,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SavedRelationState {
    Neutral,
    Peace,
    War,
    Alliance,
}

impl From<&RelationState> for SavedRelationState {
    fn from(v: &RelationState) -> Self {
        match v {
            RelationState::Neutral => Self::Neutral,
            RelationState::Peace => Self::Peace,
            RelationState::War => Self::War,
            RelationState::Alliance => Self::Alliance,
        }
    }
}
impl From<SavedRelationState> for RelationState {
    fn from(v: SavedRelationState) -> Self {
        match v {
            SavedRelationState::Neutral => Self::Neutral,
            SavedRelationState::Peace => Self::Peace,
            SavedRelationState::War => Self::War,
            SavedRelationState::Alliance => Self::Alliance,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFactionView {
    pub state: SavedRelationState,
    pub standing: f64,
}

impl SavedFactionView {
    pub fn from_live(v: &FactionView) -> Self {
        Self {
            state: (&v.state).into(),
            standing: v.standing,
        }
    }
    pub fn into_live(self) -> FactionView {
        FactionView::new(self.state.into(), self.standing)
    }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPlayer;

impl SavedPlayer {
    pub fn from_live(_v: &Player) -> Self {
        Self
    }
    pub fn into_live(self) -> Player {
        Player
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedStationedAt {
    pub system_bits: u64,
}

impl SavedStationedAt {
    pub fn from_live(v: &StationedAt) -> Self {
        Self {
            system_bits: v.system.to_bits(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> StationedAt {
        StationedAt {
            system: remap_entity(self.system_bits, map),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedAboardShip {
    pub ship_bits: u64,
}

impl SavedAboardShip {
    pub fn from_live(v: &AboardShip) -> Self {
        Self {
            ship_bits: v.ship.to_bits(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> AboardShip {
        AboardShip {
            ship: remap_entity(self.ship_bits, map),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedEmpire {
    pub name: String,
}

impl SavedEmpire {
    pub fn from_live(v: &Empire) -> Self {
        Self { name: v.name.clone() }
    }
    pub fn into_live(self) -> Empire {
        Empire { name: self.name }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPlayerEmpire;

// ---------------------------------------------------------------------------
// Obscured marker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedObscuredByGas;

// ---------------------------------------------------------------------------
// SavedComponentBag
// ---------------------------------------------------------------------------

/// Holds every persistable component for a single entity as `Option<_>` fields.
/// Only the populated options are re-inserted on load.
///
/// Phase A range: galaxy (StarSystem/Planet/attributes/sovereignty/hostile/port),
/// colony basics (Colony, stockpile, capacity), ship basics (Ship/ShipState/HP/cargo),
/// faction identity + owner, player location, and generic `Position`/`MovementState`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedComponentBag {
    // Transforms
    pub position: Option<Position>,
    pub movement_state: Option<SavedMovementState>,
    // Galaxy
    pub star_system: Option<SavedStarSystem>,
    pub planet: Option<SavedPlanet>,
    pub system_attributes: Option<SavedSystemAttributes>,
    pub sovereignty: Option<SavedSovereignty>,
    pub hostile_presence: Option<SavedHostilePresence>,
    pub obscured_by_gas: Option<SavedObscuredByGas>,
    pub port_facility: Option<SavedPortFacility>,
    // Colony
    pub colony: Option<SavedColony>,
    pub resource_stockpile: Option<SavedResourceStockpile>,
    pub resource_capacity: Option<SavedResourceCapacity>,
    // Ship
    pub ship: Option<SavedShip>,
    pub ship_state: Option<SavedShipState>,
    pub ship_hitpoints: Option<SavedShipHitpoints>,
    pub cargo: Option<SavedCargo>,
    // Faction
    pub faction_owner: Option<SavedFactionOwner>,
    pub faction: Option<SavedFaction>,
    // Player
    pub player: Option<SavedPlayer>,
    pub stationed_at: Option<SavedStationedAt>,
    pub aboard_ship: Option<SavedAboardShip>,
    pub empire: Option<SavedEmpire>,
    pub player_empire: Option<SavedPlayerEmpire>,
}

impl RemapEntities for SavedComponentBag {
    fn remap_entities(&mut self, _map: &EntityMap) {
        // All entity references are stored as `u64` bits; remapping is done
        // when wire structs are converted back into live components via
        // their `into_live(map)` methods. No in-place rewriting needed here.
    }
}
