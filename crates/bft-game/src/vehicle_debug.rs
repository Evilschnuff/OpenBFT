//! Gizmo and UI overlays that show what the player vehicle is doing.
//!
//! A debug aid, off by default: the vehicle draws its own model
//! ([`crate::player_model`]), and these overlays show which way the hull
//! faces and whether the suspension is touching the ground.
//!
//! Three things are drawn:
//!
//! - **the hull box**, in the form's colour, with its forward face marked so
//!   "which way am I pointing" is answerable at a glance;
//! - **the suspension rays**, green where a spring is touching ground and red
//!   where it is not, which is the controller's own [`GroundState`] made
//!   visible;
//! - **the turret**, as a line along its aim from the `aimy` height, so the
//!   mouse has something to move;
//!
//! All of it is `bft-game` rather than `bft-sim` for the same reason the
//! physics overlay is: gizmos and UI are `bevy_render`, and the sim crate
//! builds without a renderer (NFR-4).

use bevy::prelude::*;
use bft_sim::physics::DebugFlags;
use bft_sim::vehicle::{
    GroundState, Traction, Turret, Vehicle, VehicleForm, VehicleTable, hull_box,
};

/// The colour a form's hull draws in.
///
/// One per form, so a transformation is visible as well as audible: the four
/// are far enough apart in hue to tell at a glance against the levels'
/// sand-and-sea palette.
pub fn form_color(form: VehicleForm) -> Color {
    match form {
        VehicleForm::Track => Color::srgb(0.95, 0.85, 0.2),
        VehicleForm::Amphib => Color::srgb(0.2, 0.8, 0.95),
        VehicleForm::Hover => Color::srgb(0.7, 0.4, 1.0),
        VehicleForm::Sub => Color::srgb(0.2, 1.0, 0.5),
    }
}

/// The hull's colour while the tracks are sliding.
///
/// One colour for every form, unlike the gripping colours: a skid is a state
/// worth spotting instantly, and "the box went red" is easier to read at
/// speed than a shift of hue.
pub const SLIDING_COLOR: Color = Color::srgb(1.0, 0.25, 0.15);

/// A suspension ray that found ground.
pub const GROUNDED_COLOR: Color = Color::srgb(0.2, 1.0, 0.3);
/// A suspension ray with nothing under it.
pub const AIRBORNE_COLOR: Color = Color::srgb(1.0, 0.3, 0.2);
/// The turret's aim line.
pub const TURRET_COLOR: Color = Color::srgb(1.0, 0.4, 0.1);

/// The longest the slip arrow grows, in metres.
///
/// // approximation: a cap so a fast skid does not draw a line across the
/// level; the arrow is for direction and rough magnitude.
pub const SLIP_ARROW_MAX: f32 = 8.0;

/// How long the turret's aim line is, in metres.
///
/// // approximation: long enough to read against the terrain, short enough
/// not to reach the fog. No reader field corresponds — the original drew a
/// model here.
pub const AIM_LINE: f32 = 12.0;

/// The vehicle overlays.
pub struct VehicleDebugPlugin;

impl Plugin for VehicleDebugPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (draw_vehicle, draw_turret));
    }
}

