use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::GalaxyView;
use crate::colony::{Buildings, Colony};
use crate::components::Position;
use crate::deep_space::{ConstructionPlatform, DeepSpaceStructure, Scrapyard, StructureHitpoints};
use crate::galaxy::{AtSystem, GalaxyConfig, Hostile, ObscuredByGas, Planet, StarSystem};
use crate::knowledge::{KnowledgeStore, SystemVisibilityMap, SystemVisibilityTier};
use crate::player::{Empire, Player, PlayerEmpire, StationedAt};
use crate::ship::{Ship, ShipState};
use crate::technology::GlobalParams;
use crate::time_system::GameClock;

use super::{SelectedShip, SelectedSystem};

#[derive(Component)]
pub(super) struct StarVisual {
    pub system_entity: Entity,
}

/// Marks a sprite as a glow halo behind a star.
#[derive(Component)]
pub(super) struct StarGlow;

/// #434: Marks a Text2d entity as a star name label so `update_star_colors`
/// can dynamically show/hide it based on KnowledgeStore.
#[derive(Component)]
pub(super) struct StarLabel;

/// Stores the base pixel size of a star sprite so zoom-responsive scaling can reference it.
#[derive(Component)]
pub(super) struct BaseStarSize(pub f32);

/// #439 Phase 4 `OnExit(GameState::InGame)` cleanup — despawn every sprite /
/// label / glow spawned by [`spawn_star_visuals`]. These entities live on
/// the camera-rendered sprite layer and do **not** carry a
/// `SaveableMarker` (they are reconstructable view state, not game state),
/// so the generic `cleanup_ingame_entities` system in `game_state` does
/// not catch them. `StarVisual` is the common marker attached to all
/// three kinds (glow, main dot, label), so a single query covers them.
pub fn cleanup_star_visuals(mut commands: Commands, visuals: Query<Entity, With<StarVisual>>) {
    for e in &visuals {
        commands.entity(e).despawn();
    }
}

