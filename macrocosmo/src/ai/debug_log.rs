//! AI debug logging — JSONL decision + world-state log streams (#397).
//!
//! The entire module is gated behind `#[cfg(feature = "ai-log")]`.
//! When the feature is off, nothing in this file is compiled and the
//! game binary carries zero overhead.
//!
//! Two log files are written under `logs/`:
//!
//! - **`ai_decision.jsonl`** — one line per NPC empire per tick, recording
//!   the policy's output and a snapshot of key bus metrics at decision time.
//! - **`world_state.jsonl`** — one line per tick with a high-level summary
//!   of every empire (ship count, colony count, stockpile totals).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};

use bevy::prelude::*;
use serde_json::json;

use crate::ai::emit::AiBusReader;
use crate::ai::schema::ids::metric;
use crate::colony::Colony;
use crate::faction::FactionOwner;
use crate::galaxy::{Hostile, StarSystem};
use crate::player::{Empire, Faction, PlayerEmpire};
use crate::ship::Ship;
use crate::time_system::GameClock;

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

/// Holds open file handles for the two JSONL log streams.
#[derive(Resource)]
pub struct AiLogConfig {
    decision_writer: BufWriter<File>,
    world_state_writer: BufWriter<File>,
}

// ---------------------------------------------------------------------------
// Startup system
// ---------------------------------------------------------------------------

/// Create the `logs/` directory and open the two JSONL files.
///
/// Registered at `Startup` when `ai-log` is enabled.
pub fn setup_ai_log(mut commands: Commands) {
    let dir = "logs";
    if let Err(e) = fs::create_dir_all(dir) {
        error!("ai-log: failed to create {dir}: {e}");
        return;
    }

    let decision_file = match File::create(format!("{dir}/ai_decision.jsonl")) {
        Ok(f) => f,
        Err(e) => {
            error!("ai-log: failed to open ai_decision.jsonl: {e}");
            return;
        }
    };
    let world_state_file = match File::create(format!("{dir}/world_state.jsonl")) {
        Ok(f) => f,
        Err(e) => {
            error!("ai-log: failed to open world_state.jsonl: {e}");
            return;
        }
    };

    info!("ai-log: writing to {dir}/ai_decision.jsonl and {dir}/world_state.jsonl");
    commands.insert_resource(AiLogConfig {
        decision_writer: BufWriter::new(decision_file),
        world_state_writer: BufWriter::new(world_state_file),
    });
}

// ---------------------------------------------------------------------------
// Decision log
// ---------------------------------------------------------------------------

