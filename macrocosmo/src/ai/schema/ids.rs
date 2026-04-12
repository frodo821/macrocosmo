//! Canonical ID helpers for the AI bus schema.
//!
//! Every metric / command / evidence topic that `macrocosmo` declares on
//! `AiBus` has a `fn` here that returns a fresh `Arc<str>`-backed ID. All
//! callers referring to the same topic should go through these helpers
//! rather than typing the string literal inline — this way the topic name
//! can be refactored in a single place.
//!
//! The IDs themselves are plain strings (see
//! `macrocosmo_ai::arc_str_id!`); equality is by contents, so
//! `metric::my_strength() == metric::my_strength()` holds even though
//! each call allocates a new `Arc<str>`. Construction is cheap; avoid
//! pre-computing globally (`once_cell` / `LazyLock`) unless profiling
//! shows the allocation is a hotspot.
//!
//! Grouping mirrors the categories in `docs/ai-atom-reference.md`.

use macrocosmo_ai::{CommandKindId, EvidenceKindId, MetricId};

/// Canonical metric topic IDs. One function per metric declared in
/// `schema::declare_metrics`.
pub mod metric {
    use super::MetricId;

    // 1.1 Military — Self -----------------------------------------------
    pub fn my_total_ships() -> MetricId {
        MetricId::from("my_total_ships")
    }
    pub fn my_strength() -> MetricId {
        MetricId::from("my_strength")
    }
    pub fn my_fleet_ready() -> MetricId {
        MetricId::from("my_fleet_ready")
    }
    pub fn my_armor() -> MetricId {
        MetricId::from("my_armor")
    }
    pub fn my_shields() -> MetricId {
        MetricId::from("my_shields")
    }
    pub fn my_shield_regen_rate() -> MetricId {
        MetricId::from("my_shield_regen_rate")
    }
    pub fn my_vulnerability_score() -> MetricId {
        MetricId::from("my_vulnerability_score")
    }
    pub fn my_has_flagship() -> MetricId {
        MetricId::from("my_has_flagship")
    }

    // 1.2 Economy — Production ------------------------------------------
    pub fn net_production_minerals() -> MetricId {
        MetricId::from("net_production_minerals")
    }
    pub fn net_production_energy() -> MetricId {
        MetricId::from("net_production_energy")
    }
    pub fn net_production_food() -> MetricId {
        MetricId::from("net_production_food")
    }
    pub fn net_production_research() -> MetricId {
        MetricId::from("net_production_research")
    }
    pub fn net_production_authority() -> MetricId {
        MetricId::from("net_production_authority")
    }
    pub fn food_consumption_rate() -> MetricId {
        MetricId::from("food_consumption_rate")
    }
    pub fn food_surplus() -> MetricId {
        MetricId::from("food_surplus")
    }

    // 1.3 Economy — Stockpiles ------------------------------------------
    pub fn stockpile_minerals() -> MetricId {
        MetricId::from("stockpile_minerals")
    }
    pub fn stockpile_energy() -> MetricId {
        MetricId::from("stockpile_energy")
    }
    pub fn stockpile_food() -> MetricId {
        MetricId::from("stockpile_food")
    }
    pub fn stockpile_authority() -> MetricId {
        MetricId::from("stockpile_authority")
    }
    pub fn stockpile_ratio_minerals() -> MetricId {
        MetricId::from("stockpile_ratio_minerals")
    }
    pub fn stockpile_ratio_energy() -> MetricId {
        MetricId::from("stockpile_ratio_energy")
    }
    pub fn stockpile_ratio_food() -> MetricId {
        MetricId::from("stockpile_ratio_food")
    }
    pub fn total_authority_debt() -> MetricId {
        MetricId::from("total_authority_debt")
    }

    // 1.4 Population ----------------------------------------------------
    pub fn population_total() -> MetricId {
        MetricId::from("population_total")
    }
    pub fn population_growth_rate() -> MetricId {
        MetricId::from("population_growth_rate")
    }
    pub fn population_carrying_capacity() -> MetricId {
        MetricId::from("population_carrying_capacity")
    }
    pub fn population_ratio() -> MetricId {
        MetricId::from("population_ratio")
    }

    // 1.5 Territory -----------------------------------------------------
    pub fn colony_count() -> MetricId {
        MetricId::from("colony_count")
    }
    pub fn colonized_system_count() -> MetricId {
        MetricId::from("colonized_system_count")
    }
    pub fn border_system_count() -> MetricId {
        MetricId::from("border_system_count")
    }
    pub fn habitable_systems_known() -> MetricId {
        MetricId::from("habitable_systems_known")
    }
    pub fn colonizable_systems_remaining() -> MetricId {
        MetricId::from("colonizable_systems_remaining")
    }
    pub fn systems_with_hostiles() -> MetricId {
        MetricId::from("systems_with_hostiles")
    }