pub fn spawn_star_visuals(
    mut commands: Commands,
    stars: Query<(Entity, &StarSystem, &Position, Option<&ObscuredByGas>)>,
    colonies: Query<&Colony>,
    planets: Query<&Planet>,
    view: Res<GalaxyView>,
    empire_q: Query<&KnowledgeStore, With<PlayerEmpire>>,
    player_q: Query<&StationedAt, With<Player>>,
    // Observer mode: no PlayerEmpire exists, so the knowledge query above
    // is empty. Rather than early-returning (which left the galaxy map
    // blank), we grant full ground-truth visibility — observer mode is
    // explicitly god-view for balance/debug.
    observer_mode: Res<crate::observer::ObserverMode>,
) {
    // Build a set of colonized system entities
    let colonized_systems: std::collections::HashSet<Entity> =
        colonies.iter().filter_map(|c| c.system(&planets)).collect();

    let knowledge = empire_q.iter().next();
    let player_system = player_q.iter().next().map(|s| s.system);
    // #417 parity: observer mode collapses the knowledge gate so every
    // star is rendered as if the player had direct visibility.
    let god_view = observer_mode.enabled;

    for (entity, star, pos, obscured) in &stars {
        let x = pos.x as f32 * view.scale;
        let y = pos.y as f32 * view.scale;
        let is_obscured = obscured.is_some();

        // #434: Gate is_capital/surveyed/colonized on KnowledgeStore for
        // remote systems — the live StarSystem flags are global and would
        // leak NPC faction data to the player.
        let is_local = player_system == Some(entity);
        let use_ground_truth = is_local || god_view;
        let effective_capital = if use_ground_truth {
            star.is_capital
        } else {
            knowledge
                .and_then(|k| k.get(entity))
                .map(|k| k.data.is_capital)
                .unwrap_or(false)
        };
        let effective_surveyed = if use_ground_truth {
            star.surveyed
        } else {
            knowledge
                .and_then(|k| k.get(entity))
                .map(|k| k.data.surveyed)
                .unwrap_or(false)
        };
        let is_colonized = if use_ground_truth {
            colonized_systems.contains(&entity)
        } else {
            knowledge
                .and_then(|k| k.get(entity))
                .map(|k| k.data.colonized)
                .unwrap_or(false)
        };

        let effective_star = StarSystem {
            name: star.name.clone(),
            star_type: star.star_type.clone(),
            surveyed: effective_surveyed,
            is_capital: effective_capital,
        };
        let color = star_color(&effective_star, is_colonized, is_obscured);

        // Determine base size based on knowledge-gated status
        let size = if effective_capital {
            16.0
        } else if is_colonized {
            14.0
        } else if effective_surveyed {
            12.0
        } else {
            10.0
        };

        // Spawn glow halo behind the star (skip for obscured stars)
        if !is_obscured {
            let [r, g, b, _] = color.to_srgba().to_f32_array();
            let glow_alpha = if effective_capital || is_colonized {
                0.2
            } else {
                0.15
            };
            let glow_size = size * 3.0;
            commands.spawn((
                StarVisual {
                    system_entity: entity,
                },
                StarGlow,
                BaseStarSize(glow_size),
                Sprite {
                    color: Color::srgba(r, g, b, glow_alpha),
                    custom_size: Some(Vec2::splat(glow_size)),
                    ..default()
                },
                Transform::from_xyz(x, y, -0.1),
            ));
        }

        // Spawn main star dot
        commands.spawn((
            StarVisual {
                system_entity: entity,
            },
            BaseStarSize(size),
            Sprite {
                color,
                custom_size: Some(Vec2::splat(size)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.0),
        ));

        // #434: Spawn labels for ALL stars (with alpha=0 for unknown ones)
        // so that `update_star_colors` can dynamically show/hide them as
        // knowledge is discovered.
        let label_alpha = if effective_capital {
            1.0
        } else if is_colonized {
            0.9
        } else if effective_surveyed {
            0.7
        } else {
            0.0
        };
        commands.spawn((
            StarVisual {
                system_entity: entity,
            },
            StarLabel,
            Text2d::new(&star.name),
            TextFont {
                font_size: 14.0,
                ..default()
            },
            TextColor(Color::srgba(1.0, 1.0, 1.0, label_alpha)),
            Transform::from_xyz(x, y + 14.0, 1.0),
        ));
    }
}

pub(super) fn star_color(star: &StarSystem, colonized: bool, obscured: bool) -> Color {
    if obscured {
        Color::srgba(0.2, 0.2, 0.25, 0.15) // Barely visible
    } else if star.is_capital {
        Color::srgb(1.0, 0.84, 0.0) // Gold
    } else if colonized {
        Color::srgb(0.3, 1.0, 0.3) // Bright green, more saturated
    } else if star.surveyed {
        Color::srgb(0.5, 0.7, 1.0) // Bright blue
    } else {
        Color::srgba(0.5, 0.5, 0.55, 0.4) // Dim, small, unsurveyed
    }
}

// #17: Enhanced update_star_colors with KnowledgeStore-based alpha fading
// #40: Also handles zoom-responsive sizing and glow color updates
// #176: Uses KnowledgeStore for remote system colonized status
pub fn update_star_colors(
    stars: Query<(Entity, &StarSystem, Option<&ObscuredByGas>)>,
    mut visuals: Query<
        (
            &StarVisual,
            &mut Sprite,
            Option<&StarGlow>,
            Option<&mut BaseStarSize>,
        ),
        Without<StarLabel>,
    >,
    mut labels: Query<(&StarVisual, &mut TextColor), With<StarLabel>>,
    empire_q: Query<(&KnowledgeStore, Option<&SystemVisibilityMap>), With<PlayerEmpire>>,
    colonies: Query<&Colony>,
    planets: Query<&Planet>,
    clock: Res<GameClock>,
    camera_q: Query<&Projection, With<Camera2d>>,
    player_q: Query<&StationedAt, With<Player>>,
    // See `spawn_star_visuals` for the observer-mode rationale. In god
    // view we skip the knowledge gate entirely and drive visuals off the
    // live `StarSystem` / colony state.
    observer_mode: Res<crate::observer::ObserverMode>,
) {
    crate::prof_span!("update_star_colors");
    let god_view = observer_mode.enabled;
    let empire_row = empire_q.single().ok();
    if empire_row.is_none() && !god_view {
        // Normal-mode: no PlayerEmpire → nothing to update. Observer mode
        // still needs to fall through to refresh colors from ground truth.
        return;
    }
    let knowledge = empire_row.map(|(k, _)| k);
    let vis_map_opt = empire_row.and_then(|(_, v)| v);
    let player_system = player_q.iter().next().map(|s| s.system);

    // Build colonized systems set for local system only (real-time)
    let local_colonized: std::collections::HashSet<Entity> =
        colonies.iter().filter_map(|c| c.system(&planets)).collect();

    // Get the current camera scale for zoom-responsive sizing
    let camera_scale = camera_q
        .iter()
        .find_map(|proj| {
            if let Projection::Orthographic(ref ortho) = *proj {
                Some(ortho.scale)
            } else {
                None
            }
        })
        .unwrap_or(1.0);

    // Zoom-responsive scale factor: when zoomed out, make stars proportionally larger
    let zoom_factor = (1.0 + (camera_scale - 1.0) * 0.5).max(1.0);

    for (vis, mut sprite, glow, base_size) in &mut visuals {
        if let Ok((_, star, obscured)) = stars.get(vis.system_entity) {
            let use_ground_truth = god_view || player_system == Some(vis.system_entity);
            // #176: Local system uses real-time colonized status, remote uses KnowledgeStore
            let is_colonized = if use_ground_truth {
                local_colonized.contains(&vis.system_entity)
            } else {
                knowledge
                    .and_then(|k| k.get(vis.system_entity))
                    .map(|k| k.data.colonized)
                    .unwrap_or(false)
            };
            // #434: For remote systems, use knowledge-based survey status only.
            // Do NOT fall back to star.surveyed — it is global.
            let effective_surveyed = if use_ground_truth {
                star.surveyed
            } else {
                knowledge
                    .and_then(|k| k.get(vis.system_entity))
                    .map(|k| k.data.surveyed)
                    .unwrap_or(false)
            };
            // #430: Gate is_capital on KnowledgeStore for remote systems
            let effective_capital = if use_ground_truth {
                star.is_capital
            } else {
                knowledge
                    .and_then(|k| k.get(vis.system_entity))
                    .map(|k| k.data.is_capital)
                    .unwrap_or(false)
            };
            // Create a temporary view of the star with knowledge-based survey status
            let effective_star = StarSystem {
                name: star.name.clone(),
                star_type: star.star_type.clone(),
                surveyed: effective_surveyed,
                is_capital: effective_capital,
            };
            let base_color = star_color(&effective_star, is_colonized, obscured.is_some());
            // #392: Tier-based alpha multiplier. Catalogued systems are extra
            // dim; surveyed systems use knowledge age fading.
            let tier = if god_view {
                SystemVisibilityTier::Local
            } else {
                vis_map_opt
                    .map(|vm| vm.get(vis.system_entity))
                    .unwrap_or(if effective_surveyed {
                        SystemVisibilityTier::Surveyed
                    } else {
                        SystemVisibilityTier::Catalogued
                    })
            };
            let alpha_multiplier = if tier == SystemVisibilityTier::Catalogued {
                0.3 // Catalogued: faint star point only
            } else if god_view {
                1.0
            } else {
                match knowledge.and_then(|k| k.info_age(vis.system_entity, clock.elapsed)) {
                    None => 1.0,
                    Some(age) if age < 60 => 1.0, // Fresh (< 1 year)
                    Some(age) => (1.0 - (age as f32 - 60.0) / 600.0).clamp(0.3, 1.0),
                }
            };
            let [r, g, b, a] = base_color.to_srgba().to_f32_array();

            if glow.is_some() {
                // Glow sprites: use base color with low alpha, also apply age fading
                let glow_alpha = if effective_capital || is_colonized {
                    0.2
                } else {
                    0.15
                };
                sprite.color = Color::srgba(r, g, b, glow_alpha * alpha_multiplier);
            } else {
                sprite.color = Color::srgba(r, g, b, a * alpha_multiplier);
            }

            // #430: Recalculate base size from knowledge-gated state so that
            // enemy capitals don't leak a larger dot before being discovered.
            if let Some(mut base) = base_size {
                let new_base = if glow.is_some() {
                    // Glow sprites are 3x the star size
                    let star_size = if effective_capital {
                        16.0
                    } else if is_colonized {
                        14.0
                    } else if effective_surveyed {
                        12.0
                    } else {
                        10.0
                    };
                    star_size * 3.0
                } else if effective_capital {
                    16.0
                } else if is_colonized {
                    14.0
                } else if effective_surveyed {
                    12.0
                } else {
                    10.0
                };
                base.0 = new_base;
                let scaled = new_base * zoom_factor;
                sprite.custom_size = Some(Vec2::splat(scaled));
            }
        }
    }

    // #434: Dynamically update star label visibility based on KnowledgeStore.
    for (vis, mut text_color) in &mut labels {
        if let Ok((_, star, _)) = stars.get(vis.system_entity) {
            let use_ground_truth = god_view || player_system == Some(vis.system_entity);
            let effective_capital = if use_ground_truth {
                star.is_capital
            } else {
                knowledge
                    .and_then(|k| k.get(vis.system_entity))
                    .map(|k| k.data.is_capital)
                    .unwrap_or(false)
            };
            let effective_surveyed = if use_ground_truth {
                star.surveyed
            } else {
                knowledge
                    .and_then(|k| k.get(vis.system_entity))
                    .map(|k| k.data.surveyed)
                    .unwrap_or(false)
            };
            let is_colonized = if use_ground_truth {
                local_colonized.contains(&vis.system_entity)
            } else {
                knowledge
                    .and_then(|k| k.get(vis.system_entity))
                    .map(|k| k.data.colonized)
                    .unwrap_or(false)
            };

            let label_alpha = if effective_capital {
                1.0
            } else if is_colonized {
                0.9
            } else if effective_surveyed {
                0.7
            } else {
                0.0
            };
            *text_color = TextColor(Color::srgba(1.0, 1.0, 1.0, label_alpha));
        }
    }
}

/// Bundle that resolves the "viewing" empire for visualization systems.
/// In normal play this is the `PlayerEmpire` entity; in observer mode
/// it is whatever empire the top-bar selector is currently focused on
/// (`ObserverView.viewing`). Collapsing three params into one SystemParam
/// keeps `draw_galaxy_overlay` under Bevy's 16-param ceiling.
#[derive(SystemParam)]
pub struct ViewingEmpireResolver<'w, 's> {
    pub observer_mode: Res<'w, crate::observer::ObserverMode>,
    pub observer_view: Res<'w, crate::observer::ObserverView>,
    pub player_empire: Query<'w, 's, Entity, With<PlayerEmpire>>,
}

impl<'w, 's> ViewingEmpireResolver<'w, 's> {
    pub fn resolve(&self) -> Option<Entity> {
        if self.observer_mode.enabled {
            self.observer_view.viewing
        } else {
            self.player_empire.single().ok()
        }
    }

    pub fn is_god_view(&self) -> bool {
        self.observer_mode.enabled
    }
}

pub fn draw_galaxy_overlay(
    mut gizmos: Gizmos,
    player_q: Query<&StationedAt, With<Player>>,
    stars: Query<(Entity, &StarSystem, &Position)>,
    view: Res<GalaxyView>,
    clock: Res<GameClock>,
    selected: Res<SelectedSystem>,
    selected_ship: Res<SelectedShip>,
    ships: Query<(Entity, &Ship, &ShipState)>,
    // Every empire's state — we pick which one via `viewer`. In normal
    // mode the filter narrows to the player; in observer mode we follow
    // the top-bar selector.
    empire_params_q: Query<
        (
            Entity,
            &GlobalParams,
            &KnowledgeStore,
            Option<&SystemVisibilityMap>,
        ),
        With<Empire>,
    >,
    viewer: ViewingEmpireResolver,
    sys_mods_q: Query<&crate::galaxy::SystemModifiers>,
    colonies: Query<(&Colony, &Buildings)>,
    planets: Query<&Planet>,
    galaxy_config: Option<Res<GalaxyConfig>>,
    hostiles: Query<(&AtSystem, Option<&crate::faction::FactionOwner>), With<Hostile>>,
    faction_relations: Res<crate::faction::FactionRelations>,
) {
    // Galaxy outline: center marker and boundary circle
    if let Some(ref config) = galaxy_config {
        let boundary_radius = config.radius as f32 * view.scale;
        // Galaxy center crosshair
        gizmos.circle_2d(Vec2::ZERO, 3.0, Color::srgba(0.5, 0.5, 0.5, 0.15));
        gizmos.line_2d(
            Vec2::new(-5.0, 0.0),
            Vec2::new(5.0, 0.0),
            Color::srgba(0.5, 0.5, 0.5, 0.1),
        );
        gizmos.line_2d(
            Vec2::new(0.0, -5.0),
            Vec2::new(0.0, 5.0),
            Color::srgba(0.5, 0.5, 0.5, 0.1),
        );
        // Galaxy boundary circle
        gizmos.circle_2d(
            Vec2::ZERO,
            boundary_radius,
            Color::srgba(0.3, 0.3, 0.5, 0.08),
        );
    }

    // Resolve the viewing empire. Normal mode: the PlayerEmpire.
    // Observer mode: whatever empire `ObserverView.viewing` points at (the
    // top-bar selector). If neither exists, only the galaxy boundary is
    // drawn (already emitted above).
    let Some(viewer_entity) = viewer.resolve() else {
        return;
    };
    let Ok((viewer_empire, global_params, knowledge, vis_map_opt)) =
        empire_params_q.get(viewer_entity)
    else {
        return;
    };
    // Player ruler location: normal mode reads it from the Player-tagged
    // Ruler. Observer mode has no Player entity; fall back to the galaxy
    // capital so the pulse ring still lands somewhere visible.
    let stationed_system = player_q.iter().next().map(|s| s.system).or_else(|| {
        if viewer.is_god_view() {
            stars
                .iter()
                .find(|(_, s, _)| s.is_capital)
                .map(|(e, _, _)| e)
        } else {
            None
        }
    });
    let Some(stationed_system) = stationed_system else {
        return;
    };
    // Keep existing `stationed.system` accesses working after the
    // refactor without restructuring the rest of the function.
    struct StationedRef {
        system: Entity,
    }
    let stationed = StationedRef {
        system: stationed_system,
    };
    let player_system = stationed.system;
    let Ok((_, _player_star, player_pos)) = stars.get(player_system) else {
        return;
    };

    let px = player_pos.x as f32 * view.scale;
    let py = player_pos.y as f32 * view.scale;

    // Capital pulsing ring (larger to match new star sizes)
    let pulse = (clock.as_years_f64() as f32 * 3.0).sin() * 0.3 + 0.7;
    gizmos.circle_2d(Vec2::new(px, py), 20.0, Color::srgba(1.0, 0.84, 0.0, pulse));

    // #176: Build colonized systems set using KnowledgeStore for remote, real-time for local
    let local_colonized: std::collections::HashSet<Entity> = colonies
        .iter()
        .filter_map(|(c, _)| c.system(&planets))
        .collect();

    // Draw rings around colonized stars (only for >= Surveyed tier)
    for (entity, star, star_pos) in &stars {
        // #392: Skip overlay elements for Catalogued-only systems
        let tier = vis_map_opt.map(|vm| vm.get(entity)).unwrap_or_else(|| {
            if star.surveyed {
                SystemVisibilityTier::Surveyed
            } else {
                SystemVisibilityTier::Catalogued
            }
        });
        if !tier.can_see_planets() {
            continue;
        }

        let is_colonized = if entity == player_system {
            local_colonized.contains(&entity)
        } else {
            knowledge
                .get(entity)
                .map(|k| k.data.colonized)
                .unwrap_or(false)
        };
        // #430: Gate is_capital on KnowledgeStore for remote systems
        let effective_capital = if entity == player_system {
            star.is_capital
        } else {
            knowledge
                .get(entity)
                .map(|k| k.data.is_capital)
                .unwrap_or(false)
        };
        if is_colonized && !effective_capital {
            let sx = star_pos.x as f32 * view.scale;
            let sy = star_pos.y as f32 * view.scale;
            gizmos.circle_2d(Vec2::new(sx, sy), 18.0, Color::srgba(0.3, 1.0, 0.3, 0.6));
        }
    }

    for &radius_ly in &[5.0_f32, 10.0, 25.0, 50.0] {
        let radius_px = radius_ly * view.scale;
        gizmos.circle_2d(
            Vec2::new(px, py),
            radius_px,
            Color::srgba(0.3, 0.5, 1.0, 0.15),
        );
    }

    gizmos.circle_2d(
        Vec2::new(px, py),
        5.0 * view.scale,
        Color::srgba(0.2, 1.0, 0.2, 0.25),
    );

    // #17: Information horizon ring (where knowledge becomes older than 5 years)
    let info_horizon_ly = 5.0_f32;
    let info_horizon_px = info_horizon_ly * view.scale;
    let horizon_pulse = (clock.as_years_f64() as f32 * 1.5).sin() * 0.05 + 0.2;
    gizmos.circle_2d(
        Vec2::new(px, py),
        info_horizon_px,
        Color::srgba(1.0, 0.6, 0.0, horizon_pulse),
    );

    // #176: Survey lines use KnowledgeStore for remote systems
    // #392: Only draw for systems with tier >= Surveyed
    for (entity, star, star_pos) in &stars {
        // #434: Don't fall back to star.surveyed — use KnowledgeStore only.
        let tier = vis_map_opt
            .map(|vm| vm.get(entity))
            .unwrap_or(SystemVisibilityTier::Catalogued);
        if !tier.can_see_planets() {
            continue;
        }

        let is_surveyed = if entity == player_system {
            star.surveyed
        } else {
            knowledge
                .get(entity)
                .map(|k| k.data.surveyed)
                .unwrap_or(false)
        };
        // #430: Gate is_capital on KnowledgeStore for remote systems
        let effective_capital_2 = if entity == player_system {
            star.is_capital
        } else {
            knowledge
                .get(entity)
                .map(|k| k.data.is_capital)
                .unwrap_or(false)
        };
        if is_surveyed && !effective_capital_2 {
            let sx = star_pos.x as f32 * view.scale;
            let sy = star_pos.y as f32 * view.scale;
            gizmos.line_2d(
                Vec2::new(px, py),
                Vec2::new(sx, sy),
                Color::srgba(0.4, 0.6, 1.0, 0.15),
            );
        }
    }

    // Selection ring around selected system
    if let Some(selected_entity) = selected.0 {
        if let Ok((_, _star, sel_pos)) = stars.get(selected_entity) {
            let sx = sel_pos.x as f32 * view.scale;
            let sy = sel_pos.y as f32 * view.scale;
            let sel_pulse = (clock.as_years_f64() as f32 * 4.0).sin() * 0.2 + 0.8;
            gizmos.circle_2d(
                Vec2::new(sx, sy),
                22.0,
                Color::srgba(0.0, 1.0, 1.0, sel_pulse),
            );
        }
    }

    // #48: FTL range circle around selected ship
    if let Some(ship_entity) = selected_ship.0 {
        if let Ok((_, ship, state)) = ships.get(ship_entity) {
            let effective_range = ship.ftl_range + global_params.ftl_range_bonus;
            if effective_range > 0.0 {
                let ship_pos = match state {
                    ShipState::InSystem { system } => stars.get(*system).ok().map(|(_, _, pos)| {
                        Vec2::new(pos.x as f32 * view.scale, pos.y as f32 * view.scale)
                    }),
                    _ => None,
                };
                if let Some(ship_pos_px) = ship_pos {
                    let range_px = effective_range as f32 * view.scale;
                    gizmos.circle_2d(ship_pos_px, range_px, Color::srgba(0.3, 0.5, 1.0, 0.1));
                }
            }
        }
    }

    // #52/#56/#176: Hostile presence markers — red X on surveyed systems with hostiles
    // #293: Local system: query (AtSystem, FactionOwner, With<Hostile>) filtered by
    // FactionRelations. Remote: use KnowledgeStore.
    {
        // Local system hostiles (real-time)
        for (at_system, owner) in &hostiles {
            if at_system.0 != player_system {
                continue;
            }
            // Only draw if the empire considers this faction hostile.
            // Hostiles without FactionOwner fall through (drawn) — matches
            // legacy behavior when FactionRelations isn't populated yet.
            if let Some(o) = owner {
                if !faction_relations
                    .get_or_default(viewer_empire, o.0)
                    .can_attack_aggressive()
                {
                    continue;
                }
            }
            let Ok((_, star, star_pos)) = stars.get(at_system.0) else {
                continue;
            };
            if !star.surveyed {
                continue;
            }
            let sx = star_pos.x as f32 * view.scale;
            let sy = star_pos.y as f32 * view.scale;
            let hostile_color = Color::srgba(1.0, 0.2, 0.2, 0.7);
            let s = 5.0_f32;
            gizmos.line_2d(
                Vec2::new(sx - s, sy - s),
                Vec2::new(sx + s, sy + s),
                hostile_color,
            );
            gizmos.line_2d(
                Vec2::new(sx - s, sy + s),
                Vec2::new(sx + s, sy - s),
                hostile_color,
            );
        }
        // Remote system hostiles (from KnowledgeStore)
        for (_entity, k) in knowledge.iter() {
            if k.system == player_system {
                continue;
            }
            if !k.data.has_hostile || !k.data.surveyed {
                continue;
            }
            let Ok((_, _, star_pos)) = stars.get(k.system) else {
                continue;
            };
            let sx = star_pos.x as f32 * view.scale;
            let sy = star_pos.y as f32 * view.scale;
            let hostile_color = Color::srgba(1.0, 0.2, 0.2, 0.7);
            let s = 5.0_f32;
            gizmos.line_2d(
                Vec2::new(sx - s, sy - s),
                Vec2::new(sx + s, sy + s),
                hostile_color,
            );
            gizmos.line_2d(
                Vec2::new(sx - s, sy + s),
                Vec2::new(sx + s, sy - s),
                hostile_color,
            );
        }
    }

    // #46/#176: Port facility markers - draw a diamond icon on systems with ports
    // Local system: read SystemBuildings directly. Remote: use KnowledgeStore.
    {
        // Collect port systems: local from ECS, remote from knowledge
        let mut port_system_entities: Vec<Entity> = Vec::new();
        // Local system ports (real-time, via SystemModifiers)
        if sys_mods_q
            .get(player_system)
            .map(|m| m.port_repair.value().final_value() > crate::amount::Amt::ZERO)
            .unwrap_or(false)
        {
            port_system_entities.push(player_system);
        }
        // Remote system ports (from KnowledgeStore)
        for (_entity, k) in knowledge.iter() {
            if k.system == player_system {
                continue;
            }
            if k.data.has_port {
                port_system_entities.push(k.system);
            }
        }

        for system_entity in &port_system_entities {
            if let Ok((_, _star, star_pos)) = stars.get(*system_entity) {
                let sx = star_pos.x as f32 * view.scale;
                let sy = star_pos.y as f32 * view.scale;
                let port_pulse = (clock.as_years_f64() as f32 * 2.0).sin() * 0.15 + 0.6;
                let d = 6.0_f32;
                let top = Vec2::new(sx, sy + d);
                let right = Vec2::new(sx + d, sy);
                let bottom = Vec2::new(sx, sy - d);
                let left = Vec2::new(sx - d, sy);
                let port_color = Color::srgba(0.8, 0.5, 1.0, port_pulse);
                gizmos.line_2d(top, right, port_color);
                gizmos.line_2d(right, bottom, port_color);
                gizmos.line_2d(bottom, left, port_color);
                gizmos.line_2d(left, top, port_color);
            }
        }
    }

    // #395: Station infrastructure icons — draw small icons next to system names
    // for each immobile ship (station) docked there.
    // #434: Only draw for the local system (real-time) or systems with
    // knowledge — don't leak NPC station positions from ground truth.
    {
        let mut system_stations: HashMap<Entity, Vec<StationKind>> = HashMap::new();

        // Local system: use ECS ground truth for player-owned stations.
        for (_, ship, state) in &ships {
            if !ship.is_immobile() {
                continue;
            }
            let sys = match &*state {
                ShipState::InSystem { system } => *system,
                ShipState::Refitting { system, .. } => *system,
                _ => continue,
            };
            // Only show stations at the player's local system from ground truth.
            if sys != player_system {
                continue;
            }
            system_stations
                .entry(sys)
                .or_default()
                .push(station_kind_from_hull(&ship.hull_id));
        }

        // Remote systems: derive station icons from KnowledgeStore flags
        // (has_port, has_shipyard). Detailed per-ship info isn't available
        // in snapshots, so we show the corresponding icon types.
        for (_entity, k) in knowledge.iter() {
            if k.system == player_system {
                continue;
            }
            let mut kinds = Vec::new();
            if k.data.has_shipyard {
                kinds.push(StationKind::Shipyard);
            }
            if k.data.has_port {
                kinds.push(StationKind::Port);
            }
            if !kinds.is_empty() {
                system_stations.insert(k.system, kinds);
            }
        }

        for (sys_entity, kinds) in &system_stations {
            let Ok((_, _star, star_pos)) = stars.get(*sys_entity) else {
                continue;
            };
            let sx = star_pos.x as f32 * view.scale;
            // Position icons below the system name label (name is at y+14, so start at y+24)
            let base_y = star_pos.y as f32 * view.scale + 24.0;
            let icon_spacing = 10.0;
            let start_x = sx - (kinds.len() as f32 - 1.0) * icon_spacing / 2.0;

            for (i, kind) in kinds.iter().enumerate() {
                let ix = start_x + i as f32 * icon_spacing;
                let iy = base_y;
                draw_station_icon(&mut gizmos, Vec2::new(ix, iy), *kind);
            }
        }
    }
}

/// Classification of station types for icon rendering.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StationKind {
    Core,
    Shipyard,
    Port,
    ResearchLab,
    Unknown,
}