/// Write one JSONL line per NPC empire recording the policy decision and a
/// snapshot of key bus metrics.
///
/// Called from [`npc_decision_tick`](super::npc_decision::npc_decision_tick)
/// behind `#[cfg(feature = "ai-log")]`.
pub fn write_decision_log(
    log: &mut AiLogConfig,
    tick: i64,
    faction_id: &str,
    reader: &crate::ai::plugin::AiBusResource,
) {
    // Snapshot key metrics from the bus.
    let snapshot: HashMap<&str, f64> = [
        ("my_total_ships", reader.current(&metric::my_total_ships())),
        ("my_fleet_ready", reader.current(&metric::my_fleet_ready())),
        ("my_strength", reader.current(&metric::my_strength())),
        ("colony_count", reader.current(&metric::colony_count())),
        (
            "systems_with_hostiles",
            reader.current(&metric::systems_with_hostiles()),
        ),
        (
            "net_production_minerals",
            reader.current(&metric::net_production_minerals()),
        ),
        (
            "net_production_energy",
            reader.current(&metric::net_production_energy()),
        ),
        (
            "stockpile_minerals",
            reader.current(&metric::stockpile_minerals()),
        ),
        (
            "stockpile_energy",
            reader.current(&metric::stockpile_energy()),
        ),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|val| (k, val)))
    .collect();

    let line = json!({
        "tick": tick,
        "faction_id": faction_id,
        "decisions": [],  // NoOpPolicy emits nothing; future policies populate this
        "metrics_snapshot": snapshot,
    });

    if let Err(e) = writeln!(log.decision_writer, "{}", line) {
        warn!("ai-log: decision write failed: {e}");
    }
    if let Err(e) = log.decision_writer.flush() {
        warn!("ai-log: decision flush failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// World-state log
// ---------------------------------------------------------------------------

/// Emit one JSONL line per tick summarising the world state visible to the
/// AI pipeline.
///
/// Registered under [`AiTickSet::MetricProduce`] (after emitters) when
/// `ai-log` is enabled.
pub fn emit_world_state_log(
    clock: Res<GameClock>,
    mut log: ResMut<AiLogConfig>,
    factions: Query<(Entity, &Faction), With<Empire>>,
    ships: Query<&FactionOwner, With<Ship>>,
    colonies: Query<&FactionOwner, With<Colony>>,
    systems: Query<Entity, With<StarSystem>>,
    hostiles: Query<Entity, With<Hostile>>,
    player_empires: Query<Entity, With<PlayerEmpire>>,
) {
    let tick = clock.elapsed;

    // Build per-empire summary.
    let mut empire_map: HashMap<Entity, (&str, u32, u32, bool)> = HashMap::new();
    for (entity, faction) in &factions {
        let is_player = player_empires.contains(entity);
        empire_map.insert(entity, (&faction.id, 0, 0, is_player));
    }

    // Count ships per empire.
    for faction_owner in &ships {
        if let Some(entry) = empire_map.get_mut(&faction_owner.0) {
            entry.1 += 1;
        }
    }

    // Count colonies per empire.
    for faction_owner in &colonies {
        if let Some(entry) = empire_map.get_mut(&faction_owner.0) {
            entry.2 += 1;
        }
    }

    let empires_json: Vec<_> = empire_map
        .values()
        .map(|(fid, ships, colonies, is_player)| {
            json!({
                "faction_id": fid,
                "ship_count": ships,
                "colony_count": colonies,
                "is_player": is_player,
            })
        })
        .collect();

    let line = json!({
        "tick": tick,
        "empires": empires_json,
        "total_systems": systems.iter().count(),
        "total_hostiles": hostiles.iter().count(),
    });

    if let Err(e) = writeln!(log.world_state_writer, "{}", line) {
        warn!("ai-log: world_state write failed: {e}");
    }
    if let Err(e) = log.world_state_writer.flush() {
        warn!("ai-log: world_state flush failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_jsonl_format() {
        let snapshot: HashMap<&str, f64> = [("my_total_ships", 5.0), ("colony_count", 3.0)]
            .into_iter()
            .collect();

        let line = json!({
            "tick": 120,
            "faction_id": "npc_1",
            "decisions": [],
            "metrics_snapshot": snapshot,
        });

        let s = serde_json::to_string(&line).unwrap();
        // Verify it's valid JSON and contains expected fields.
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["tick"], 120);
        assert_eq!(parsed["faction_id"], "npc_1");
        assert_eq!(parsed["metrics_snapshot"]["my_total_ships"], 5.0);
        assert_eq!(parsed["metrics_snapshot"]["colony_count"], 3.0);
    }

    #[test]
    fn world_state_jsonl_format() {
        let empires = vec![json!({
            "faction_id": "npc_1",
            "ship_count": 5,
            "colony_count": 3,
            "is_player": false,
        })];

        let line = json!({
            "tick": 120,
            "empires": empires,
            "total_systems": 10,
            "total_hostiles": 2,
        });

        let s = serde_json::to_string(&line).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["tick"], 120);
        assert_eq!(parsed["total_systems"], 10);
        assert_eq!(parsed["total_hostiles"], 2);
        assert_eq!(parsed["empires"][0]["ship_count"], 5);
    }
}
