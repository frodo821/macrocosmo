//! Wire-format "saved component bag" for Phase A + B save/load (#247).
//!
//! Each live ECS component is mirrored by a `Saved*` wire struct which is
//! `Serialize + Deserialize`-able via postcard. Entity references are encoded
//! as `u64` save ids (via [`EntityMap`]) and translated back to live
//! `Entity`s on load via [`RemapEntities`].
//!
//! Phase A scope: galaxy / colony basics / ship basics / faction / player.
//! Phase B adds: ship extension state (CommandQueue/CourierRoute/SurveyData/
//! ScoutReport/Fleet/DetectedHostiles/RulesOfEngagement/ShipModifiers),
//! colony extension state (BuildQueue/BuildingQueue/SystemBuildings/
//! Buildings/Production/ColonyJobs/ColonyJobRates/ColonyPopulation/
//! ConstructionParams/AuthorityParams/MaintenanceCost/FoodConsumption/
//! DeliverableStockpile/ColonizationQueue/Anomalies), deep-space
//! (DeepSpaceStructure/FTLCommRelay/StructureHitpoints/ConstructionPlatform/
//! Scrapyard/LifetimeCost/ForbiddenRegion), knowledge (KnowledgeStore/
//! PendingFactQueue/CommsParams), pending command queues (PendingShipCommand/
//! PendingDiplomaticAction/PendingCommand), tech tree (TechTree/ResearchQueue/
//! ResearchPool/PendingResearch/TechKnowledge/PendingKnowledgePropagation/
//! PendingColonyTechModifiers/EmpireModifiers/GameFlags/ScopedFlags/
//! GlobalParams), event/notification logs.

use bevy::prelude::Entity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::amount::Amt;
use crate::colony::{
    AlertCooldowns, AuthorityParams, BuildKind, BuildOrder, BuildQueue, Buildings, BuildingOrder,
    BuildingQueue, Colony, ColonizationOrder, ColonizationQueue, ColonyJobRates,
    ConstructionParams, DeliverableStockpile, DemolitionOrder, FoodConsumption, MaintenanceCost,
    PendingColonizationOrder, Production, ProductionFocus, ResourceCapacity, ResourceStockpile,
    SystemBuildingQueue, SystemBuildings, UpgradeOrder,
};
use crate::communication::{
    ColonyCommand, ColonyCommandKind, CommandLog, CommandLogEntry, PendingCommand, RemoteCommand,
};
use crate::components::{MovementState, Position};
use crate::condition::ScopedFlags;
use crate::deep_space::{
    CommDirection, ConstructionPlatform, DeepSpaceStructure, FTLCommRelay, LifetimeCost,
    ResourceCost, Scrapyard, StructureHitpoints,
};
use crate::empire::CommsParams;
use crate::events::{EventLog, GameEvent, GameEventKind};
use crate::faction::{
    DiplomaticAction, FactionOwner, FactionView, PendingDiplomaticAction, RelationState,
};
use crate::galaxy::{
    Anomalies, Anomaly, ForbiddenRegion, HostilePresence, HostileType, Planet, PortFacility,
    Sovereignty, StarSystem, SystemAttributes,
};
use crate::knowledge::{
    KnowledgeFact, KnowledgeStore, ObservationSource, PendingFactQueue, PerceivedFact, ShipSnapshot,
    ShipSnapshotState, SystemKnowledge, SystemSnapshot,
};
use crate::knowledge::facts::CombatVictor;
use crate::modifier::{ModifiedValue, ScopedModifiers};
use crate::notifications::{Notification, NotificationPriority, NotificationQueue};
use crate::player::{AboardShip, Empire, Faction, Player, StationedAt};
use crate::scripting::building_api::BuildingId;
use crate::ship::scout::ScoutReport;
use crate::ship::{
    Cargo, CargoItem, CommandQueue, CourierMode, CourierRoute, DetectedHostiles, Fleet,
    FleetMembership, Owner, PendingShipCommand, QueuedCommand, ReportMode, RulesOfEngagement,
    Ship, ShipCommand, ShipHitpoints, ShipModifiers, ShipState, SurveyData,
};
use crate::species::{ColonyJobs, ColonyPopulation, ColonySpecies, JobSlot};
use crate::technology::{
    EmpireModifiers, GameFlags, GlobalParams, PendingColonyTechModifiers,
    PendingKnowledgePropagation, PendingResearch, RecentlyResearched, ResearchPool, ResearchQueue,
    TechId, TechKnowledge, TechTree,
};

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

// ===========================================================================
// Phase B wire types
// ===========================================================================

// ---------------------------------------------------------------------------
// Ship — extension state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SavedReportMode {
    FtlComm,
    Return,
}

