//! The view from a tether-guided missile.
//!
//! The upgraded missile and nuke (`wep_8.1`, `wep_9.1`) are steered in
//! flight. The manual: "After you launch a Tether-Guided Missile, you must
//! guide to its target, steering it up, down, left and right. A remote
//! camera allows for you to see from the point of view of the missile.
//! Press the launch-key again to return your camera view to the BFT."
//!
//! While the local player's missile carries
//! [`Guided`](bft_sim::weapons::Guided):
//!
//! - the play camera rides fixed right over the end of the missile, just
//!   above it and looking the way it flies, so the missile runs forward from
//!   the bottom of the frame and its exhaust is out of view;
//! - the missile is drawn half transparent, so it does not hide what it is
//!   being steered at;
//! - a green filter lies over the view, under the HUD;
//! - where the cursor sits relative to the window's centre steers it —
//!   `input_map` writes that into
//!   [`PlayerInput::guide`](bft_sim::vehicle::PlayerInput::guide) with
//!   [`guide_from_cursor`] — and the tank is left alone: no driving, no
//!   turret, no fire. The launch key lets go.
//!
//! When the tether is cut — let go, or the missile hit something — the view
//! goes back to the chase camera where it left off.

use avian3d::prelude::SpatialQuery;
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bft_sim::states::AppState;
use bft_sim::vehicle::Controlled;
use bft_sim::weapons::{Guided, Projectile};

use crate::camera::FlyCamera;
use crate::camera_tuning::GuidedViewOverrides;
use crate::chase_camera::{COLLISION_RADIUS, ChaseCamera, clear_distance, follow_target};

/// How far behind the tail end of the missile the camera rides, metres:
/// zero is straight over the end, negative forward along the body.
///
/// // approximation: as the original is remembered, the view sits right on
/// the end of the missile, far enough forward that its exhaust is not in
/// view. Measured from the model's bounds, not its origin, which happens to
/// be at the tail for `miss_tg.flt` (the body runs 4.8 m forward from it;
/// `nuke_tg.flt` 3.15 m).
pub const GUIDED_BACK: f32 = 0.0;

/// How far above the top of the missile the camera rides, metres.
///
/// // approximation: no reader field corresponds. Just clear of the body, so
/// its front half runs from the bottom of the frame toward the centre.
pub const GUIDED_UP: f32 = 0.4;

/// How opaque the guided missile is drawn.
///
/// // approximation: "half transparent", as it is remembered from the
/// original.
pub const GUIDED_MISSILE_ALPHA: f32 = 0.5;

/// The filter over the view: sRGB red, green, blue and opacity.
///
/// // approximation: a green wash, as it is remembered from the original;
/// no reader field or texture names it.
pub const GUIDED_FILTER: [f32; 4] = [0.15, 0.85, 0.2, 0.28];

/// The fraction of the half-window around the centre that steers nothing.
///
/// // approximation: no reader field corresponds. Small, so steering stays
/// proportional, but enough that a cursor left near the middle flies
/// straight rather than drifting.
pub const GUIDED_DEAD_ZONE: f32 = 0.04;

/// The local player's missile that is being guided, if one is.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GuidedMissile(pub Option<Entity>);

impl GuidedMissile {
    /// Whether the view and the controls are on a missile.
    pub fn is_guiding(&self) -> bool {
        self.0.is_some()
    }
}

/// A guided missile's mesh drawn translucent, and the material it had.
#[derive(Component, Debug, Clone)]
struct Faded(Handle<StandardMaterial>);

/// The green filter over the view while guiding.
#[derive(Component, Debug, Clone, Copy)]
struct GuidedFilter;

/// The guided missile's view, look and filter.
pub struct GuidedViewPlugin;

impl Plugin for GuidedViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GuidedMissile>()
            .add_systems(
                PreUpdate,
                // Before the input is read, which steers by it.
                track_guided_missile.before(crate::input_map::read_input),
            )
            .add_systems(
                Update,
                (
                    ride_the_missile.after(follow_target),
                    fade_the_missile,
                    show_the_filter,
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), forget_the_missile);
    }
}

/// Find the local player's guided missile.
pub fn track_guided_missile(
    mut guided: ResMut<GuidedMissile>,
    missiles: Query<(Entity, &Projectile), With<Guided>>,
    owners: Query<&Controlled>,
) {
    let found = missiles
        .iter()
        .find(|(_, missile)| owners.get(missile.owner) == Ok(&Controlled::Local))
        .map(|(entity, _)| entity);

    guided.set_if_neq(GuidedMissile(found));
}

fn forget_the_missile(mut guided: ResMut<GuidedMissile>) {
    guided.set_if_neq(GuidedMissile(None));
}

