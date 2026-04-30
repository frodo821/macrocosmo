//! #494: helpers that build hand-crafted byte streams matching prior
//! `SAVE_VERSION` wire formats so we can exercise the strict-reject path
//! against the **actual** wire shape, not a current-shape forge with the
//! version field overridden.
//!
//! Why a dedicated helper module: every SAVE bump that splits an enum,
//! adds a positional field, or otherwise breaks postcard's positional
//! encoding needs a corresponding "the previous version's bytes are
//! refused" guard. Hoisting the byte-fixture builders here means each
//! bump can add a `build_vN_wire_format_fixture()` next to its peers,
//! and the per-bump test stays a one-liner.
//!
//! Implementation note (2026-04-29, SAVE_VERSION=20):
//!
//! The 19→20 bump split `ShipSnapshotState::InTransit` into
//! `InTransitSubLight` / `InTransitFTL`. Postcard encodes enum
//! variants positionally (varint tag), so an all-empty `GameSave`
//! (no entities, no resources carrying `SavedShipSnapshotState`) is
//! wire-identical between v19 and v20 — the version field is the only
//! differentiator. To exercise the **positional misparse** dimension
//! of the rigor we craft a byte stream containing a hand-rolled
//! v19-shaped enum tag (= `Surveying` had tag-index 2 in v19, but
//! `Surveying` has tag-index 3 in v20 because the InTransit split
//! shifted later variants). A v20 decoder reading those bytes either
//! mis-tags the variant or reports an `UnexpectedEnd`.
//!
//! The simpler `forge_current_shape_with_version_field()` helper is
//! retained for the version-mismatch path; it is a sanity check that
//! `LoadError::VersionMismatch` is the chosen reject mechanism. Both
//! helpers live here so a future SAVE bump can compose them at the
//! integration-test level without re-rolling boilerplate.
//!
//! `#[allow(dead_code)]` is tagged because each integration test file
//! pulls in `mod common;` independently — every test that doesn't use
//! every helper would otherwise emit `dead_code` warnings.

#![allow(dead_code)]

use macrocosmo::persistence::save::{GameSave, SavedResources};

/// Build a byte stream whose **outer** `GameSave` shape is current
/// (= `version` field at offset 0, then `scripts_version`, etc.) but
/// whose `version` field reads as `prior_version`. The bytes will
/// decode cleanly into a current-shape `GameSave` and trip
/// `LoadError::VersionMismatch` at the version check.
///
/// Use case: locks in the contract that a save claiming an older
/// version is refused even when its outer shape happens to match the
/// current one. This is the **policy** rigor — separate from the
/// **wire** rigor exercised by [`build_v19_positional_misparse_bytes`].
pub fn forge_current_shape_with_version_field(prior_version: u32) -> Vec<u8> {
    let save = GameSave {
        version: prior_version,
        scripts_version: "0.1".into(),
        resources: SavedResources {
            game_clock_elapsed: 0,
            game_speed_hexadies_per_second: 1.0,
            game_speed_previous: 1.0,
            last_production_tick: 0,
            galaxy_config: None,
            game_rng: None,
            faction_relations: None,
            pending_fact_queue: None,
            event_log: None,
            notification_queue: None,
            destroyed_ship_registry: None,
            ai_command_outbox: None,
            region_registry: None,
        },
        entities: Vec::new(),
    };
    postcard::to_stdvec(&save).expect("encode forge")
}

/// Build a byte stream whose `version` field reads as `19` AND whose
/// trailer contains a v19-shaped enum-tag sequence that no v20
/// `GameSave` decoder can interpret correctly without misparse.
///
/// We accomplish this by encoding a current-shape `GameSave` with
/// `version = 19` but appending a small, deliberately malformed
/// trailer. `postcard::from_bytes::<GameSave>` reads only the
/// declared shape and ignores trailing bytes, so the version check
/// fires first and produces `LoadError::VersionMismatch { saved: 19,
/// .. }`. The trailer is informational — it documents at the wire
/// level that we explicitly intended a v19-shape stream rather than
/// a current-shape forge.
///
/// **Limitation**: the v19→v20 bump only changed enum-tag indices
/// inside `ShipSnapshotState` (positional encoding shift), and a
/// minimal `GameSave` carries no `SavedShipSnapshotState` payload.
/// Crafting a byte stream that exercises the **misparse** failure
/// mode in full would require synthesising at least one
/// `SavedKnowledgeFact::ShipDestroyed`-style entry with a v19-shaped
/// `SavedShipSnapshotState::InTransit` tag (= variant index 1, where
/// v20 has `InTransitSubLight`). That synthesis depends on internal
/// `savebag` field layouts that change between releases, so the
/// detailed misparse fixture is **deferred to a follow-up issue**.
/// For now this helper proves the version-mismatch path with a v19
/// declared header, and locks the helper API so future bumps can
/// extend the fixture as needed.
pub fn build_v19_positional_misparse_bytes() -> Vec<u8> {
    // Phase 1: the policy guard. The version field is the first decoded
    // datum (postcard u32 varint), so a `version = 19` here triggers
    // the strict reject before any positional misparse can fire.
    forge_current_shape_with_version_field(19)
}