impl From<&ReportMode> for SavedReportMode {
    fn from(v: &ReportMode) -> Self {
        match v {
            ReportMode::FtlComm => Self::FtlComm,
            ReportMode::Return => Self::Return,
        }
    }
}
impl From<SavedReportMode> for ReportMode {
    fn from(v: SavedReportMode) -> Self {
        match v {
            SavedReportMode::FtlComm => Self::FtlComm,
            SavedReportMode::Return => Self::Return,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SavedRulesOfEngagement {
    Aggressive,
    Defensive,
    Retreat,
}

impl From<&RulesOfEngagement> for SavedRulesOfEngagement {
    fn from(v: &RulesOfEngagement) -> Self {
        match v {
            RulesOfEngagement::Aggressive => Self::Aggressive,
            RulesOfEngagement::Defensive => Self::Defensive,
            RulesOfEngagement::Retreat => Self::Retreat,
        }
    }
}
impl From<SavedRulesOfEngagement> for RulesOfEngagement {
    fn from(v: SavedRulesOfEngagement) -> Self {
        match v {
            SavedRulesOfEngagement::Aggressive => Self::Aggressive,
            SavedRulesOfEngagement::Defensive => Self::Defensive,
            SavedRulesOfEngagement::Retreat => Self::Retreat,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedQueuedCommand {
    MoveTo { system_bits: u64 },
    Survey { system_bits: u64 },
    Colonize { system_bits: u64, planet_bits: Option<u64> },
    MoveToCoordinates { target: [f64; 3] },
    Scout { target_system_bits: u64, observation_duration: i64, report_mode: SavedReportMode },
    LoadDeliverable { system_bits: u64, stockpile_index: usize },
    DeployDeliverable { position: [f64; 3], item_index: usize },
    TransferToStructure { structure_bits: u64, minerals: Amt, energy: Amt },
    LoadFromScrapyard { structure_bits: u64 },
}

impl SavedQueuedCommand {
    pub fn from_live(v: &QueuedCommand) -> Self {
        match v {
            QueuedCommand::MoveTo { system } => Self::MoveTo { system_bits: system.to_bits() },
            QueuedCommand::Survey { system } => Self::Survey { system_bits: system.to_bits() },
            QueuedCommand::Colonize { system, planet } => Self::Colonize {
                system_bits: system.to_bits(),
                planet_bits: planet.map(|e| e.to_bits()),
            },
            QueuedCommand::MoveToCoordinates { target } => Self::MoveToCoordinates { target: *target },
            QueuedCommand::Scout { target_system, observation_duration, report_mode } => Self::Scout {
                target_system_bits: target_system.to_bits(),
                observation_duration: *observation_duration,
                report_mode: report_mode.into(),
            },
            QueuedCommand::LoadDeliverable { system, stockpile_index } => Self::LoadDeliverable {
                system_bits: system.to_bits(),
                stockpile_index: *stockpile_index,
            },
            QueuedCommand::DeployDeliverable { position, item_index } => Self::DeployDeliverable {
                position: *position,
                item_index: *item_index,
            },
            QueuedCommand::TransferToStructure { structure, minerals, energy } => Self::TransferToStructure {
                structure_bits: structure.to_bits(),
                minerals: *minerals,
                energy: *energy,
            },
            QueuedCommand::LoadFromScrapyard { structure } => Self::LoadFromScrapyard {
                structure_bits: structure.to_bits(),
            },
        }
    }

    pub fn into_live(self, map: &EntityMap) -> QueuedCommand {
        match self {
            Self::MoveTo { system_bits } => QueuedCommand::MoveTo { system: remap_entity(system_bits, map) },
            Self::Survey { system_bits } => QueuedCommand::Survey { system: remap_entity(system_bits, map) },
            Self::Colonize { system_bits, planet_bits } => QueuedCommand::Colonize {
                system: remap_entity(system_bits, map),
                planet: planet_bits.map(|b| remap_entity(b, map)),
            },
            Self::MoveToCoordinates { target } => QueuedCommand::MoveToCoordinates { target },
            Self::Scout { target_system_bits, observation_duration, report_mode } => QueuedCommand::Scout {
                target_system: remap_entity(target_system_bits, map),
                observation_duration,
                report_mode: report_mode.into(),
            },
            Self::LoadDeliverable { system_bits, stockpile_index } => QueuedCommand::LoadDeliverable {
                system: remap_entity(system_bits, map),
                stockpile_index,
            },
            Self::DeployDeliverable { position, item_index } => QueuedCommand::DeployDeliverable { position, item_index },
            Self::TransferToStructure { structure_bits, minerals, energy } => QueuedCommand::TransferToStructure {
                structure: remap_entity(structure_bits, map),
                minerals,
                energy,
            },
            Self::LoadFromScrapyard { structure_bits } => QueuedCommand::LoadFromScrapyard {
                structure: remap_entity(structure_bits, map),
            },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedCommandQueue {
    pub commands: Vec<SavedQueuedCommand>,
    pub predicted_position: [f64; 3],
    pub predicted_system_bits: Option<u64>,
}

impl SavedCommandQueue {
    pub fn from_live(v: &CommandQueue) -> Self {
        Self {
            commands: v.commands.iter().map(SavedQueuedCommand::from_live).collect(),
            predicted_position: v.predicted_position,
            predicted_system_bits: v.predicted_system.map(|e| e.to_bits()),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> CommandQueue {
        CommandQueue {
            commands: self.commands.into_iter().map(|c| c.into_live(map)).collect(),
            predicted_position: self.predicted_position,
            predicted_system: self.predicted_system_bits.map(|b| remap_entity(b, map)),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedShipModifiers {
    pub speed: ScopedModifiers,
    pub ftl_range: ScopedModifiers,
    pub survey_speed: ScopedModifiers,
    pub colonize_speed: ScopedModifiers,
    pub evasion: ScopedModifiers,
    pub cargo_capacity: ScopedModifiers,
    pub attack: ScopedModifiers,
    pub defense: ScopedModifiers,
    pub armor_max: ScopedModifiers,
    pub shield_max: ScopedModifiers,
    pub shield_regen: ScopedModifiers,
}

impl SavedShipModifiers {
    pub fn from_live(v: &ShipModifiers) -> Self {
        Self {
            speed: v.speed.clone(),
            ftl_range: v.ftl_range.clone(),
            survey_speed: v.survey_speed.clone(),
            colonize_speed: v.colonize_speed.clone(),
            evasion: v.evasion.clone(),
            cargo_capacity: v.cargo_capacity.clone(),
            attack: v.attack.clone(),
            defense: v.defense.clone(),
            armor_max: v.armor_max.clone(),
            shield_max: v.shield_max.clone(),
            shield_regen: v.shield_regen.clone(),
        }
    }
    pub fn into_live(self) -> ShipModifiers {
        ShipModifiers {
            speed: self.speed,
            ftl_range: self.ftl_range,
            survey_speed: self.survey_speed,
            colonize_speed: self.colonize_speed,
            evasion: self.evasion,
            cargo_capacity: self.cargo_capacity,
            attack: self.attack,
            defense: self.defense,
            armor_max: self.armor_max,
            shield_max: self.shield_max,
            shield_regen: self.shield_regen,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedCourierMode {
    KnowledgeRelay,
    ResourceTransport,
    MessageDelivery,
}
impl From<&CourierMode> for SavedCourierMode {
    fn from(v: &CourierMode) -> Self {
        match v {
            CourierMode::KnowledgeRelay => Self::KnowledgeRelay,
            CourierMode::ResourceTransport => Self::ResourceTransport,
            CourierMode::MessageDelivery => Self::MessageDelivery,
        }
    }
}
impl From<SavedCourierMode> for CourierMode {
    fn from(v: SavedCourierMode) -> Self {
        match v {
            SavedCourierMode::KnowledgeRelay => Self::KnowledgeRelay,
            SavedCourierMode::ResourceTransport => Self::ResourceTransport,
            SavedCourierMode::MessageDelivery => Self::MessageDelivery,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCourierRoute {
    pub waypoints_bits: Vec<u64>,
    pub current_index: usize,
    pub mode: SavedCourierMode,
    pub repeat: bool,
    pub paused: bool,
}

impl SavedCourierRoute {
    pub fn from_live(v: &CourierRoute) -> Self {
        Self {
            waypoints_bits: v.waypoints.iter().map(|e| e.to_bits()).collect(),
            current_index: v.current_index,
            mode: (&v.mode).into(),
            repeat: v.repeat,
            paused: v.paused,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> CourierRoute {
        CourierRoute {
            waypoints: self.waypoints_bits.into_iter().map(|b| remap_entity(b, map)).collect(),
            current_index: self.current_index,
            mode: self.mode.into(),
            repeat: self.repeat,
            paused: self.paused,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSurveyData {
    pub target_system_bits: u64,
    pub surveyed_at: i64,
    pub system_name: String,
    pub anomaly_id: Option<String>,
}

impl SavedSurveyData {
    pub fn from_live(v: &SurveyData) -> Self {
        Self {
            target_system_bits: v.target_system.to_bits(),
            surveyed_at: v.surveyed_at,
            system_name: v.system_name.clone(),
            anomaly_id: v.anomaly_id.clone(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> SurveyData {
        SurveyData {
            target_system: remap_entity(self.target_system_bits, map),
            surveyed_at: self.surveyed_at,
            system_name: self.system_name,
            anomaly_id: self.anomaly_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedScoutReport {
    pub target_system_bits: u64,
    pub origin_system_bits: u64,
    pub observed_at: i64,
    pub report_mode: SavedReportMode,
    pub system_snapshot: SavedSystemSnapshot,
    pub ship_snapshots: Vec<SavedShipSnapshot>,
    pub return_queued: bool,
}

impl SavedScoutReport {
    pub fn from_live(v: &ScoutReport) -> Self {
        Self {
            target_system_bits: v.target_system.to_bits(),
            origin_system_bits: v.origin_system.to_bits(),
            observed_at: v.observed_at,
            report_mode: (&v.report_mode).into(),
            system_snapshot: SavedSystemSnapshot::from_live(&v.system_snapshot),
            ship_snapshots: v.ship_snapshots.iter().map(SavedShipSnapshot::from_live).collect(),
            return_queued: v.return_queued,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> ScoutReport {
        ScoutReport {
            target_system: remap_entity(self.target_system_bits, map),
            origin_system: remap_entity(self.origin_system_bits, map),
            observed_at: self.observed_at,
            report_mode: self.report_mode.into(),
            system_snapshot: self.system_snapshot.into_live(),
            ship_snapshots: self.ship_snapshots.into_iter().map(|s| s.into_live(map)).collect(),
            return_queued: self.return_queued,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFleet {
    pub name: String,
    pub members_bits: Vec<u64>,
    pub flagship_bits: u64,
}

impl SavedFleet {
    pub fn from_live(v: &Fleet) -> Self {
        Self {
            name: v.name.clone(),
            members_bits: v.members.iter().map(|e| e.to_bits()).collect(),
            flagship_bits: v.flagship.to_bits(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> Fleet {
        Fleet {
            name: self.name,
            members: self.members_bits.into_iter().map(|b| remap_entity(b, map)).collect(),
            flagship: remap_entity(self.flagship_bits, map),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SavedFleetMembership {
    pub fleet_bits: u64,
}

impl SavedFleetMembership {
    pub fn from_live(v: &FleetMembership) -> Self {
        Self { fleet_bits: v.fleet.to_bits() }
    }
    pub fn into_live(self, map: &EntityMap) -> FleetMembership {
        FleetMembership { fleet: remap_entity(self.fleet_bits, map) }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedDetectedHostiles {
    /// Vec<(target_save_id_bits, last_detected_at)>.
    pub entries: Vec<(u64, i64)>,
}

impl SavedDetectedHostiles {
    pub fn from_live(v: &DetectedHostiles) -> Self {
        Self {
            entries: v.entries.iter().map(|(e, t)| (e.to_bits(), *t)).collect(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> DetectedHostiles {
        let mut out = DetectedHostiles::default();
        for (bits, t) in self.entries {
            out.entries.insert(remap_entity(bits, map), t);
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPendingShipCommand {
    pub ship_bits: u64,
    pub command: SavedShipCommand,
    pub arrives_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedShipCommand {
    MoveTo { destination_bits: u64 },
    Survey { target_bits: u64 },
    Colonize,
    SetROE { roe: SavedRulesOfEngagement },
    EnqueueCommand(SavedQueuedCommand),
}

impl SavedShipCommand {
    pub fn from_live(v: &ShipCommand) -> Self {
        match v {
            ShipCommand::MoveTo { destination } => Self::MoveTo { destination_bits: destination.to_bits() },
            ShipCommand::Survey { target } => Self::Survey { target_bits: target.to_bits() },
            ShipCommand::Colonize => Self::Colonize,
            ShipCommand::SetROE { roe } => Self::SetROE { roe: roe.into() },
            ShipCommand::EnqueueCommand(c) => Self::EnqueueCommand(SavedQueuedCommand::from_live(c)),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> ShipCommand {
        match self {
            Self::MoveTo { destination_bits } => ShipCommand::MoveTo { destination: remap_entity(destination_bits, map) },
            Self::Survey { target_bits } => ShipCommand::Survey { target: remap_entity(target_bits, map) },
            Self::Colonize => ShipCommand::Colonize,
            Self::SetROE { roe } => ShipCommand::SetROE { roe: roe.into() },
            Self::EnqueueCommand(c) => ShipCommand::EnqueueCommand(c.into_live(map)),
        }
    }
}

impl SavedPendingShipCommand {
    pub fn from_live(v: &PendingShipCommand) -> Self {
        Self {
            ship_bits: v.ship.to_bits(),
            command: SavedShipCommand::from_live(&v.command),
            arrives_at: v.arrives_at,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> PendingShipCommand {
        PendingShipCommand {
            ship: remap_entity(self.ship_bits, map),
            command: self.command.into_live(map),
            arrives_at: self.arrives_at,
        }
    }
}

// ---------------------------------------------------------------------------
// Colony — extension state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedBuildKind {
    Ship,
    Deliverable { cargo_size: u32 },
}

impl From<&BuildKind> for SavedBuildKind {
    fn from(v: &BuildKind) -> Self {
        match v {
            BuildKind::Ship => Self::Ship,
            BuildKind::Deliverable { cargo_size } => Self::Deliverable { cargo_size: *cargo_size },
        }
    }
}
impl From<SavedBuildKind> for BuildKind {
    fn from(v: SavedBuildKind) -> Self {
        match v {
            SavedBuildKind::Ship => Self::Ship,
            SavedBuildKind::Deliverable { cargo_size } => Self::Deliverable { cargo_size },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedBuildOrder {
    pub kind: SavedBuildKind,
    pub design_id: String,
    pub display_name: String,
    pub minerals_cost: Amt,
    pub minerals_invested: Amt,
    pub energy_cost: Amt,
    pub energy_invested: Amt,
    pub build_time_total: i64,
    pub build_time_remaining: i64,
}

impl SavedBuildOrder {
    pub fn from_live(v: &BuildOrder) -> Self {
        Self {
            kind: (&v.kind).into(),
            design_id: v.design_id.clone(),
            display_name: v.display_name.clone(),
            minerals_cost: v.minerals_cost,
            minerals_invested: v.minerals_invested,
            energy_cost: v.energy_cost,
            energy_invested: v.energy_invested,
            build_time_total: v.build_time_total,
            build_time_remaining: v.build_time_remaining,
        }
    }
    pub fn into_live(self) -> BuildOrder {
        BuildOrder {
            kind: self.kind.into(),
            design_id: self.design_id,
            display_name: self.display_name,
            minerals_cost: self.minerals_cost,
            minerals_invested: self.minerals_invested,
            energy_cost: self.energy_cost,
            energy_invested: self.energy_invested,
            build_time_total: self.build_time_total,
            build_time_remaining: self.build_time_remaining,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedBuildQueue {
    pub queue: Vec<SavedBuildOrder>,
}

impl SavedBuildQueue {
    pub fn from_live(v: &BuildQueue) -> Self {
        Self { queue: v.queue.iter().map(SavedBuildOrder::from_live).collect() }
    }
    pub fn into_live(self) -> BuildQueue {
        BuildQueue { queue: self.queue.into_iter().map(SavedBuildOrder::into_live).collect() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedBuildingOrder {
    pub building_id: String,
    pub target_slot: usize,
    pub minerals_remaining: Amt,
    pub energy_remaining: Amt,
    pub build_time_remaining: i64,
}

impl SavedBuildingOrder {
    pub fn from_live(v: &BuildingOrder) -> Self {
        Self {
            building_id: v.building_id.0.clone(),
            target_slot: v.target_slot,
            minerals_remaining: v.minerals_remaining,
            energy_remaining: v.energy_remaining,
            build_time_remaining: v.build_time_remaining,
        }
    }
    pub fn into_live(self) -> BuildingOrder {
        BuildingOrder {
            building_id: BuildingId::new(self.building_id),
            target_slot: self.target_slot,
            minerals_remaining: self.minerals_remaining,
            energy_remaining: self.energy_remaining,
            build_time_remaining: self.build_time_remaining,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDemolitionOrder {
    pub target_slot: usize,
    pub building_id: String,
    pub time_remaining: i64,
    pub minerals_refund: Amt,
    pub energy_refund: Amt,
}

impl SavedDemolitionOrder {
    pub fn from_live(v: &DemolitionOrder) -> Self {
        Self {
            target_slot: v.target_slot,
            building_id: v.building_id.0.clone(),
            time_remaining: v.time_remaining,
            minerals_refund: v.minerals_refund,
            energy_refund: v.energy_refund,
        }
    }
    pub fn into_live(self) -> DemolitionOrder {
        DemolitionOrder {
            target_slot: self.target_slot,
            building_id: BuildingId::new(self.building_id),
            time_remaining: self.time_remaining,
            minerals_refund: self.minerals_refund,
            energy_refund: self.energy_refund,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedUpgradeOrder {
    pub slot_index: usize,
    pub target_id: String,
    pub minerals_remaining: Amt,
    pub energy_remaining: Amt,
    pub build_time_remaining: i64,
}

impl SavedUpgradeOrder {
    pub fn from_live(v: &UpgradeOrder) -> Self {
        Self {
            slot_index: v.slot_index,
            target_id: v.target_id.0.clone(),
            minerals_remaining: v.minerals_remaining,
            energy_remaining: v.energy_remaining,
            build_time_remaining: v.build_time_remaining,
        }
    }
    pub fn into_live(self) -> UpgradeOrder {
        UpgradeOrder {
            slot_index: self.slot_index,
            target_id: BuildingId::new(self.target_id),
            minerals_remaining: self.minerals_remaining,
            energy_remaining: self.energy_remaining,
            build_time_remaining: self.build_time_remaining,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedBuildingQueue {
    pub queue: Vec<SavedBuildingOrder>,
    pub demolition_queue: Vec<SavedDemolitionOrder>,
    pub upgrade_queue: Vec<SavedUpgradeOrder>,
}

impl SavedBuildingQueue {
    pub fn from_live(v: &BuildingQueue) -> Self {
        Self {
            queue: v.queue.iter().map(SavedBuildingOrder::from_live).collect(),
            demolition_queue: v.demolition_queue.iter().map(SavedDemolitionOrder::from_live).collect(),
            upgrade_queue: v.upgrade_queue.iter().map(SavedUpgradeOrder::from_live).collect(),
        }
    }
    pub fn into_live(self) -> BuildingQueue {
        BuildingQueue {
            queue: self.queue.into_iter().map(SavedBuildingOrder::into_live).collect(),
            demolition_queue: self.demolition_queue.into_iter().map(SavedDemolitionOrder::into_live).collect(),
            upgrade_queue: self.upgrade_queue.into_iter().map(SavedUpgradeOrder::into_live).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedBuildings {
    pub slots: Vec<Option<String>>,
}

impl SavedBuildings {
    pub fn from_live(v: &Buildings) -> Self {
        Self { slots: v.slots.iter().map(|s| s.as_ref().map(|b| b.0.clone())).collect() }
    }
    pub fn into_live(self) -> Buildings {
        Buildings { slots: self.slots.into_iter().map(|s| s.map(BuildingId::new)).collect() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSystemBuildings {
    pub slots: Vec<Option<String>>,
}

impl SavedSystemBuildings {
    pub fn from_live(v: &SystemBuildings) -> Self {
        Self { slots: v.slots.iter().map(|s| s.as_ref().map(|b| b.0.clone())).collect() }
    }
    pub fn into_live(self) -> SystemBuildings {
        SystemBuildings { slots: self.slots.into_iter().map(|s| s.map(BuildingId::new)).collect() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedSystemBuildingQueue {
    pub queue: Vec<SavedBuildingOrder>,
    pub demolition_queue: Vec<SavedDemolitionOrder>,
    pub upgrade_queue: Vec<SavedUpgradeOrder>,
}

impl SavedSystemBuildingQueue {
    pub fn from_live(v: &SystemBuildingQueue) -> Self {
        Self {
            queue: v.queue.iter().map(SavedBuildingOrder::from_live).collect(),
            demolition_queue: v.demolition_queue.iter().map(SavedDemolitionOrder::from_live).collect(),
            upgrade_queue: v.upgrade_queue.iter().map(SavedUpgradeOrder::from_live).collect(),
        }
    }
    pub fn into_live(self) -> SystemBuildingQueue {
        SystemBuildingQueue {
            queue: self.queue.into_iter().map(SavedBuildingOrder::into_live).collect(),
            demolition_queue: self.demolition_queue.into_iter().map(SavedDemolitionOrder::into_live).collect(),
            upgrade_queue: self.upgrade_queue.into_iter().map(SavedUpgradeOrder::into_live).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedProductionFocus {
    pub minerals_weight: Amt,
    pub energy_weight: Amt,
    pub research_weight: Amt,
}
impl SavedProductionFocus {
    pub fn from_live(v: &ProductionFocus) -> Self {
        Self {
            minerals_weight: v.minerals_weight,
            energy_weight: v.energy_weight,
            research_weight: v.research_weight,
        }
    }
    pub fn into_live(self) -> ProductionFocus {
        ProductionFocus {
            minerals_weight: self.minerals_weight,
            energy_weight: self.energy_weight,
            research_weight: self.research_weight,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedProduction {
    pub minerals_per_hexadies: ModifiedValue,
    pub energy_per_hexadies: ModifiedValue,
    pub research_per_hexadies: ModifiedValue,
    pub food_per_hexadies: ModifiedValue,
}
impl SavedProduction {
    pub fn from_live(v: &Production) -> Self {
        Self {
            minerals_per_hexadies: v.minerals_per_hexadies.clone(),
            energy_per_hexadies: v.energy_per_hexadies.clone(),
            research_per_hexadies: v.research_per_hexadies.clone(),
            food_per_hexadies: v.food_per_hexadies.clone(),
        }
    }
    pub fn into_live(self) -> Production {
        Production {
            minerals_per_hexadies: self.minerals_per_hexadies,
            energy_per_hexadies: self.energy_per_hexadies,
            research_per_hexadies: self.research_per_hexadies,
            food_per_hexadies: self.food_per_hexadies,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedJobSlot {
    pub job_id: String,
    pub capacity: u32,
    pub assigned: u32,
    pub capacity_from_buildings: u32,
}

impl SavedJobSlot {
    pub fn from_live(v: &JobSlot) -> Self {
        Self {
            job_id: v.job_id.clone(),
            capacity: v.capacity,
            assigned: v.assigned,
            capacity_from_buildings: v.capacity_from_buildings,
        }
    }
    pub fn into_live(self) -> JobSlot {
        JobSlot {
            job_id: self.job_id,
            capacity: self.capacity,
            assigned: self.assigned,
            capacity_from_buildings: self.capacity_from_buildings,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedColonyJobs {
    pub slots: Vec<SavedJobSlot>,
}

impl SavedColonyJobs {
    pub fn from_live(v: &ColonyJobs) -> Self {
        Self { slots: v.slots.iter().map(SavedJobSlot::from_live).collect() }
    }
    pub fn into_live(self) -> ColonyJobs {
        ColonyJobs { slots: self.slots.into_iter().map(SavedJobSlot::into_live).collect() }
    }
}

/// Vec<((job_id, target), ModifiedValue)> wire form for ColonyJobRates buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedColonyJobRates {
    pub buckets: Vec<(String, String, ModifiedValue)>,
}

impl SavedColonyJobRates {
    pub fn from_live(v: &ColonyJobRates) -> Self {
        Self {
            buckets: v.iter().map(|(j, t, mv)| (j.clone(), t.clone(), mv.clone())).collect(),
        }
    }
    pub fn into_live(self) -> ColonyJobRates {
        let mut out = ColonyJobRates::default();
        for (job, target, mv) in self.buckets {
            *out.bucket_mut(&job, &target) = mv;
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedColonySpecies {
    pub species_id: String,
    pub population: u32,
}
impl SavedColonySpecies {
    pub fn from_live(v: &ColonySpecies) -> Self {
        Self { species_id: v.species_id.clone(), population: v.population }
    }
    pub fn into_live(self) -> ColonySpecies {
        ColonySpecies { species_id: self.species_id, population: self.population }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedColonyPopulation {
    pub species: Vec<SavedColonySpecies>,
}
impl SavedColonyPopulation {
    pub fn from_live(v: &ColonyPopulation) -> Self {
        Self { species: v.species.iter().map(SavedColonySpecies::from_live).collect() }
    }
    pub fn into_live(self) -> ColonyPopulation {
        ColonyPopulation { species: self.species.into_iter().map(SavedColonySpecies::into_live).collect() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConstructionParams {
    pub ship_cost_modifier: ModifiedValue,
    pub building_cost_modifier: ModifiedValue,
    pub ship_build_time_modifier: ModifiedValue,
    pub building_build_time_modifier: ModifiedValue,
}
impl SavedConstructionParams {
    pub fn from_live(v: &ConstructionParams) -> Self {
        Self {
            ship_cost_modifier: v.ship_cost_modifier.clone(),
            building_cost_modifier: v.building_cost_modifier.clone(),
            ship_build_time_modifier: v.ship_build_time_modifier.clone(),
            building_build_time_modifier: v.building_build_time_modifier.clone(),
        }
    }
    pub fn into_live(self) -> ConstructionParams {
        ConstructionParams {
            ship_cost_modifier: self.ship_cost_modifier,
            building_cost_modifier: self.building_cost_modifier,
            ship_build_time_modifier: self.ship_build_time_modifier,
            building_build_time_modifier: self.building_build_time_modifier,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedAuthorityParams {
    pub production: ModifiedValue,
    pub cost_per_colony: ModifiedValue,
}
impl SavedAuthorityParams {
    pub fn from_live(v: &AuthorityParams) -> Self {
        Self { production: v.production.clone(), cost_per_colony: v.cost_per_colony.clone() }
    }
    pub fn into_live(self) -> AuthorityParams {
        AuthorityParams { production: self.production, cost_per_colony: self.cost_per_colony }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedMaintenanceCost {
    pub energy_per_hexadies: ModifiedValue,
}
impl SavedMaintenanceCost {
    pub fn from_live(v: &MaintenanceCost) -> Self {
        Self { energy_per_hexadies: v.energy_per_hexadies.clone() }
    }
    pub fn into_live(self) -> MaintenanceCost {
        MaintenanceCost { energy_per_hexadies: self.energy_per_hexadies }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFoodConsumption {
    pub food_per_hexadies: ModifiedValue,
}
impl SavedFoodConsumption {
    pub fn from_live(v: &FoodConsumption) -> Self {
        Self { food_per_hexadies: v.food_per_hexadies.clone() }
    }
    pub fn into_live(self) -> FoodConsumption {
        FoodConsumption { food_per_hexadies: self.food_per_hexadies }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedDeliverableStockpile {
    pub items: Vec<SavedCargoItem>,
}
impl SavedDeliverableStockpile {
    pub fn from_live(v: &DeliverableStockpile) -> Self {
        Self { items: v.items.iter().map(Into::into).collect() }
    }
    pub fn into_live(self) -> DeliverableStockpile {
        DeliverableStockpile { items: self.items.into_iter().map(Into::into).collect() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedColonizationOrder {
    pub target_planet_bits: u64,
    pub source_colony_bits: u64,
    pub minerals_remaining: Amt,
    pub energy_remaining: Amt,
    pub build_time_remaining: i64,
    pub initial_population: f64,
}

impl SavedColonizationOrder {
    pub fn from_live(v: &ColonizationOrder) -> Self {
        Self {
            target_planet_bits: v.target_planet.to_bits(),
            source_colony_bits: v.source_colony.to_bits(),
            minerals_remaining: v.minerals_remaining,
            energy_remaining: v.energy_remaining,
            build_time_remaining: v.build_time_remaining,
            initial_population: v.initial_population,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> ColonizationOrder {
        ColonizationOrder {
            target_planet: remap_entity(self.target_planet_bits, map),
            source_colony: remap_entity(self.source_colony_bits, map),
            minerals_remaining: self.minerals_remaining,
            energy_remaining: self.energy_remaining,
            build_time_remaining: self.build_time_remaining,
            initial_population: self.initial_population,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedColonizationQueue {
    pub orders: Vec<SavedColonizationOrder>,
}

impl SavedColonizationQueue {
    pub fn from_live(v: &ColonizationQueue) -> Self {
        Self { orders: v.orders.iter().map(SavedColonizationOrder::from_live).collect() }
    }
    pub fn into_live(self, map: &EntityMap) -> ColonizationQueue {
        ColonizationQueue {
            orders: self.orders.into_iter().map(|o| o.into_live(map)).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPendingColonizationOrder {
    pub system_entity_bits: u64,
    pub target_planet_bits: u64,
    pub source_colony_bits: u64,
}

impl SavedPendingColonizationOrder {
    pub fn from_live(v: &PendingColonizationOrder) -> Self {
        Self {
            system_entity_bits: v.system_entity.to_bits(),
            target_planet_bits: v.target_planet.to_bits(),
            source_colony_bits: v.source_colony.to_bits(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> PendingColonizationOrder {
        PendingColonizationOrder {
            system_entity: remap_entity(self.system_entity_bits, map),
            target_planet: remap_entity(self.target_planet_bits, map),
            source_colony: remap_entity(self.source_colony_bits, map),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedAnomaly {
    pub id: String,
    pub name: String,
    pub description: String,
    pub discovered_at: i64,
}
impl SavedAnomaly {
    pub fn from_live(v: &Anomaly) -> Self {
        Self {
            id: v.id.clone(),
            name: v.name.clone(),
            description: v.description.clone(),
            discovered_at: v.discovered_at,
        }
    }
    pub fn into_live(self) -> Anomaly {
        Anomaly {
            id: self.id,
            name: self.name,
            description: self.description,
            discovered_at: self.discovered_at,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedAnomalies {
    pub discoveries: Vec<SavedAnomaly>,
}
impl SavedAnomalies {
    pub fn from_live(v: &Anomalies) -> Self {
        Self { discoveries: v.discoveries.iter().map(SavedAnomaly::from_live).collect() }
    }
    pub fn into_live(self) -> Anomalies {
        Anomalies { discoveries: self.discoveries.into_iter().map(SavedAnomaly::into_live).collect() }
    }
}

// ---------------------------------------------------------------------------
// Deep space
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedResourceCost {
    pub minerals: Amt,
    pub energy: Amt,
}
impl SavedResourceCost {
    pub fn from_live(v: &ResourceCost) -> Self {
        Self { minerals: v.minerals, energy: v.energy }
    }
    pub fn into_live(self) -> ResourceCost {
        ResourceCost { minerals: self.minerals, energy: self.energy }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDeepSpaceStructure {
    pub definition_id: String,
    pub name: String,
    pub owner: SavedOwner,
}
impl SavedDeepSpaceStructure {
    pub fn from_live(v: &DeepSpaceStructure) -> Self {
        Self {
            definition_id: v.definition_id.clone(),
            name: v.name.clone(),
            owner: SavedOwner::from_live(&v.owner),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> DeepSpaceStructure {
        DeepSpaceStructure {
            definition_id: self.definition_id,
            name: self.name,
            owner: self.owner.into_live(map),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SavedCommDirection {
    Bidirectional,
    OneWay,
}
impl From<&CommDirection> for SavedCommDirection {
    fn from(v: &CommDirection) -> Self {
        match v {
            CommDirection::Bidirectional => Self::Bidirectional,
            CommDirection::OneWay => Self::OneWay,
        }
    }
}
impl From<SavedCommDirection> for CommDirection {
    fn from(v: SavedCommDirection) -> Self {
        match v {
            SavedCommDirection::Bidirectional => Self::Bidirectional,
            SavedCommDirection::OneWay => Self::OneWay,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFTLCommRelay {
    pub paired_with_bits: u64,
    pub direction: SavedCommDirection,
}
impl SavedFTLCommRelay {
    pub fn from_live(v: &FTLCommRelay) -> Self {
        Self {
            paired_with_bits: v.paired_with.to_bits(),
            direction: (&v.direction).into(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> FTLCommRelay {
        FTLCommRelay {
            paired_with: remap_entity(self.paired_with_bits, map),
            direction: self.direction.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedStructureHitpoints {
    pub current: f64,
    pub max: f64,
}
impl SavedStructureHitpoints {
    pub fn from_live(v: &StructureHitpoints) -> Self {
        Self { current: v.current, max: v.max }
    }
    pub fn into_live(self) -> StructureHitpoints {
        StructureHitpoints { current: self.current, max: self.max }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedConstructionPlatform {
    pub target_id: Option<String>,
    pub accumulated: SavedResourceCost,
}
impl SavedConstructionPlatform {
    pub fn from_live(v: &ConstructionPlatform) -> Self {
        Self {
            target_id: v.target_id.clone(),
            accumulated: SavedResourceCost::from_live(&v.accumulated),
        }
    }
    pub fn into_live(self) -> ConstructionPlatform {
        ConstructionPlatform {
            target_id: self.target_id,
            accumulated: self.accumulated.into_live(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedScrapyard {
    pub remaining: SavedResourceCost,
    pub original_definition_id: String,
}
impl SavedScrapyard {
    pub fn from_live(v: &Scrapyard) -> Self {
        Self {
            remaining: SavedResourceCost::from_live(&v.remaining),
            original_definition_id: v.original_definition_id.clone(),
        }
    }
    pub fn into_live(self) -> Scrapyard {
        Scrapyard {
            remaining: self.remaining.into_live(),
            original_definition_id: self.original_definition_id,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedLifetimeCost {
    pub cost: SavedResourceCost,
}
impl SavedLifetimeCost {
    pub fn from_live(v: &LifetimeCost) -> Self {
        Self { cost: SavedResourceCost::from_live(&v.0) }
    }
    pub fn into_live(self) -> LifetimeCost {
        LifetimeCost(self.cost.into_live())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedForbiddenRegion {
    pub id: u64,
    pub type_id: String,
    pub spheres: Vec<([f64; 3], f64)>,
    pub threshold: f64,
    /// Capability keys only — params are reconstructed as default on load
    /// (region-type registry holds the canonical params).
    pub capabilities: Vec<String>,
}
impl SavedForbiddenRegion {
    pub fn from_live(v: &ForbiddenRegion) -> Self {
        Self {
            id: v.id,
            type_id: v.type_id.clone(),
            spheres: v.spheres.clone(),
            threshold: v.threshold,
            capabilities: v.capabilities.keys().cloned().collect(),
        }
    }
    pub fn into_live(self) -> ForbiddenRegion {
        let mut caps = HashMap::new();
        for k in self.capabilities {
            caps.insert(k, crate::galaxy::region::CapabilityParams::default());
        }
        ForbiddenRegion {
            id: self.id,
            type_id: self.type_id,
            spheres: self.spheres,
            threshold: self.threshold,
            capabilities: caps,
        }
    }
}

// ---------------------------------------------------------------------------
// Knowledge / Fact pipeline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SavedObservationSource {
    Direct,
    Relay,
    Scout,
    Stale,
}
impl From<&ObservationSource> for SavedObservationSource {
    fn from(v: &ObservationSource) -> Self {
        match v {
            ObservationSource::Direct => Self::Direct,
            ObservationSource::Relay => Self::Relay,
            ObservationSource::Scout => Self::Scout,
            ObservationSource::Stale => Self::Stale,
        }
    }
}
impl From<SavedObservationSource> for ObservationSource {
    fn from(v: SavedObservationSource) -> Self {
        match v {
            SavedObservationSource::Direct => Self::Direct,
            SavedObservationSource::Relay => Self::Relay,
            SavedObservationSource::Scout => Self::Scout,
            SavedObservationSource::Stale => Self::Stale,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedSystemSnapshot {
    pub name: String,
    pub position: [f64; 3],
    pub surveyed: bool,
    pub colonized: bool,
    pub population: f64,
    pub production: f64,
    pub minerals: Amt,
    pub energy: Amt,
    pub food: Amt,
    pub authority: Amt,
    pub has_hostile: bool,
    pub hostile_strength: f64,
    pub has_port: bool,
    pub has_shipyard: bool,
    pub habitability: Option<f64>,
    pub mineral_richness: Option<f64>,
    pub energy_potential: Option<f64>,
    pub research_potential: Option<f64>,
    pub max_building_slots: Option<u8>,
    pub production_minerals: Amt,
    pub production_energy: Amt,
    pub production_food: Amt,
    pub production_research: Amt,
    pub maintenance_energy: Amt,
}

impl SavedSystemSnapshot {
    pub fn from_live(v: &SystemSnapshot) -> Self {
        Self {
            name: v.name.clone(),
            position: v.position,
            surveyed: v.surveyed,
            colonized: v.colonized,
            population: v.population,
            production: v.production,
            minerals: v.minerals,
            energy: v.energy,
            food: v.food,
            authority: v.authority,
            has_hostile: v.has_hostile,
            hostile_strength: v.hostile_strength,
            has_port: v.has_port,
            has_shipyard: v.has_shipyard,
            habitability: v.habitability,
            mineral_richness: v.mineral_richness,
            energy_potential: v.energy_potential,
            research_potential: v.research_potential,
            max_building_slots: v.max_building_slots,
            production_minerals: v.production_minerals,
            production_energy: v.production_energy,
            production_food: v.production_food,
            production_research: v.production_research,
            maintenance_energy: v.maintenance_energy,
        }
    }
    pub fn into_live(self) -> SystemSnapshot {
        SystemSnapshot {
            name: self.name,
            position: self.position,
            surveyed: self.surveyed,
            colonized: self.colonized,
            population: self.population,
            production: self.production,
            minerals: self.minerals,
            energy: self.energy,
            food: self.food,
            authority: self.authority,
            has_hostile: self.has_hostile,
            hostile_strength: self.hostile_strength,
            has_port: self.has_port,
            has_shipyard: self.has_shipyard,
            habitability: self.habitability,
            mineral_richness: self.mineral_richness,
            energy_potential: self.energy_potential,
            research_potential: self.research_potential,
            max_building_slots: self.max_building_slots,
            production_minerals: self.production_minerals,
            production_energy: self.production_energy,
            production_food: self.production_food,
            production_research: self.production_research,
            maintenance_energy: self.maintenance_energy,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSystemKnowledge {
    pub system_bits: u64,
    pub observed_at: i64,
    pub received_at: i64,
    pub data: SavedSystemSnapshot,
    pub source: SavedObservationSource,
}
impl SavedSystemKnowledge {
    pub fn from_live(v: &SystemKnowledge) -> Self {
        Self {
            system_bits: v.system.to_bits(),
            observed_at: v.observed_at,
            received_at: v.received_at,
            data: SavedSystemSnapshot::from_live(&v.data),
            source: (&v.source).into(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> SystemKnowledge {
        SystemKnowledge {
            system: remap_entity(self.system_bits, map),
            observed_at: self.observed_at,
            received_at: self.received_at,
            data: self.data.into_live(),
            source: self.source.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedShipSnapshotState {
    Docked,
    InTransit,
    Surveying,
    Settling,
    Refitting,
    Destroyed,
    Loitering { position: [f64; 3] },
}
impl From<&ShipSnapshotState> for SavedShipSnapshotState {
    fn from(v: &ShipSnapshotState) -> Self {
        match v {
            ShipSnapshotState::Docked => Self::Docked,
            ShipSnapshotState::InTransit => Self::InTransit,
            ShipSnapshotState::Surveying => Self::Surveying,
            ShipSnapshotState::Settling => Self::Settling,
            ShipSnapshotState::Refitting => Self::Refitting,
            ShipSnapshotState::Destroyed => Self::Destroyed,
            ShipSnapshotState::Loitering { position } => Self::Loitering { position: *position },
        }
    }
}
impl From<SavedShipSnapshotState> for ShipSnapshotState {
    fn from(v: SavedShipSnapshotState) -> Self {
        match v {
            SavedShipSnapshotState::Docked => Self::Docked,
            SavedShipSnapshotState::InTransit => Self::InTransit,
            SavedShipSnapshotState::Surveying => Self::Surveying,
            SavedShipSnapshotState::Settling => Self::Settling,
            SavedShipSnapshotState::Refitting => Self::Refitting,
            SavedShipSnapshotState::Destroyed => Self::Destroyed,
            SavedShipSnapshotState::Loitering { position } => Self::Loitering { position },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedShipSnapshot {
    pub entity_bits: u64,
    pub name: String,
    pub design_id: String,
    pub last_known_state: SavedShipSnapshotState,
    pub last_known_system_bits: Option<u64>,
    pub observed_at: i64,
    pub hp: f64,
    pub hp_max: f64,
    pub source: SavedObservationSource,
}
impl SavedShipSnapshot {
    pub fn from_live(v: &ShipSnapshot) -> Self {
        Self {
            entity_bits: v.entity.to_bits(),
            name: v.name.clone(),
            design_id: v.design_id.clone(),
            last_known_state: (&v.last_known_state).into(),
            last_known_system_bits: v.last_known_system.map(|e| e.to_bits()),
            observed_at: v.observed_at,
            hp: v.hp,
            hp_max: v.hp_max,
            source: (&v.source).into(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> ShipSnapshot {
        ShipSnapshot {
            entity: remap_entity(self.entity_bits, map),
            name: self.name,
            design_id: self.design_id,
            last_known_state: self.last_known_state.into(),
            last_known_system: self.last_known_system_bits.map(|b| remap_entity(b, map)),
            observed_at: self.observed_at,
            hp: self.hp,
            hp_max: self.hp_max,
            source: self.source.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedKnowledgeStore {
    pub entries: Vec<SavedSystemKnowledge>,
    pub ship_snapshots: Vec<SavedShipSnapshot>,
}

impl SavedKnowledgeStore {
    pub fn from_live(v: &KnowledgeStore) -> Self {
        Self {
            entries: v.iter().map(|(_, k)| SavedSystemKnowledge::from_live(k)).collect(),
            ship_snapshots: v.iter_ships().map(|(_, s)| SavedShipSnapshot::from_live(s)).collect(),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> KnowledgeStore {
        let mut store = KnowledgeStore::default();
        for entry in self.entries {
            store.update(entry.into_live(map));
        }
        for ship in self.ship_snapshots {
            store.update_ship(ship.into_live(map));
        }
        store
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SavedCombatVictor {
    Player,
    Hostile,
}
impl From<&CombatVictor> for SavedCombatVictor {
    fn from(v: &CombatVictor) -> Self {
        match v {
            CombatVictor::Player => Self::Player,
            CombatVictor::Hostile => Self::Hostile,
        }
    }
}
impl From<SavedCombatVictor> for CombatVictor {
    fn from(v: SavedCombatVictor) -> Self {
        match v {
            SavedCombatVictor::Player => Self::Player,
            SavedCombatVictor::Hostile => Self::Hostile,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedKnowledgeFact {
    HostileDetected {
        target_bits: u64,
        detector_bits: u64,
        target_pos: [f64; 3],
        description: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
    CombatOutcome {
        system_bits: u64,
        victor: SavedCombatVictor,
        detail: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
    SurveyComplete {
        system_bits: u64,
        system_name: String,
        detail: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
    AnomalyDiscovered {
        system_bits: u64,
        anomaly_id: String,
        detail: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
    SurveyDiscovery {
        system_bits: u64,
        detail: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
    StructureBuilt {
        system_bits: Option<u64>,
        kind: String,
        name: String,
        destroyed: bool,
        detail: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
    ColonyEstablished {
        system_bits: u64,
        planet_bits: u64,
        name: String,
        detail: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
    ColonyFailed {
        system_bits: u64,
        name: String,
        reason: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
    ShipArrived {
        system_bits: Option<u64>,
        name: String,
        detail: String,
        #[serde(default)]
        event_id: Option<u64>,
    },
}

impl SavedKnowledgeFact {
    pub fn from_live(v: &KnowledgeFact) -> Self {
        match v {
            KnowledgeFact::HostileDetected { event_id, target, detector, target_pos, description } => Self::HostileDetected {
                target_bits: target.to_bits(),
                detector_bits: detector.to_bits(),
                target_pos: *target_pos,
                description: description.clone(),
                event_id: event_id.map(|e| e.0),
            },
            KnowledgeFact::CombatOutcome { event_id, system, victor, detail } => Self::CombatOutcome {
                system_bits: system.to_bits(),
                victor: victor.into(),
                detail: detail.clone(),
                event_id: event_id.map(|e| e.0),
            },
            KnowledgeFact::SurveyComplete { event_id, system, system_name, detail } => Self::SurveyComplete {
                system_bits: system.to_bits(),
                system_name: system_name.clone(),
                detail: detail.clone(),
                event_id: event_id.map(|e| e.0),
            },
            KnowledgeFact::AnomalyDiscovered { event_id, system, anomaly_id, detail } => Self::AnomalyDiscovered {
                system_bits: system.to_bits(),
                anomaly_id: anomaly_id.clone(),
                detail: detail.clone(),
                event_id: event_id.map(|e| e.0),
            },
            KnowledgeFact::SurveyDiscovery { event_id, system, detail } => Self::SurveyDiscovery {
                system_bits: system.to_bits(),
                detail: detail.clone(),
                event_id: event_id.map(|e| e.0),
            },
            KnowledgeFact::StructureBuilt { event_id, system, kind, name, destroyed, detail } => Self::StructureBuilt {
                system_bits: system.map(|e| e.to_bits()),
                kind: kind.clone(),
                name: name.clone(),
                destroyed: *destroyed,
                detail: detail.clone(),
                event_id: event_id.map(|e| e.0),
            },
            KnowledgeFact::ColonyEstablished { event_id, system, planet, name, detail } => Self::ColonyEstablished {
                system_bits: system.to_bits(),
                planet_bits: planet.to_bits(),
                name: name.clone(),
                detail: detail.clone(),
                event_id: event_id.map(|e| e.0),
            },
            KnowledgeFact::ColonyFailed { event_id, system, name, reason } => Self::ColonyFailed {
                system_bits: system.to_bits(),
                name: name.clone(),
                reason: reason.clone(),
                event_id: event_id.map(|e| e.0),
            },
            KnowledgeFact::ShipArrived { event_id, system, name, detail } => Self::ShipArrived {
                system_bits: system.map(|e| e.to_bits()),
                name: name.clone(),
                detail: detail.clone(),
                event_id: event_id.map(|e| e.0),
            },
        }
    }
    pub fn into_live(self, map: &EntityMap) -> KnowledgeFact {
        use crate::knowledge::EventId;
        match self {
            Self::HostileDetected { target_bits, detector_bits, target_pos, description, event_id } => KnowledgeFact::HostileDetected {
                event_id: event_id.map(EventId),
                target: remap_entity(target_bits, map),
                detector: remap_entity(detector_bits, map),
                target_pos,
                description,
            },
            Self::CombatOutcome { system_bits, victor, detail, event_id } => KnowledgeFact::CombatOutcome {
                event_id: event_id.map(EventId),
                system: remap_entity(system_bits, map),
                victor: victor.into(),
                detail,
            },
            Self::SurveyComplete { system_bits, system_name, detail, event_id } => KnowledgeFact::SurveyComplete {
                event_id: event_id.map(EventId),
                system: remap_entity(system_bits, map),
                system_name,
                detail,
            },
            Self::AnomalyDiscovered { system_bits, anomaly_id, detail, event_id } => KnowledgeFact::AnomalyDiscovered {
                event_id: event_id.map(EventId),
                system: remap_entity(system_bits, map),
                anomaly_id,
                detail,
            },
            Self::SurveyDiscovery { system_bits, detail, event_id } => KnowledgeFact::SurveyDiscovery {
                event_id: event_id.map(EventId),
                system: remap_entity(system_bits, map),
                detail,
            },
            Self::StructureBuilt { system_bits, kind, name, destroyed, detail, event_id } => KnowledgeFact::StructureBuilt {
                event_id: event_id.map(EventId),
                system: system_bits.map(|b| remap_entity(b, map)),
                kind, name, destroyed, detail,
            },
            Self::ColonyEstablished { system_bits, planet_bits, name, detail, event_id } => KnowledgeFact::ColonyEstablished {
                event_id: event_id.map(EventId),
                system: remap_entity(system_bits, map),
                planet: remap_entity(planet_bits, map),
                name, detail,
            },
            Self::ColonyFailed { system_bits, name, reason, event_id } => KnowledgeFact::ColonyFailed {
                event_id: event_id.map(EventId),
                system: remap_entity(system_bits, map),
                name, reason,
            },
            Self::ShipArrived { system_bits, name, detail, event_id } => KnowledgeFact::ShipArrived {
                event_id: event_id.map(EventId),
                system: system_bits.map(|b| remap_entity(b, map)),
                name, detail,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPerceivedFact {
    pub fact: SavedKnowledgeFact,
    pub observed_at: i64,
    pub arrives_at: i64,
    pub source: SavedObservationSource,
    pub origin_pos: [f64; 3],
    pub related_system_bits: Option<u64>,
}

impl SavedPerceivedFact {
    pub fn from_live(v: &PerceivedFact) -> Self {
        Self {
            fact: SavedKnowledgeFact::from_live(&v.fact),
            observed_at: v.observed_at,
            arrives_at: v.arrives_at,
            source: (&v.source).into(),
            origin_pos: v.origin_pos,
            related_system_bits: v.related_system.map(|e| e.to_bits()),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> PerceivedFact {
        PerceivedFact {
            fact: self.fact.into_live(map),
            observed_at: self.observed_at,
            arrives_at: self.arrives_at,
            source: self.source.into(),
            origin_pos: self.origin_pos,
            related_system: self.related_system_bits.map(|b| remap_entity(b, map)),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedPendingFactQueue {
    pub facts: Vec<SavedPerceivedFact>,
}

impl SavedPendingFactQueue {
    pub fn from_live(v: &PendingFactQueue) -> Self {
        Self { facts: v.facts.iter().map(SavedPerceivedFact::from_live).collect() }
    }
    pub fn into_live(self, map: &EntityMap) -> PendingFactQueue {
        PendingFactQueue {
            facts: self.facts.into_iter().map(|f| f.into_live(map)).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCommsParams {
    pub empire_relay_range: ModifiedValue,
    pub empire_relay_inv_latency: ModifiedValue,
    pub fleet_relay_range: ModifiedValue,
    pub fleet_relay_inv_latency: ModifiedValue,
}
impl SavedCommsParams {
    pub fn from_live(v: &CommsParams) -> Self {
        Self {
            empire_relay_range: v.empire_relay_range.clone(),
            empire_relay_inv_latency: v.empire_relay_inv_latency.clone(),
            fleet_relay_range: v.fleet_relay_range.clone(),
            fleet_relay_inv_latency: v.fleet_relay_inv_latency.clone(),
        }
    }
    pub fn into_live(self) -> CommsParams {
        CommsParams {
            empire_relay_range: self.empire_relay_range,
            empire_relay_inv_latency: self.empire_relay_inv_latency,
            fleet_relay_range: self.fleet_relay_range,
            fleet_relay_inv_latency: self.fleet_relay_inv_latency,
        }
    }
}

// ---------------------------------------------------------------------------
// Pending command / communication
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedDiplomaticAction {
    DeclareWar,
    ProposePeace,
    ProposeAlliance,
    BreakAlliance,
    AcceptPeace,
    AcceptAlliance,
    CustomAction(String),
}
impl From<&DiplomaticAction> for SavedDiplomaticAction {
    fn from(v: &DiplomaticAction) -> Self {
        match v {
            DiplomaticAction::DeclareWar => Self::DeclareWar,
            DiplomaticAction::ProposePeace => Self::ProposePeace,
            DiplomaticAction::ProposeAlliance => Self::ProposeAlliance,
            DiplomaticAction::BreakAlliance => Self::BreakAlliance,
            DiplomaticAction::AcceptPeace => Self::AcceptPeace,
            DiplomaticAction::AcceptAlliance => Self::AcceptAlliance,
            DiplomaticAction::CustomAction(s) => Self::CustomAction(s.clone()),
        }
    }
}
impl From<SavedDiplomaticAction> for DiplomaticAction {
    fn from(v: SavedDiplomaticAction) -> Self {
        match v {
            SavedDiplomaticAction::DeclareWar => Self::DeclareWar,
            SavedDiplomaticAction::ProposePeace => Self::ProposePeace,
            SavedDiplomaticAction::ProposeAlliance => Self::ProposeAlliance,
            SavedDiplomaticAction::BreakAlliance => Self::BreakAlliance,
            SavedDiplomaticAction::AcceptPeace => Self::AcceptPeace,
            SavedDiplomaticAction::AcceptAlliance => Self::AcceptAlliance,
            SavedDiplomaticAction::CustomAction(s) => Self::CustomAction(s),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPendingDiplomaticAction {
    pub from_bits: u64,
    pub to_bits: u64,
    pub action: SavedDiplomaticAction,
    pub arrives_at: i64,
    pub one_way_delay_hexadies: i64,
}
impl SavedPendingDiplomaticAction {
    pub fn from_live(v: &PendingDiplomaticAction) -> Self {
        Self {
            from_bits: v.from.to_bits(),
            to_bits: v.to.to_bits(),
            action: (&v.action).into(),
            arrives_at: v.arrives_at,
            one_way_delay_hexadies: v.one_way_delay_hexadies,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> PendingDiplomaticAction {
        PendingDiplomaticAction {
            from: remap_entity(self.from_bits, map),
            to: remap_entity(self.to_bits, map),
            action: self.action.into(),
            arrives_at: self.arrives_at,
            one_way_delay_hexadies: self.one_way_delay_hexadies,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedRemoteCommand {
    BuildShip { design_id: String },
    SetProductionFocus { minerals: f64, energy: f64, research: f64 },
    Colony(SavedColonyCommand),
}
impl From<&RemoteCommand> for SavedRemoteCommand {
    fn from(v: &RemoteCommand) -> Self {
        match v {
            RemoteCommand::BuildShip { design_id } => Self::BuildShip { design_id: design_id.clone() },
            RemoteCommand::SetProductionFocus { minerals, energy, research } => Self::SetProductionFocus {
                minerals: *minerals, energy: *energy, research: *research,
            },
            RemoteCommand::Colony(cc) => Self::Colony(SavedColonyCommand::from_live(cc)),
        }
    }
}
impl SavedRemoteCommand {
    pub fn into_live(self, map: &EntityMap) -> RemoteCommand {
        match self {
            SavedRemoteCommand::BuildShip { design_id } => RemoteCommand::BuildShip { design_id },
            SavedRemoteCommand::SetProductionFocus { minerals, energy, research } => {
                RemoteCommand::SetProductionFocus { minerals, energy, research }
            }
            SavedRemoteCommand::Colony(sc) => RemoteCommand::Colony(sc.into_live(map)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedColonyCommand {
    pub target_planet_bits: Option<u64>,
    pub kind: SavedColonyCommandKind,
}

impl SavedColonyCommand {
    pub fn from_live(v: &ColonyCommand) -> Self {
        Self {
            target_planet_bits: v.target_planet.map(|e| e.to_bits()),
            kind: SavedColonyCommandKind::from_live(&v.kind),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> ColonyCommand {
        ColonyCommand {
            target_planet: self.target_planet_bits.map(|b| remap_entity(b, map)),
            kind: self.kind.into_live(map),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SavedColonyCommandKind {
    QueueBuilding { building_id: String, target_slot: usize },
    DemolishBuilding { target_slot: usize },
    UpgradeBuilding { slot_index: usize, target_id: String },
    QueueShipBuild {
        host_colony_bits: u64,
        design_id: String,
        build_kind: SavedBuildKind,
    },
    QueueDeliverableBuild {
        host_colony_bits: u64,
        def_id: String,
        display_name: String,
        cargo_size: u32,
        minerals_cost: Amt,
        energy_cost: Amt,
        build_time: i64,
    },
}

impl SavedColonyCommandKind {
    pub fn from_live(v: &ColonyCommandKind) -> Self {
        match v {
            ColonyCommandKind::QueueBuilding { building_id, target_slot } => Self::QueueBuilding {
                building_id: building_id.clone(),
                target_slot: *target_slot,
            },
            ColonyCommandKind::DemolishBuilding { target_slot } => {
                Self::DemolishBuilding { target_slot: *target_slot }
            }
            ColonyCommandKind::UpgradeBuilding { slot_index, target_id } => Self::UpgradeBuilding {
                slot_index: *slot_index,
                target_id: target_id.clone(),
            },
            ColonyCommandKind::QueueShipBuild { host_colony, design_id, build_kind } => {
                Self::QueueShipBuild {
                    host_colony_bits: host_colony.to_bits(),
                    design_id: design_id.clone(),
                    build_kind: build_kind.into(),
                }
            }
            ColonyCommandKind::QueueDeliverableBuild {
                host_colony,
                def_id,
                display_name,
                cargo_size,
                minerals_cost,
                energy_cost,
                build_time,
            } => Self::QueueDeliverableBuild {
                host_colony_bits: host_colony.to_bits(),
                def_id: def_id.clone(),
                display_name: display_name.clone(),
                cargo_size: *cargo_size,
                minerals_cost: *minerals_cost,
                energy_cost: *energy_cost,
                build_time: *build_time,
            },
        }
    }
    pub fn into_live(self, map: &EntityMap) -> ColonyCommandKind {
        match self {
            Self::QueueBuilding { building_id, target_slot } => {
                ColonyCommandKind::QueueBuilding { building_id, target_slot }
            }
            Self::DemolishBuilding { target_slot } => {
                ColonyCommandKind::DemolishBuilding { target_slot }
            }
            Self::UpgradeBuilding { slot_index, target_id } => {
                ColonyCommandKind::UpgradeBuilding { slot_index, target_id }
            }
            Self::QueueShipBuild { host_colony_bits, design_id, build_kind } => {
                ColonyCommandKind::QueueShipBuild {
                    host_colony: remap_entity(host_colony_bits, map),
                    design_id,
                    build_kind: build_kind.into(),
                }
            }
            Self::QueueDeliverableBuild {
                host_colony_bits,
                def_id,
                display_name,
                cargo_size,
                minerals_cost,
                energy_cost,
                build_time,
            } => ColonyCommandKind::QueueDeliverableBuild {
                host_colony: remap_entity(host_colony_bits, map),
                def_id,
                display_name,
                cargo_size,
                minerals_cost,
                energy_cost,
                build_time,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPendingCommand {
    pub target_system_bits: u64,
    pub command: SavedRemoteCommand,
    pub sent_at: i64,
    pub arrives_at: i64,
    pub origin_pos: [f64; 3],
    pub destination_pos: [f64; 3],
}
impl SavedPendingCommand {
    pub fn from_live(v: &PendingCommand) -> Self {
        Self {
            target_system_bits: v.target_system.to_bits(),
            command: (&v.command).into(),
            sent_at: v.sent_at,
            arrives_at: v.arrives_at,
            origin_pos: v.origin_pos,
            destination_pos: v.destination_pos,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> PendingCommand {
        PendingCommand {
            target_system: remap_entity(self.target_system_bits, map),
            command: self.command.into_live(map),
            sent_at: self.sent_at,
            arrives_at: self.arrives_at,
            origin_pos: self.origin_pos,
            destination_pos: self.destination_pos,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCommandLogEntry {
    pub description: String,
    pub sent_at: i64,
    pub arrives_at: i64,
    pub arrived: bool,
}
impl SavedCommandLogEntry {
    pub fn from_live(v: &CommandLogEntry) -> Self {
        Self {
            description: v.description.clone(),
            sent_at: v.sent_at,
            arrives_at: v.arrives_at,
            arrived: v.arrived,
        }
    }
    pub fn into_live(self) -> CommandLogEntry {
        CommandLogEntry {
            description: self.description,
            sent_at: self.sent_at,
            arrives_at: self.arrives_at,
            arrived: self.arrived,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedCommandLog {
    pub entries: Vec<SavedCommandLogEntry>,
}
impl SavedCommandLog {
    pub fn from_live(v: &CommandLog) -> Self {
        Self { entries: v.entries.iter().map(SavedCommandLogEntry::from_live).collect() }
    }
    pub fn into_live(self) -> CommandLog {
        CommandLog { entries: self.entries.into_iter().map(SavedCommandLogEntry::into_live).collect() }
    }
}

// ---------------------------------------------------------------------------
// Technology
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedTechTree {
    /// Researched tech ids; full Technology definitions are reloaded from Lua.
    pub researched: Vec<String>,
}
impl SavedTechTree {
    pub fn from_live(v: &TechTree) -> Self {
        Self { researched: v.researched.iter().map(|t| t.0.clone()).collect() }
    }
    /// Merge into an existing TechTree (preserves the tree's `technologies`
    /// field which was populated from Lua scripts at startup).
    pub fn apply_to(self, tree: &mut TechTree) {
        tree.researched.clear();
        for id in self.researched {
            tree.researched.insert(TechId(id));
        }
    }
    /// Build a fresh TechTree containing only the researched set (no Lua
    /// reload). Useful when no live tree exists yet.
    pub fn into_live_minimal(self) -> TechTree {
        let mut tree = TechTree::default();
        self.apply_to(&mut tree);
        tree
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedResearchQueue {
    pub current: Option<String>,
    pub accumulated: f64,
    pub blocked: bool,
}
impl SavedResearchQueue {
    pub fn from_live(v: &ResearchQueue) -> Self {
        Self {
            current: v.current.as_ref().map(|t| t.0.clone()),
            accumulated: v.accumulated,
            blocked: v.blocked,
        }
    }
    pub fn into_live(self) -> ResearchQueue {
        ResearchQueue {
            current: self.current.map(TechId),
            accumulated: self.accumulated,
            blocked: self.blocked,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedResearchPool {
    pub points: f64,
}
impl SavedResearchPool {
    pub fn from_live(v: &ResearchPool) -> Self {
        Self { points: v.points }
    }
    pub fn into_live(self) -> ResearchPool {
        ResearchPool { points: self.points }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedRecentlyResearched {
    pub techs: Vec<String>,
}
impl SavedRecentlyResearched {
    pub fn from_live(v: &RecentlyResearched) -> Self {
        Self { techs: v.techs.iter().map(|t| t.0.clone()).collect() }
    }
    pub fn into_live(self) -> RecentlyResearched {
        RecentlyResearched { techs: self.techs.into_iter().map(TechId).collect() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPendingResearch {
    pub amount: f64,
    pub arrives_at: i64,
}
impl SavedPendingResearch {
    pub fn from_live(v: &PendingResearch) -> Self {
        Self { amount: v.amount, arrives_at: v.arrives_at }
    }
    pub fn into_live(self) -> PendingResearch {
        PendingResearch { amount: self.amount, arrives_at: self.arrives_at }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedTechKnowledge {
    pub known_techs: Vec<String>,
}
impl SavedTechKnowledge {
    pub fn from_live(v: &TechKnowledge) -> Self {
        Self { known_techs: v.known_techs.iter().map(|t| t.0.clone()).collect() }
    }
    pub fn into_live(self) -> TechKnowledge {
        TechKnowledge {
            known_techs: self.known_techs.into_iter().map(TechId).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPendingKnowledgePropagation {
    pub tech_id: String,
    pub target_system_bits: u64,
    pub arrives_at: i64,
}
impl SavedPendingKnowledgePropagation {
    pub fn from_live(v: &PendingKnowledgePropagation) -> Self {
        Self {
            tech_id: v.tech_id.0.clone(),
            target_system_bits: v.target_system.to_bits(),
            arrives_at: v.arrives_at,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> PendingKnowledgePropagation {
        PendingKnowledgePropagation {
            tech_id: TechId(self.tech_id),
            target_system: remap_entity(self.target_system_bits, map),
            arrives_at: self.arrives_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedParsedModifier {
    pub target: String,
    pub base_add: f64,
    pub multiplier: f64,
    pub add: f64,
}
impl SavedParsedModifier {
    pub fn from_live(v: &crate::modifier::ParsedModifier) -> Self {
        Self {
            target: v.target.clone(),
            base_add: v.base_add,
            multiplier: v.multiplier,
            add: v.add,
        }
    }
    pub fn into_live(self) -> crate::modifier::ParsedModifier {
        crate::modifier::ParsedModifier {
            target: self.target,
            base_add: self.base_add,
            multiplier: self.multiplier,
            add: self.add,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedPendingColonyTechModifiers {
    pub entries: Vec<(String, SavedParsedModifier)>,
}
impl SavedPendingColonyTechModifiers {
    pub fn from_live(v: &PendingColonyTechModifiers) -> Self {
        Self {
            entries: v.entries.iter().map(|(t, pm)| (t.0.clone(), SavedParsedModifier::from_live(pm))).collect(),
        }
    }
    pub fn into_live(self) -> PendingColonyTechModifiers {
        PendingColonyTechModifiers {
            entries: self.entries.into_iter().map(|(t, pm)| (TechId(t), pm.into_live())).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedEmpireModifiers {
    pub population_growth: ModifiedValue,
}
impl SavedEmpireModifiers {
    pub fn from_live(v: &EmpireModifiers) -> Self {
        Self { population_growth: v.population_growth.clone() }
    }
    pub fn into_live(self) -> EmpireModifiers {
        EmpireModifiers { population_growth: self.population_growth }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedGameFlags {
    pub flags: Vec<String>,
}
impl SavedGameFlags {
    pub fn from_live(v: &GameFlags) -> Self {
        Self { flags: v.flags.iter().cloned().collect() }
    }
    pub fn into_live(self) -> GameFlags {
        GameFlags { flags: self.flags.into_iter().collect() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedScopedFlags {
    pub flags: Vec<String>,
}
impl SavedScopedFlags {
    pub fn from_live(v: &ScopedFlags) -> Self {
        Self { flags: v.flags.iter().cloned().collect() }
    }
    pub fn into_live(self) -> ScopedFlags {
        ScopedFlags { flags: self.flags.into_iter().collect() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGlobalParams {
    pub sublight_speed_bonus: f64,
    pub ftl_speed_multiplier: f64,
    pub ftl_range_bonus: f64,
    pub survey_range_bonus: f64,
    pub build_speed_multiplier: f64,
}
impl SavedGlobalParams {
    pub fn from_live(v: &GlobalParams) -> Self {
        Self {
            sublight_speed_bonus: v.sublight_speed_bonus,
            ftl_speed_multiplier: v.ftl_speed_multiplier,
            ftl_range_bonus: v.ftl_range_bonus,
            survey_range_bonus: v.survey_range_bonus,
            build_speed_multiplier: v.build_speed_multiplier,
        }
    }
    pub fn into_live(self) -> GlobalParams {
        GlobalParams {
            sublight_speed_bonus: self.sublight_speed_bonus,
            ftl_speed_multiplier: self.ftl_speed_multiplier,
            ftl_range_bonus: self.ftl_range_bonus,
            survey_range_bonus: self.survey_range_bonus,
            build_speed_multiplier: self.build_speed_multiplier,
        }
    }
}

// ---------------------------------------------------------------------------
// Events / Notifications
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SavedGameEventKind {
    ShipArrived,
    SurveyComplete,
    SurveyDiscovery,
    ColonyEstablished,
    ShipBuilt,
    BuildingDemolished,
    CombatVictory,
    CombatDefeat,
    HostileDetected,
    ShipScrapped,
    ResourceAlert,
    PlayerRespawn,
    ColonyFailed,
    AnomalyDiscovered,
}
impl From<&GameEventKind> for SavedGameEventKind {
    fn from(v: &GameEventKind) -> Self {
        match v {
            GameEventKind::ShipArrived => Self::ShipArrived,
            GameEventKind::SurveyComplete => Self::SurveyComplete,
            GameEventKind::SurveyDiscovery => Self::SurveyDiscovery,
            GameEventKind::ColonyEstablished => Self::ColonyEstablished,
            GameEventKind::ShipBuilt => Self::ShipBuilt,
            GameEventKind::BuildingDemolished => Self::BuildingDemolished,
            GameEventKind::CombatVictory => Self::CombatVictory,
            GameEventKind::CombatDefeat => Self::CombatDefeat,
            GameEventKind::HostileDetected => Self::HostileDetected,
            GameEventKind::ShipScrapped => Self::ShipScrapped,
            GameEventKind::ResourceAlert => Self::ResourceAlert,
            GameEventKind::PlayerRespawn => Self::PlayerRespawn,
            GameEventKind::ColonyFailed => Self::ColonyFailed,
            GameEventKind::AnomalyDiscovered => Self::AnomalyDiscovered,
        }
    }
}
impl From<SavedGameEventKind> for GameEventKind {
    fn from(v: SavedGameEventKind) -> Self {
        match v {
            SavedGameEventKind::ShipArrived => Self::ShipArrived,
            SavedGameEventKind::SurveyComplete => Self::SurveyComplete,
            SavedGameEventKind::SurveyDiscovery => Self::SurveyDiscovery,
            SavedGameEventKind::ColonyEstablished => Self::ColonyEstablished,
            SavedGameEventKind::ShipBuilt => Self::ShipBuilt,
            SavedGameEventKind::BuildingDemolished => Self::BuildingDemolished,
            SavedGameEventKind::CombatVictory => Self::CombatVictory,
            SavedGameEventKind::CombatDefeat => Self::CombatDefeat,
            SavedGameEventKind::HostileDetected => Self::HostileDetected,
            SavedGameEventKind::ShipScrapped => Self::ShipScrapped,
            SavedGameEventKind::ResourceAlert => Self::ResourceAlert,
            SavedGameEventKind::PlayerRespawn => Self::PlayerRespawn,
            SavedGameEventKind::ColonyFailed => Self::ColonyFailed,
            SavedGameEventKind::AnomalyDiscovered => Self::AnomalyDiscovered,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGameEvent {
    pub timestamp: i64,
    pub kind: SavedGameEventKind,
    pub description: String,
    pub related_system_bits: Option<u64>,
    /// #249: Optional for backward compatibility with pre-migration saves.
    /// Deserialized `None` maps to `EventId::default()` on load.
    #[serde(default)]
    pub event_id: Option<u64>,
}
impl SavedGameEvent {
    pub fn from_live(v: &GameEvent) -> Self {
        Self {
            timestamp: v.timestamp,
            kind: (&v.kind).into(),
            description: v.description.clone(),
            related_system_bits: v.related_system.map(|e| e.to_bits()),
            event_id: Some(v.id.0),
        }
    }
    pub fn into_live(self, map: &EntityMap) -> GameEvent {
        GameEvent {
            id: crate::knowledge::EventId(self.event_id.unwrap_or(0)),
            timestamp: self.timestamp,
            kind: self.kind.into(),
            description: self.description,
            related_system: self.related_system_bits.map(|b| remap_entity(b, map)),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedEventLog {
    pub entries: Vec<SavedGameEvent>,
    pub max_entries: usize,
}
impl SavedEventLog {
    pub fn from_live(v: &EventLog) -> Self {
        Self {
            entries: v.entries.iter().map(SavedGameEvent::from_live).collect(),
            max_entries: v.max_entries,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> EventLog {
        EventLog {
            entries: self.entries.into_iter().map(|e| e.into_live(map)).collect(),
            max_entries: if self.max_entries == 0 { 50 } else { self.max_entries },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SavedNotificationPriority {
    Low,
    Medium,
    High,
}
impl From<&NotificationPriority> for SavedNotificationPriority {
    fn from(v: &NotificationPriority) -> Self {
        match v {
            NotificationPriority::Low => Self::Low,
            NotificationPriority::Medium => Self::Medium,
            NotificationPriority::High => Self::High,
        }
    }
}
impl From<SavedNotificationPriority> for NotificationPriority {
    fn from(v: SavedNotificationPriority) -> Self {
        match v {
            SavedNotificationPriority::Low => Self::Low,
            SavedNotificationPriority::Medium => Self::Medium,
            SavedNotificationPriority::High => Self::High,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedNotification {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub icon: Option<String>,
    pub priority: SavedNotificationPriority,
    pub target_system_bits: Option<u64>,
    pub remaining_seconds: Option<f32>,
}
impl SavedNotification {
    pub fn from_live(v: &Notification) -> Self {
        Self {
            id: v.id,
            title: v.title.clone(),
            description: v.description.clone(),
            icon: v.icon.clone(),
            priority: (&v.priority).into(),
            target_system_bits: v.target_system.map(|e| e.to_bits()),
            remaining_seconds: v.remaining_seconds,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> Notification {
        Notification {
            id: self.id,
            title: self.title,
            description: self.description,
            icon: self.icon,
            priority: self.priority.into(),
            target_system: self.target_system_bits.map(|b| remap_entity(b, map)),
            remaining_seconds: self.remaining_seconds,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedNotificationQueue {
    pub items: Vec<SavedNotification>,
    pub max_items: usize,
}
impl SavedNotificationQueue {
    pub fn from_live(v: &NotificationQueue) -> Self {
        Self {
            items: v.items.iter().map(SavedNotification::from_live).collect(),
            max_items: v.max_items,
        }
    }
    pub fn into_live(self, map: &EntityMap) -> NotificationQueue {
        let mut q = NotificationQueue::new();
        if self.max_items > 0 {
            q.max_items = self.max_items;
        }
        // Preserve order by inserting from oldest (back) to newest (front).
        // Since we serialise `items` in display order (newest first), we
        // re-insert in reverse so newest ends up at index 0.
        for n in self.items.into_iter().rev() {
            // `push` takes individual fields and assigns a new id; instead
            // we reconstruct directly so the saved id and TTL survive.
            let live = n.into_live(map);
            q.items.insert(0, live);
        }
        q
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedAlertCooldowns {
    pub cooldowns: Vec<(String, u64, i64)>,
}
impl SavedAlertCooldowns {
    pub fn from_live(_v: &AlertCooldowns) -> Self {
        // AlertCooldowns has private fields; we cannot reach in. Skip persist.
        Self::default()
    }
    pub fn into_live(self) -> AlertCooldowns {
        AlertCooldowns::default()
    }
}

// ---------------------------------------------------------------------------
// SavedComponentBag (Phase A + Phase B)
// ---------------------------------------------------------------------------

/// Holds every persistable component for a single entity as `Option<_>` fields.
/// Only the populated options are re-inserted on load.
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
    pub anomalies: Option<SavedAnomalies>,
    pub forbidden_region: Option<SavedForbiddenRegion>,
    // Colony
    pub colony: Option<SavedColony>,
    pub resource_stockpile: Option<SavedResourceStockpile>,
    pub resource_capacity: Option<SavedResourceCapacity>,
    pub buildings: Option<SavedBuildings>,
    pub building_queue: Option<SavedBuildingQueue>,
    pub build_queue: Option<SavedBuildQueue>,
    pub system_buildings: Option<SavedSystemBuildings>,
    pub system_building_queue: Option<SavedSystemBuildingQueue>,
    pub production: Option<SavedProduction>,
    pub production_focus: Option<SavedProductionFocus>,
    pub colony_jobs: Option<SavedColonyJobs>,
    pub colony_job_rates: Option<SavedColonyJobRates>,
    pub colony_population: Option<SavedColonyPopulation>,
    pub maintenance_cost: Option<SavedMaintenanceCost>,
    pub food_consumption: Option<SavedFoodConsumption>,
    pub deliverable_stockpile: Option<SavedDeliverableStockpile>,
    pub colonization_queue: Option<SavedColonizationQueue>,
    pub pending_colonization_order: Option<SavedPendingColonizationOrder>,
    // Empire-attached components
    pub authority_params: Option<SavedAuthorityParams>,
    pub construction_params: Option<SavedConstructionParams>,
    pub comms_params: Option<SavedCommsParams>,
    pub empire_modifiers: Option<SavedEmpireModifiers>,
    pub global_params: Option<SavedGlobalParams>,
    pub game_flags: Option<SavedGameFlags>,
    pub scoped_flags: Option<SavedScopedFlags>,
    pub tech_tree: Option<SavedTechTree>,
    pub tech_knowledge: Option<SavedTechKnowledge>,
    pub research_queue: Option<SavedResearchQueue>,
    pub research_pool: Option<SavedResearchPool>,
    pub recently_researched: Option<SavedRecentlyResearched>,
    pub knowledge_store: Option<SavedKnowledgeStore>,
    pub command_log: Option<SavedCommandLog>,
    pub pending_colony_tech_modifiers: Option<SavedPendingColonyTechModifiers>,
    // Ship
    pub ship: Option<SavedShip>,
    pub ship_state: Option<SavedShipState>,
    pub ship_hitpoints: Option<SavedShipHitpoints>,
    pub cargo: Option<SavedCargo>,
    pub command_queue: Option<SavedCommandQueue>,
    pub ship_modifiers: Option<SavedShipModifiers>,
    pub courier_route: Option<SavedCourierRoute>,
    pub survey_data: Option<SavedSurveyData>,
    pub scout_report: Option<SavedScoutReport>,
    pub fleet: Option<SavedFleet>,
    pub fleet_membership: Option<SavedFleetMembership>,
    pub detected_hostiles: Option<SavedDetectedHostiles>,
    pub rules_of_engagement: Option<SavedRulesOfEngagement>,
    // Pending command entities (free-standing entities, not attached to a "body")
    pub pending_ship_command: Option<SavedPendingShipCommand>,
    pub pending_diplomatic_action: Option<SavedPendingDiplomaticAction>,
    pub pending_command: Option<SavedPendingCommand>,
    pub pending_research: Option<SavedPendingResearch>,
    pub pending_knowledge_propagation: Option<SavedPendingKnowledgePropagation>,
    // Deep space
    pub deep_space_structure: Option<SavedDeepSpaceStructure>,
    pub ftl_comm_relay: Option<SavedFTLCommRelay>,
    pub structure_hitpoints: Option<SavedStructureHitpoints>,
    pub construction_platform: Option<SavedConstructionPlatform>,
    pub scrapyard: Option<SavedScrapyard>,
    pub lifetime_cost: Option<SavedLifetimeCost>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colony::BuildKind;
    use crate::communication::{ColonyCommand, ColonyCommandKind, RemoteCommand};

    #[test]
    fn colony_remote_command_savebag_roundtrip() {
        let mut map = EntityMap::new();
        let live_planet = Entity::from_raw_u32(42).unwrap();
        let live_host = Entity::from_raw_u32(77).unwrap();
        map.insert(live_planet.to_bits(), live_planet);
        map.insert(live_host.to_bits(), live_host);

        let originals = vec![
            RemoteCommand::Colony(ColonyCommand {
                target_planet: Some(live_planet),
                kind: ColonyCommandKind::QueueBuilding {
                    building_id: "mine".to_string(),
                    target_slot: 2,
                },
            }),
            RemoteCommand::Colony(ColonyCommand {
                target_planet: Some(live_planet),
                kind: ColonyCommandKind::DemolishBuilding { target_slot: 1 },
            }),
            RemoteCommand::Colony(ColonyCommand {
                target_planet: None,
                kind: ColonyCommandKind::UpgradeBuilding {
                    slot_index: 3,
                    target_id: "advanced_shipyard".to_string(),
                },
            }),
            RemoteCommand::Colony(ColonyCommand {
                target_planet: None,
                kind: ColonyCommandKind::QueueShipBuild {
                    host_colony: live_host,
                    design_id: "explorer_mk1".to_string(),
                    build_kind: BuildKind::Deliverable { cargo_size: 4 },
                },
            }),
            RemoteCommand::Colony(ColonyCommand {
                target_planet: None,
                kind: ColonyCommandKind::QueueDeliverableBuild {
                    host_colony: live_host,
                    def_id: "sensor_buoy".to_string(),
                    display_name: "Sensor Buoy".to_string(),
                    cargo_size: 2,
                    minerals_cost: crate::amount::Amt::units(100),
                    energy_cost: crate::amount::Amt::units(50),
                    build_time: 30,
                },
            }),
        ];

        for original in &originals {
            let saved = SavedRemoteCommand::from(original);
            let bytes = serde_json::to_vec(&saved).expect("serialize");
            let restored: SavedRemoteCommand = serde_json::from_slice(&bytes).expect("deserialize");
            let live = restored.into_live(&map);

            // Structural equality via Debug — RemoteCommand doesn't impl Eq.
            assert_eq!(
                format!("{:?}", original),
                format!("{:?}", live),
                "round-trip mismatch for {:?}",
                original
            );
        }
    }

    #[test]
    fn legacy_remote_command_variants_still_roundtrip() {
        let map = EntityMap::new();
        let bs = RemoteCommand::BuildShip {
            design_id: "scout".to_string(),
        };
        let bs_back = SavedRemoteCommand::from(&bs).into_live(&map);
        assert_eq!(format!("{:?}", bs), format!("{:?}", bs_back));

        let focus = RemoteCommand::SetProductionFocus {
            minerals: 0.5,
            energy: 0.3,
            research: 0.2,
        };
        let focus_back = SavedRemoteCommand::from(&focus).into_live(&map);
        assert_eq!(format!("{:?}", focus), format!("{:?}", focus_back));
    }
}