/// How hard the cursor steers: its offset from the window's centre, −1..1
/// on each axis, `x` right and `y` up.
///
/// Scaled by the *smaller* half-extent on both axes, so the same distance
/// from the centre steers as hard sideways as up and down on a wide window,
/// and clamped at full deflection. Inside the dead zone it is zero, and the
/// deflection ramps from there rather than jumping.
pub fn guide_from_cursor(cursor: Vec2, size: Vec2, dead_zone: f32) -> Vec2 {
    let half = size * 0.5;
    let reach = half.min_element();
    if reach <= 0.0 {
        return Vec2::ZERO;
    }

    // Window y runs down the screen; up is positive here.
    let offset = (cursor - half) * Vec2::new(1.0, -1.0) / reach;
    let offset = offset.clamp(Vec2::NEG_ONE, Vec2::ONE);

    let dead_zone = dead_zone.clamp(0.0, 0.99);
    let ramp = |value: f32| {
        let excess = (value.abs() - dead_zone).max(0.0) / (1.0 - dead_zone);
        excess * value.signum()
    };

    Vec2::new(ramp(offset.x), ramp(offset.y))
}

/// The missile's model in its own space: the ends of its body along its
/// length and the height of its top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelExtent {
    /// The nose: the most forward point, along −z.
    pub front: f32,
    /// The tail: the most rearward point, along +z.
    pub tail: f32,
    /// The top, along +y.
    pub top: f32,
}

impl ModelExtent {
    /// How long the body is.
    pub fn length(&self) -> f32 {
        self.tail - self.front
    }
}

/// Where the missile's model reaches, in its own space.
///
/// From the bounds of the meshes under it, which the renderer computes.
/// `None` until they exist, the first frame or two after launch.
pub fn model_extent<'a>(
    missile: &GlobalTransform,
    parts: impl IntoIterator<Item = (&'a Aabb, &'a GlobalTransform)>,
) -> Option<ModelExtent> {
    let to_missile = missile.affine().inverse();
    let mut extent: Option<ModelExtent> = None;

    for (aabb, global) in parts {
        let (center, half) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
        for corner in 0..8 {
            let sign = Vec3::new(
                if corner & 1 == 0 { -1.0 } else { 1.0 },
                if corner & 2 == 0 { -1.0 } else { 1.0 },
                if corner & 4 == 0 { -1.0 } else { 1.0 },
            );
            let world = global.transform_point(center + half * sign);
            let local = to_missile.transform_point3(world);
            extent = Some(match extent {
                None => ModelExtent {
                    front: local.z,
                    tail: local.z,
                    top: local.y,
                },
                Some(extent) => ModelExtent {
                    front: extent.front.min(local.z),
                    tail: extent.tail.max(local.z),
                    top: extent.top.max(local.y),
                },
            });
        }
    }

    extent
}

/// Where the camera rides on a missile of this extent: `back` behind the
/// tail end of its body and `up` above its top.
///
/// Without an extent yet, over its origin.
pub fn guided_eye(extent: Option<ModelExtent>, back: f32, up: f32) -> Vec3 {
    let (tail, top) = extent.map_or((0.0, 0.0), |extent| (extent.tail, extent.top));

    Vec3::new(0.0, top + up, tail + back)
}

/// The camera at `eye`, in the missile's own space, looking the way it
/// flies.
///
/// The missile's own space, so it keeps its place in the frame as it climbs
/// and dives.
pub fn guided_viewpoint(missile: &Transform, eye: Vec3) -> Transform {
    Transform::from_translation(missile.transform_point(eye)).with_rotation(missile.rotation)
}

/// Put the play camera on the guided missile, over the end of it.
///
/// After [`follow_target`], which has already placed it behind the hull this
/// frame and keeps the chase camera's own state moving so the view picks up
/// from there when the tether is cut.
#[allow(clippy::type_complexity)]
pub fn ride_the_missile(
    guided: Res<GuidedMissile>,
    spatial: SpatialQuery,
    missiles: Query<(&Transform, &GlobalTransform), (With<Projectile>, Without<ChaseCamera>)>,
    children: Query<&Children>,
    parts: Query<(&Aabb, &GlobalTransform)>,
    mut cameras: Query<(&ChaseCamera, &mut Transform), Without<FlyCamera>>,
) {
    let Some((entity, (missile, global))) = guided
        .0
        .and_then(|entity| Some((entity, missiles.get(entity).ok()?)))
    else {
        return;
    };
    let extent = model_extent(
        global,
        children
            .iter_descendants(entity)
            .filter_map(|part| parts.get(part).ok()),
    );

    for (camera, mut transform) in &mut cameras {
        let overrides = &camera.overrides.guided;
        let view = guided_viewpoint(
            missile,
            guided_eye(
                extent,
                overrides.back.unwrap_or(GUIDED_BACK),
                overrides.up.unwrap_or(GUIDED_UP),
            ),
        );

        // Kept out of walls the same way the chase camera is: a missile
        // skimming a cliff must not put the view inside it.
        let radius = camera
            .overrides
            .collision_radius
            .unwrap_or(COLLISION_RADIUS);
        let clear = clear_distance(&spatial, missile.translation, view.translation, radius);
        let offset = view.translation - missile.translation;

        *transform =
            view.with_translation(missile.translation + offset.normalize_or_zero() * clear);
    }
}

