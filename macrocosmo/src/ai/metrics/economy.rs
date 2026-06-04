//! Pure economic metric snapshots plus AI-bus emission mapping.

use macrocosmo_ai::FactionId;

use crate::ai::emit::AiBusWriter;
use crate::ai::schema::ids::metric;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EmpireProductionSnapshot {
    pub minerals_rate: f64,
    pub energy_rate: f64,
    pub food_rate: f64,
    pub research_rate: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EmpirePopulationSnapshot {
    pub total: f64,
    pub growth_rate: f64,
    pub carrying_capacity: f64,
}

impl EmpirePopulationSnapshot {
    pub fn ratio(self) -> f64 {
        if self.carrying_capacity > 0.0 {
            self.total / self.carrying_capacity
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EmpireStockpileSnapshot {
    pub minerals: f64,
    pub energy: f64,
    pub food: f64,
    pub authority: f64,
    pub mineral_capacity: f64,
    pub energy_capacity: f64,
    pub food_capacity: f64,
    pub authority_debt: f64,
}

impl EmpireStockpileSnapshot {
    pub fn mineral_ratio(self) -> f64 {
        capped_ratio(self.minerals, self.mineral_capacity)
    }

    pub fn energy_ratio(self) -> f64 {
        capped_ratio(self.energy, self.energy_capacity)
    }

    pub fn food_ratio(self) -> f64 {
        capped_ratio(self.food, self.food_capacity)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EmpireInfrastructureSnapshot {
    pub systems_with_shipyard: f64,
    pub total_shipyard_slots: f64,
    pub systems_with_port: f64,
    pub systems_with_core: f64,
    pub max_building_slots: f64,
    pub used_building_slots: f64,
}

impl EmpireInfrastructureSnapshot {
    pub fn free_building_slots(self) -> f64 {
        self.max_building_slots - self.used_building_slots
    }

    pub fn can_build_ships(self) -> f64 {
        self.systems_with_shipyard
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EmpireEconomicSnapshot {
    pub production: EmpireProductionSnapshot,
    pub population: EmpirePopulationSnapshot,
    pub food_consumption_rate: f64,
    pub colony_count: f64,
    pub colonized_system_count: f64,
    pub stockpile: EmpireStockpileSnapshot,
    pub infrastructure: EmpireInfrastructureSnapshot,
}

impl EmpireEconomicSnapshot {
    pub fn food_surplus(self) -> f64 {
        self.production.food_rate - self.food_consumption_rate
    }
}

pub(crate) fn emit_economic_snapshot(
    writer: &mut AiBusWriter,
    faction: FactionId,
    snapshot: EmpireEconomicSnapshot,
) {
    let production = snapshot.production;
    writer.emit(
        &metric::for_faction("net_production_minerals", faction),
        production.minerals_rate,
    );
    writer.emit(
        &metric::for_faction("net_production_energy", faction),
        production.energy_rate,
    );
    writer.emit(
        &metric::for_faction("net_production_food", faction),
        production.food_rate,
    );
    writer.emit(
        &metric::for_faction("net_production_research", faction),
        production.research_rate,
    );

    let population = snapshot.population;
    writer.emit(
        &metric::for_faction("population_total", faction),
        population.total,
    );
    writer.emit(
        &metric::for_faction("population_growth_rate", faction),
        population.growth_rate,
    );
    writer.emit(
        &metric::for_faction("population_carrying_capacity", faction),
        population.carrying_capacity,
    );
    writer.emit(
        &metric::for_faction("population_ratio", faction),
        population.ratio(),
    );

    writer.emit(
        &metric::for_faction("food_consumption_rate", faction),
        snapshot.food_consumption_rate,
    );
    writer.emit(
        &metric::for_faction("food_surplus", faction),
        snapshot.food_surplus(),
    );

    writer.emit(
        &metric::for_faction("colony_count", faction),
        snapshot.colony_count,
    );
    writer.emit(
        &metric::for_faction("colonized_system_count", faction),
        snapshot.colonized_system_count,
    );

    let stockpile = snapshot.stockpile;
    writer.emit(
        &metric::for_faction("stockpile_minerals", faction),
        stockpile.minerals,
    );
    writer.emit(
        &metric::for_faction("stockpile_energy", faction),
        stockpile.energy,
    );
    writer.emit(
        &metric::for_faction("stockpile_food", faction),
        stockpile.food,
    );
    writer.emit(
        &metric::for_faction("stockpile_authority", faction),
        stockpile.authority,
    );
    writer.emit(
        &metric::for_faction("stockpile_ratio_minerals", faction),
        stockpile.mineral_ratio(),
    );
    writer.emit(
        &metric::for_faction("stockpile_ratio_energy", faction),
        stockpile.energy_ratio(),
    );
    writer.emit(
        &metric::for_faction("stockpile_ratio_food", faction),
        stockpile.food_ratio(),
    );
    writer.emit(
        &metric::for_faction("total_authority_debt", faction),
        stockpile.authority_debt,
    );

    let infrastructure = snapshot.infrastructure;
    writer.emit(
        &metric::for_faction("systems_with_shipyard", faction),
        infrastructure.systems_with_shipyard,
    );
    writer.emit(
        &metric::for_faction("total_shipyard_slots", faction),
        infrastructure.total_shipyard_slots,
    );
    writer.emit(
        &metric::for_faction("systems_with_port", faction),
        infrastructure.systems_with_port,
    );
    writer.emit(
        &metric::for_faction("systems_with_core", faction),
        infrastructure.systems_with_core,
    );
    writer.emit(
        &metric::for_faction("max_building_slots", faction),
        infrastructure.max_building_slots,
    );
    writer.emit(
        &metric::for_faction("used_building_slots", faction),
        infrastructure.used_building_slots,
    );
    writer.emit(
        &metric::for_faction("free_building_slots", faction),
        infrastructure.free_building_slots(),
    );
    writer.emit(
        &metric::for_faction("can_build_ships", faction),
        infrastructure.can_build_ships(),
    );
}

fn capped_ratio(value: f64, capacity: f64) -> f64 {
    if capacity > 0.0 {
        (value / capacity).min(1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn population_ratio_is_zero_without_capacity() {
        let snapshot = EmpirePopulationSnapshot {
            total: 100.0,
            carrying_capacity: 0.0,
            ..Default::default()
        };

        assert_eq!(snapshot.ratio(), 0.0);
    }

    #[test]
    fn stockpile_ratios_are_capped_and_zero_without_capacity() {
        let snapshot = EmpireStockpileSnapshot {
            minerals: 120.0,
            mineral_capacity: 100.0,
            energy: 50.0,
            energy_capacity: 200.0,
            food: 10.0,
            food_capacity: 0.0,
            ..Default::default()
        };

        assert_eq!(snapshot.mineral_ratio(), 1.0);
        assert_eq!(snapshot.energy_ratio(), 0.25);
        assert_eq!(snapshot.food_ratio(), 0.0);
    }

    #[test]
    fn economic_snapshot_derives_food_and_build_capacity() {
        let snapshot = EmpireEconomicSnapshot {
            production: EmpireProductionSnapshot {
                food_rate: 8.0,
                ..Default::default()
            },
            food_consumption_rate: 3.5,
            infrastructure: EmpireInfrastructureSnapshot {
                systems_with_shipyard: 2.0,
                max_building_slots: 6.0,
                used_building_slots: 4.0,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(snapshot.food_surplus(), 4.5);
        assert_eq!(snapshot.infrastructure.free_building_slots(), 2.0);
        assert_eq!(snapshot.infrastructure.can_build_ships(), 2.0);
    }
}