    // 1.6 Technology ----------------------------------------------------
    pub fn tech_total_researched() -> MetricId {
        MetricId::from("tech_total_researched")
    }
    pub fn tech_completion_percent() -> MetricId {
        MetricId::from("tech_completion_percent")
    }
    pub fn tech_unlocks_available() -> MetricId {
        MetricId::from("tech_unlocks_available")
    }
    pub fn research_output_ratio() -> MetricId {
        MetricId::from("research_output_ratio")
    }

    // 1.7 Infrastructure ------------------------------------------------
    pub fn systems_with_shipyard() -> MetricId {
        MetricId::from("systems_with_shipyard")
    }
    pub fn systems_with_port() -> MetricId {
        MetricId::from("systems_with_port")
    }
    pub fn max_building_slots() -> MetricId {
        MetricId::from("max_building_slots")
    }
    pub fn used_building_slots() -> MetricId {
        MetricId::from("used_building_slots")
    }
    pub fn free_building_slots() -> MetricId {
        MetricId::from("free_building_slots")
    }
    pub fn can_build_ships() -> MetricId {
        MetricId::from("can_build_ships")
    }

    // 1.8 Meta / Time ---------------------------------------------------
    pub fn game_elapsed_time() -> MetricId {
        MetricId::from("game_elapsed_time")
    }
    pub fn number_of_allies() -> MetricId {
        MetricId::from("number_of_allies")
    }
    pub fn number_of_enemies() -> MetricId {
        MetricId::from("number_of_enemies")
    }
}

/// Canonical command kind IDs. One function per command kind declared in
/// `schema::declare_commands`.
pub mod command {
    use super::CommandKindId;

    // Military
    pub fn attack_target() -> CommandKindId {
        CommandKindId::from("attack_target")
    }
    pub fn reposition() -> CommandKindId {
        CommandKindId::from("reposition")
    }
    pub fn retreat() -> CommandKindId {
        CommandKindId::from("retreat")
    }
    pub fn blockade() -> CommandKindId {
        CommandKindId::from("blockade")
    }
    pub fn fortify_system() -> CommandKindId {
        CommandKindId::from("fortify_system")
    }

    // Expansion / infrastructure
    pub fn colonize_system() -> CommandKindId {
        CommandKindId::from("colonize_system")
    }
    pub fn build_ship() -> CommandKindId {
        CommandKindId::from("build_ship")
    }
    pub fn build_structure() -> CommandKindId {
        CommandKindId::from("build_structure")
    }
    pub fn survey_system() -> CommandKindId {
        CommandKindId::from("survey_system")
    }

    // Research
    pub fn research_focus() -> CommandKindId {
        CommandKindId::from("research_focus")
    }

    // Diplomacy
    pub fn declare_war() -> CommandKindId {
        CommandKindId::from("declare_war")
    }
    pub fn seek_peace() -> CommandKindId {
        CommandKindId::from("seek_peace")
    }
    pub fn propose_alliance() -> CommandKindId {
        CommandKindId::from("propose_alliance")
    }
    pub fn establish_relation() -> CommandKindId {
        CommandKindId::from("establish_relation")
    }
}

/// Canonical evidence kind IDs. One function per evidence kind declared
/// in `schema::declare_evidence`.
pub mod evidence {
    use super::EvidenceKindId;

    // Hostile
    pub fn direct_attack() -> EvidenceKindId {
        EvidenceKindId::from("direct_attack")
    }
    pub fn system_seized() -> EvidenceKindId {
        EvidenceKindId::from("system_seized")
    }
    pub fn border_incursion() -> EvidenceKindId {
        EvidenceKindId::from("border_incursion")
    }
    pub fn hostile_buildup_near() -> EvidenceKindId {
        EvidenceKindId::from("hostile_buildup_near")
    }
    pub fn blockade_imposed() -> EvidenceKindId {
        EvidenceKindId::from("blockade_imposed")
    }
    pub fn hostile_engagement() -> EvidenceKindId {
        EvidenceKindId::from("hostile_engagement")
    }
    pub fn fleet_loss() -> EvidenceKindId {
        EvidenceKindId::from("fleet_loss")
    }

    // Friendly
    pub fn gift_given() -> EvidenceKindId {
        EvidenceKindId::from("gift_given")
    }
    pub fn trade_agreement_established() -> EvidenceKindId {
        EvidenceKindId::from("trade_agreement_established")
    }
    pub fn alliance_with_observer() -> EvidenceKindId {
        EvidenceKindId::from("alliance_with_observer")
    }
    pub fn support_against_enemy() -> EvidenceKindId {
        EvidenceKindId::from("support_against_enemy")
    }
    pub fn military_withdrawal() -> EvidenceKindId {
        EvidenceKindId::from("military_withdrawal")
    }

    // Ambiguous
    pub fn major_military_buildup() -> EvidenceKindId {
        EvidenceKindId::from("major_military_buildup")
    }
    pub fn border_colonization() -> EvidenceKindId {
        EvidenceKindId::from("border_colonization")
    }
}