/// Outline the hull and its suspension rays.
pub fn draw_vehicle(
    mut gizmos: Gizmos,
    flags: Res<DebugFlags>,
    table: Option<Res<VehicleTable>>,
    vehicles: Query<(&Vehicle, &GlobalTransform, &GroundState, &Traction)>,
) {
    if !flags.vehicle {
        return;
    }

    for (vehicle, transform, ground, traction) in &vehicles {
        let mode = table
            .as_ref()
            .and_then(|table| table.kind(&vehicle.kind))
            .and_then(|def| def.mode(vehicle.form));

        // Red while sliding, the form's own colour while gripping, so the
        // moment traction breaks is visible without reading a number.
        let color = if traction.is_skidding() {
            SLIDING_COLOR
        } else {
            form_color(vehicle.form)
        };
        let (half, centre) = hull_box(mode);
        let (_, rotation, translation) = transform.to_scale_rotation_translation();

        // The hull box, where the collider actually is.
        gizmos.cube(
            Transform {
                translation: translation + rotation * centre,
                rotation,
                scale: half * 2.0,
            },
            color,
        );

        // Which way is forward. A line out of the nose, plus a cross bar, so
        // the front is distinguishable from the back at any angle — a bare
        // box is symmetric and tells you nothing about heading.
        let nose = translation + rotation * (centre + Vec3::NEG_Z * half.z);
        let tip = nose + rotation * (Vec3::NEG_Z * half.z * 0.6);
        gizmos.line(nose, tip, color);
        gizmos.line(
            tip + rotation * (Vec3::X * half.x * 0.4),
            tip + rotation * (Vec3::NEG_X * half.x * 0.4),
            color,
        );

        // The suspension rays, coloured by whether the controller found
        // ground. This is `GroundState` made visible: a vehicle bouncing for
        // no clear reason is a vehicle whose rays are red.
        let Some(mode) = mode else {
            continue;
        };
        let ray_color = if ground.grounded {
            GROUNDED_COLOR
        } else {
            AIRBORNE_COLOR
        };

        for ray in &mode.suspension.rays {
            let origin = translation + rotation * *ray;
            let end = origin + rotation * (Vec3::NEG_Y * mode.suspension.max_len);
            gizmos.line(origin, end, ray_color);
        }

        // While sliding, one arrow along the slip: which way the vehicle is
        // actually being carried, and how hard. `slip_speed` is signed along
        // the hull's right axis, so the arrow points the way it is going.
        if traction.is_skidding() {
            let from = translation + rotation * centre;
            let slip = traction.slip_speed.clamp(-SLIP_ARROW_MAX, SLIP_ARROW_MAX);
            gizmos.arrow(from, from + rotation * Vec3::X * slip, SLIDING_COLOR);
        }
    }
}

/// Draw the turret's aim, from the vehicle's `aimy` height.
pub fn draw_turret(
    mut gizmos: Gizmos,
    flags: Res<DebugFlags>,
    table: Option<Res<VehicleTable>>,
    vehicles: Query<(&Vehicle, &GlobalTransform, &Turret)>,
) {
    if !flags.vehicle {
        return;
    }

    for (vehicle, transform, turret) in &vehicles {
        let def = table.as_ref().and_then(|table| table.kind(&vehicle.kind));
        // `aimy` is where the original puts the turret, so it is where the
        // aim line starts.
        let height = def.map(|def| def.aim_y[0]).unwrap_or(2.0);

        let (_, hull_rotation, translation) = transform.to_scale_rotation_translation();
        let aim = hull_rotation * Quat::from_euler(EulerRot::YXZ, turret.yaw, turret.pitch, 0.0);

        let base = translation + Vec3::Y * height;
        gizmos.line(base, base + aim * (Vec3::NEG_Z * AIM_LINE), TURRET_COLOR);
        // A short bar across the muzzle end, so pitch is readable and not
        // just foreshortening.
        let muzzle = base + aim * (Vec3::NEG_Z * AIM_LINE);
        gizmos.line(
            muzzle + aim * (Vec3::X * 0.8),
            muzzle + aim * (Vec3::NEG_X * 0.8),
            TURRET_COLOR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each form has to be distinguishable, or the overlay cannot answer
    /// "did the transformation happen" — which is the thing it is most often
    /// asked while driving into water.
    #[test]
    fn every_form_draws_in_its_own_colour() {
        let colours: Vec<[f32; 4]> = VehicleForm::ALL
            .iter()
            .map(|form| form_color(*form).to_srgba().to_f32_array())
            .collect();

        for (index, colour) in colours.iter().enumerate() {
            for other in &colours[index + 1..] {
                let distance: f32 = colour
                    .iter()
                    .zip(other)
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum();
                assert!(
                    distance > 0.1,
                    "two forms draw in nearly the same colour: {colour:?} and {other:?}"
                );
            }
        }
    }

    /// Grounded and airborne rays must not read the same, since telling them
    /// apart is the whole point of drawing them.
    #[test]
    fn a_grounded_ray_does_not_look_like_an_airborne_one() {
        assert_ne!(
            GROUNDED_COLOR.to_srgba().to_f32_array(),
            AIRBORNE_COLOR.to_srgba().to_f32_array()
        );
    }
}