/// Classify a station by its hull_id substring.
fn station_kind_from_hull(hull_id: &str) -> StationKind {
    if hull_id.contains("core") {
        StationKind::Core
    } else if hull_id.contains("shipyard") {
        StationKind::Shipyard
    } else if hull_id.contains("port") {
        StationKind::Port
    } else if hull_id.contains("research") {
        StationKind::ResearchLab
    } else {
        StationKind::Unknown
    }
}

/// Draw a small gizmo icon for a station type.
fn draw_station_icon(gizmos: &mut Gizmos, center: Vec2, kind: StationKind) {
    let s = 3.5;
    match kind {
        StationKind::Core => {
            // Filled-looking circle (double ring)
            let gold = Color::srgba(1.0, 0.84, 0.0, 0.8);
            gizmos.circle_2d(center, s, gold);
            gizmos.circle_2d(center, s * 0.5, gold);
        }
        StationKind::Shipyard => {
            // Crossed lines (anvil/hammer shape)
            let cyan = Color::srgba(0.3, 0.9, 1.0, 0.8);
            gizmos.line_2d(
                center + Vec2::new(-s, 0.0),
                center + Vec2::new(s, 0.0),
                cyan,
            );
            gizmos.line_2d(
                center + Vec2::new(0.0, -s),
                center + Vec2::new(0.0, s),
                cyan,
            );
            gizmos.line_2d(
                center + Vec2::new(-s * 0.7, -s * 0.7),
                center + Vec2::new(s * 0.7, s * 0.7),
                cyan,
            );
        }
        StationKind::Port => {
            // Small diamond
            let purple = Color::srgba(0.8, 0.5, 1.0, 0.8);
            let top = center + Vec2::new(0.0, s);
            let right = center + Vec2::new(s, 0.0);
            let bottom = center + Vec2::new(0.0, -s);
            let left = center + Vec2::new(-s, 0.0);
            gizmos.line_2d(top, right, purple);
            gizmos.line_2d(right, bottom, purple);
            gizmos.line_2d(bottom, left, purple);
            gizmos.line_2d(left, top, purple);
        }
        StationKind::ResearchLab => {
            // Triangle pointing up
            let green = Color::srgba(0.3, 1.0, 0.5, 0.8);
            let top = center + Vec2::new(0.0, s);
            let bl = center + Vec2::new(-s * 0.87, -s * 0.5);
            let br = center + Vec2::new(s * 0.87, -s * 0.5);
            gizmos.line_2d(top, bl, green);
            gizmos.line_2d(bl, br, green);
            gizmos.line_2d(br, top, green);
        }
        StationKind::Unknown => {
            // Simple square
            let gray = Color::srgba(0.6, 0.6, 0.6, 0.6);
            let tl = center + Vec2::new(-s, s);
            let tr = center + Vec2::new(s, s);
            let br = center + Vec2::new(s, -s);
            let bl = center + Vec2::new(-s, -s);
            gizmos.line_2d(tl, tr, gray);
            gizmos.line_2d(tr, br, gray);
            gizmos.line_2d(br, bl, gray);
            gizmos.line_2d(bl, tl, gray);
        }
    }
}

