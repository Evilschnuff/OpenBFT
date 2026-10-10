//! Gizmo overlays for what the sim collides with.
//!
//! The toggles are [`DebugFlags::physics`](bft_sim::physics::DebugFlags) and
//! `DebugFlags::water`, a `bft-sim` resource; the dev tools' inspector flips
//! them. The *drawing*
//! lives here rather than in `bft-sim` because gizmos are `bevy_render`, and
//! `bft-sim` builds without a renderer (NFR-4) — a dedicated server has the
//! flag and simply nothing that reads it.
//!
//! Two overlays:
//!
//! - collider outlines, in a colour per dominant soil, so "does the collision
//!   geometry match what I see" is answerable by eye;
//! - water volumes: the footprint at the surface height, plus a drop line to
//!   the floor where the volume is divable.
//!
//! # Why not avian's own debug renderer
//!
//! `avian3d`'s `debug-plugin` feature pulls `bevy_render` into `bft-sim`'s
//! dependency tree, which is exactly what NFR-4 and CI's
//! `headless-sim` job forbid. Drawing from `bft-game` costs one query and
//! keeps the sim crate render-free.

use avian3d::prelude::*;
use bevy::prelude::*;
use bft_assets::materials::Soil;
use bft_sim::physics::{DebugFlags, SurfaceInfo};
use bft_sim::water::{LiquidKind, WaterVolume};

/// How far from the camera a collider is still outlined.
///
/// A level has 1000–1900 colliders and drawing all of them costs more than
/// the frame it would debug; the levels' own fog range is 320–900 m, so this
/// is "what you can see" with room to spare.
pub const DRAW_RANGE: f32 = 250.0;

/// The colour a soil draws in.
fn soil_color(soil: Soil) -> Color {
    match soil {
        Soil::Water => Color::srgb(0.2, 0.5, 1.0),
        Soil::Lava => Color::srgb(1.0, 0.3, 0.0),
        Soil::Quicksand => Color::srgb(0.8, 0.7, 0.3),
        Soil::Fire => Color::srgb(1.0, 0.6, 0.1),
        // Everything else is ground. Recoil only ever sets five soils
        // (docs/data/materials.md), so the rest is one colour rather than fourteen.
        _ => Color::srgb(0.3, 1.0, 0.4),
    }
}

/// The colour a liquid volume draws in.
fn liquid_color(kind: LiquidKind) -> Color {
    match kind {
        LiquidKind::Water => Color::srgb(0.2, 0.6, 1.0),
        LiquidKind::Lava => Color::srgb(1.0, 0.35, 0.05),
        LiquidKind::Quicksand => Color::srgb(0.85, 0.75, 0.35),
    }
}

/// Outline every nearby collider in its dominant soil's colour.
///
/// The outline is the collider's AABB, not its triangles: a level's trimeshes
/// run to tens of thousands of triangles each and a per-triangle overlay
/// costs more than the problem it diagnoses. The box is enough to answer
/// "is there collision geometry here, and is it the right size".
pub fn draw_colliders(
    mut gizmos: Gizmos,
    flags: Res<DebugFlags>,
    cameras: Query<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>,
    colliders: Query<(&ColliderAabb, Option<&SurfaceInfo>), With<Collider>>,
) {
    if !flags.physics {
        return;
    }

    let Ok(camera) = cameras.single() else {
        return;
    };
    let eye = camera.translation();

    for (aabb, surface) in &colliders {
        let center = (aabb.min + aabb.max) * 0.5;

        if center.distance(eye) > DRAW_RANGE {
            continue;
        }

        let color = surface
            .map(|surface| soil_color(surface.dominant))
            .unwrap_or(Color::srgb(0.6, 0.6, 0.6));

        gizmos.cube(
            Transform::from_translation(center).with_scale(aabb.max - aabb.min),
            color,
        );
    }
}

/// Draw each water volume's footprint at the surface, and its depth.
///
/// The footprint is the volume's own triangle list in x/z, so what is drawn
/// is exactly what [`water_at`](bft_sim::water::water_at) will answer for —
/// an overlay built from the node names instead would agree with the sim only
/// by luck.
pub fn draw_water(
    mut gizmos: Gizmos,
    flags: Res<DebugFlags>,
    cameras: Query<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>,
    volumes: Query<&WaterVolume>,
) {
    if !flags.water {
        return;
    }

    let Ok(camera) = cameras.single() else {
        return;
    };
    let eye = camera.translation();

    for volume in &volumes {
        let color = liquid_color(volume.kind);

        for triangle in &volume.footprint {
            let corners: [Vec3; 3] =
                std::array::from_fn(|i| Vec3::new(triangle[i].x, volume.surface_y, triangle[i].y));

            if corners[0].distance(eye) > DRAW_RANGE {
                continue;
            }

            gizmos.line(corners[0], corners[1], color);
            gizmos.line(corners[1], corners[2], color);
            gizmos.line(corners[2], corners[0], color);

            // A divable volume gets a line down to its floor at each corner,
            // so deep water reads as a volume rather than a surface.
            if let Some(floor_y) = volume.floor_y {
                gizmos.line(
                    corners[0],
                    Vec3::new(corners[0].x, floor_y, corners[0].z),
                    color.with_alpha(0.4),
                );
            }
        }
    }
}

/// The debug-overlay plugin.
pub struct PhysicsDebugPlugin;

impl Plugin for PhysicsDebugPlugin {
    fn build(&self, app: &mut App) {
        // The conditions as well as the early returns inside: a system whose
        // condition fails is not even scheduled, so an overlay that is off
        // costs nothing.
        app.add_systems(
            Update,
            (
                draw_colliders.run_if(|flags: Res<DebugFlags>| flags.physics),
                draw_water.run_if(|flags: Res<DebugFlags>| flags.water),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five soils Recoil actually uses must be visually distinct, or the
    /// overlay cannot answer the question it exists for.
    #[test]
    fn each_used_soil_draws_in_its_own_colour() {
        let colors: Vec<Srgba> = [
            Soil::Default,
            Soil::Water,
            Soil::Lava,
            Soil::Quicksand,
            Soil::Fire,
        ]
        .into_iter()
        .map(|soil| Srgba::from(soil_color(soil)))
        .collect();

        for (i, a) in colors.iter().enumerate() {
            for b in &colors[i + 1..] {
                assert_ne!(a, b, "two used soils share a colour");
            }
        }
    }

    /// The unused soils are one colour on purpose: nothing in Recoil sets
    /// them, and fourteen entries would suggest they mean something.
    #[test]
    fn the_unused_soils_all_read_as_ground() {
        for soil in [Soil::Dirt, Soil::Grass, Soil::Snow, Soil::NoSlip] {
            assert_eq!(soil_color(soil), soil_color(Soil::Default));
        }
    }
}