/// Draw the guided missile half transparent, and put it back once let go.
///
/// Every frame rather than once: the missile's model is cloned under it
/// after it spawns and its meshes are attached later still, so a single pass
/// at launch would find nothing to fade.
#[allow(clippy::type_complexity)]
fn fade_the_missile(
    mut commands: Commands,
    guided: Res<GuidedMissile>,
    mut faded_missile: Local<Option<Entity>>,
    children: Query<&Children>,
    mut meshes: Query<(&mut MeshMaterial3d<StandardMaterial>, Option<&Faded>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    cameras: Query<&ChaseCamera>,
) {
    // Let go, and still flying: its own materials back.
    if let Some(previous) = *faded_missile
        && guided.0 != Some(previous)
    {
        for entity in children.iter_descendants(previous) {
            if let Ok((mut material, Some(Faded(original)))) = meshes.get_mut(entity) {
                material.0 = original.clone();
                commands.entity(entity).remove::<Faded>();
            }
        }
        *faded_missile = None;
    }

    let Some(missile) = guided.0 else {
        return;
    };
    *faded_missile = Some(missile);

    let alpha = cameras
        .iter()
        .find_map(|camera| camera.overrides.guided.missile_alpha)
        .unwrap_or(GUIDED_MISSILE_ALPHA)
        .clamp(0.0, 1.0);

    for entity in children.iter_descendants(missile) {
        let Ok((mut material, None)) = meshes.get_mut(entity) else {
            continue;
        };
        // A copy of its own, blended: the level's other copies of this model
        // share the original.
        let Some(mut translucent) = materials.get(&material.0).cloned() else {
            continue;
        };
        let base = translucent.base_color.to_srgba();
        translucent.base_color = base.with_alpha(base.alpha * alpha).into();
        translucent.alpha_mode = AlphaMode::Blend;

        let original = std::mem::replace(&mut material.0, materials.add(translucent));
        commands.entity(entity).insert(Faded(original));
    }
}

/// Lay the green filter over the view while guiding, under the HUD.
fn show_the_filter(
    mut commands: Commands,
    guided: Res<GuidedMissile>,
    filters: Query<Entity, With<GuidedFilter>>,
    cameras: Query<&ChaseCamera>,
) {
    let shown = !filters.is_empty();

    if guided.is_guiding() && !shown {
        let overrides: GuidedViewOverrides = cameras
            .iter()
            .next()
            .map(|camera| camera.overrides.guided.clone())
            .unwrap_or_default();
        let [red, green, blue, alpha] = overrides.filter.unwrap_or(GUIDED_FILTER);

        commands.spawn((
            Name::new("guided missile filter"),
            GuidedFilter,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::srgba(red, green, blue, alpha)),
            // Below every root node, so the HUD draws over it.
            GlobalZIndex(-1),
            DespawnOnExit(AppState::InGame),
        ));
    } else if !guided.is_guiding() {
        for filter in &filters {
            commands.entity(filter).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Vec2 = Vec2::new(1600.0, 900.0);

    #[test]
    fn the_centre_steers_nothing() {
        assert_eq!(guide_from_cursor(WINDOW * 0.5, WINDOW, 0.04), Vec2::ZERO);
        // Just inside the dead zone either way.
        let near = WINDOW * 0.5 + Vec2::new(10.0, -10.0);
        assert_eq!(guide_from_cursor(near, WINDOW, 0.04), Vec2::ZERO);
    }

    /// Window y runs down the screen; the guide's y runs up.
    #[test]
    fn right_is_positive_x_and_the_top_is_positive_y() {
        let right = guide_from_cursor(Vec2::new(1400.0, 450.0), WINDOW, 0.0);
        let top = guide_from_cursor(Vec2::new(800.0, 100.0), WINDOW, 0.0);

        assert!(right.x > 0.0 && right.y == 0.0, "{right:?}");
        assert!(top.y > 0.0 && top.x == 0.0, "{top:?}");
    }

    /// Proportional to the offset, the same scale on both axes, and full at
    /// the top edge.
    #[test]
    fn steering_is_proportional_and_clamps_at_full() {
        // A quarter of the half-height off centre, sideways and up.
        let side = guide_from_cursor(Vec2::new(800.0 + 112.5, 450.0), WINDOW, 0.0);
        let up = guide_from_cursor(Vec2::new(800.0, 450.0 - 112.5), WINDOW, 0.0);
        assert!((side.x - 0.25).abs() < 1e-5, "{side:?}");
        assert!((up.y - 0.25).abs() < 1e-5, "{up:?}");

        let top = guide_from_cursor(Vec2::new(800.0, 0.0), WINDOW, 0.0);
        assert!((top.y - 1.0).abs() < 1e-5, "{top:?}");
        // Past the half-height sideways, on a wide window: clamped.
        let far = guide_from_cursor(Vec2::new(1600.0, 450.0), WINDOW, 0.0);
        assert_eq!(far.x, 1.0);
    }

    /// Leaving the dead zone ramps from zero rather than jumping.
    #[test]
    fn the_dead_zone_ramps_rather_than_jumps() {
        let edge = 450.0 * 0.04;
        let just_out = guide_from_cursor(Vec2::new(800.0 + edge + 1.0, 450.0), WINDOW, 0.04);

        assert!(just_out.x > 0.0 && just_out.x < 0.01, "{just_out:?}");
    }

    /// `miss_tg.flt`, as the level has it: origin at the tail, 4.8 m of
    /// body forward of it along −z, 0.45 m above and below the axis.
    fn guided_missile_bounds() -> Aabb {
        Aabb::from_min_max(Vec3::new(-0.45, -0.45, -4.8), Vec3::new(0.45, 0.45, 0.0))
    }

    /// The model's reach is read in the missile's own space, whichever way
    /// it flies.
    #[test]
    fn the_extent_finds_the_nose_the_tail_and_the_top() {
        let missile = Transform::from_xyz(10.0, 20.0, 30.0).looking_to(Vec3::X, Vec3::Y);
        let global = GlobalTransform::from(missile);

        let extent =
            model_extent(&global, [(&guided_missile_bounds(), &global)]).expect("one part");

        assert!((extent.front + 4.8).abs() < 1e-4, "{extent:?}");
        assert!(extent.tail.abs() < 1e-4, "{extent:?}");
        assert!((extent.top - 0.45).abs() < 1e-4, "{extent:?}");
        assert!((extent.length() - 4.8).abs() < 1e-4, "{extent:?}");
    }

    #[test]
    fn no_meshes_yet_is_no_extent() {
        assert_eq!(model_extent(&GlobalTransform::IDENTITY, []), None);
    }

    /// Over the end and just above: the nose is in the lower half of the
    /// frame, and the tail end, where the exhaust is, is out of view below
    /// the frame.
    #[test]
    fn the_missile_runs_forward_from_the_bottom_with_its_tail_out_of_view() {
        let missile = Transform::from_xyz(10.0, 20.0, 30.0).looking_to(Vec3::X, Vec3::Y);
        let global = GlobalTransform::from(missile);
        let extent = model_extent(&global, [(&guided_missile_bounds(), &global)]);

        let view = guided_viewpoint(&missile, guided_eye(extent, GUIDED_BACK, GUIDED_UP));
        assert!(view.forward().dot(Vec3::X) > 0.999, "looks where it flies");

        let to_view = view.compute_affine().inverse();
        let seen = |corner: Vec3| to_view.transform_point3(missile.transform_point(corner));
        let half_fov = crate::chase_camera::FOV_DEG.to_radians() * 0.5;
        let below = |at: Vec3| (-at.y / -at.z).atan();

        // The nose: ahead, below the centre, inside the frame.
        for corner in [Vec3::new(0.0, 0.45, -4.8), Vec3::new(0.0, -0.45, -4.8)] {
            let at = seen(corner);
            assert!(at.z < 0.0, "the nose is behind the camera: {at:?}");
            assert!(at.y < 0.0, "the nose is above the centre: {at:?}");
            assert!(
                below(at) < half_fov,
                "the nose is {} degrees below the centre, off the frame",
                below(at).to_degrees()
            );
        }

        // The tail end: under or behind the camera, never in the frame.
        for corner in [Vec3::new(0.0, 0.45, 0.0), Vec3::new(0.0, -0.45, 0.0)] {
            let at = seen(corner);
            assert!(
                at.z >= 0.0 || below(at) > half_fov,
                "the tail end is in view: {at:?}"
            );
        }
    }
}