pub fn draw_deep_space_structures(
    mut gizmos: Gizmos,
    structures: Query<(
        &DeepSpaceStructure,
        &Position,
        &StructureHitpoints,
        Option<&ConstructionPlatform>,
        Option<&Scrapyard>,
    )>,
    view: Res<GalaxyView>,
) {
    for (_structure, pos, _hp, platform, scrap) in &structures {
        let x = pos.x as f32 * view.scale;
        let y = pos.y as f32 * view.scale;
        // Draw a small diamond marker. #229: colour encodes lifecycle state —
        // yellow while a ConstructionPlatform is accumulating resources, red
        // while the structure is a drained/being-drained Scrapyard, blue for
        // fully active structures.
        let size = 4.0;
        let color = if platform.is_some() {
            Color::srgba(1.0, 0.9, 0.2, 0.85) // yellow — under construction
        } else if scrap.is_some() {
            Color::srgba(1.0, 0.3, 0.3, 0.85) // red — dismantled / scrapyard
        } else {
            Color::srgba(0.7, 0.7, 1.0, 0.6) // blue — active
        };
        gizmos.line_2d(Vec2::new(x, y - size), Vec2::new(x + size, y), color);
        gizmos.line_2d(Vec2::new(x + size, y), Vec2::new(x, y + size), color);
        gizmos.line_2d(Vec2::new(x, y + size), Vec2::new(x - size, y), color);
        gizmos.line_2d(Vec2::new(x - size, y), Vec2::new(x, y - size), color);
    }
}

/// #145: Draw forbidden regions as a loose union of 2D discs.
///
/// Each effective sphere (`sphere_radius / sqrt(threshold)`) is rendered as a
/// filled-looking circle via nested outlines at decreasing alpha. A proper
/// metaball iso-surface shader is a 1.0.0 task — this is intentionally coarse
/// but makes the no-go volume unambiguous on the galaxy map.
pub fn draw_forbidden_regions(
    mut gizmos: Gizmos,
    regions: Query<&crate::galaxy::ForbiddenRegion>,
    region_types: Res<crate::galaxy::RegionTypeRegistry>,
    view: Res<GalaxyView>,
) {
    for region in &regions {
        // Visual style from the type definition (fallback to muted purple).
        let (r, g, b, density) = match region_types.types.get(&region.type_id) {
            Some(t) => (
                t.visual_color[0],
                t.visual_color[1],
                t.visual_color[2],
                t.visual_density,
            ),
            None => (0.3, 0.1, 0.5, 0.6),
        };
        let base_alpha = (density * 0.6).clamp(0.1, 0.9);

        for (center, radius) in &region.spheres {
            let eff = crate::galaxy::effective_radius(*radius, region.threshold);
            if eff <= 0.0 {
                continue;
            }
            let cx = center[0] as f32 * view.scale;
            let cy = center[1] as f32 * view.scale;
            let screen_r = eff as f32 * view.scale;

            // Nested rings: outer faint, inner denser.
            for (i, frac) in [1.0_f32, 0.75, 0.5, 0.25].iter().enumerate() {
                let alpha = base_alpha * (0.25 + i as f32 * 0.18);
                gizmos.circle_2d(
                    Vec2::new(cx, cy),
                    screen_r * frac,
                    Color::srgba(r, g, b, alpha),
                );
            }
        }
    }
}
