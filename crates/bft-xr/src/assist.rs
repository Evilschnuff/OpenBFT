//! Aim assist: the beam snaps onto an enemy it passes close to.
//!
//! Aiming with a beam is all angle, and a wrist holds about half a degree;
//! zoomed out at the table, half a degree is more than an enemy is wide. So
//! the beam locks onto an enemy within [`SNAP`] of it, judged as the head
//! sees it, the angle between the enemy and the nearest point of the beam,
//! which is the same at any zoom. The lock holds until the beam strays past
//! [`RELEASE`], and moves to another enemy only when that one is nearer the
//! beam by [`SWITCH`], so two enemies close together do not trade the lock
//! back and forth. Only enemies the player can see are taken: at the table,
//! those the cutaway shows; in the seat, those in clear line from the head.
//!
//! While locked, the turret aims at the enemy and the beam bends to it
//! ([`AimLock`]). Recoil had no aim assist, so all of this, the angles
//! included, is a presentation choice for the headset only.

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;
use bevy_mod_xr::camera::XrCamera;
use bft_game::aim::{ExternalAim, aim_point};
use bft_game::cutaway::Cutaway;
use bft_sim::ai::{Enemy, TurretAi};
use bft_sim::damage::Health;
use bft_sim::level::GameLayer;

use crate::cut::{VEHICLE_LIFT, VEHICLE_MARGIN};

/// How near the beam an enemy has to come to lock on, as the head sees it.
const SNAP: f32 = 3.0 * std::f32::consts::PI / 180.0;

/// How far the beam can stray from a locked enemy before the lock lets go.
const RELEASE: f32 = 4.5 * std::f32::consts::PI / 180.0;

/// How much nearer the beam another enemy has to be to take the lock.
const SWITCH: f32 = 1.0 * std::f32::consts::PI / 180.0;

/// How far short of the enemy's middle the aim's ray starts, metres: inside
/// the enemy's own shape, so the game's cast stops on it and nothing in
/// front of it.
const LEAD_IN: f32 = 1.5;

/// How much short of the enemy a line from the head may stop and still see
/// it, metres: the enemy's own near side, or a turret's base.
const LINE_SLACK: f32 = 3.0;

/// The enemy the aim is locked on, and the point aimed at.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub(crate) struct AimLock {
    pub(crate) target: Option<Entity>,
    pub(crate) point: Option<Vec3>,
}

/// The ray the player points along this frame, before the cutaway or the
/// lock bends it; `None` with nothing to point with.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub(crate) struct Pointing(pub(crate) Option<Ray3d>);

/// Snaps the aim onto enemies.
pub(crate) struct AssistPlugin;

impl Plugin for AssistPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AimLock>()
            .init_resource::<Pointing>()
            .add_systems(Update, lock_on.after(crate::choose_aim).before(aim_point));
    }
}

/// Lock onto the enemy the beam passes nearest, as the head sees it, and
/// aim at it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn lock_on(
    pointing: Res<Pointing>,
    cutaway: Res<Cutaway>,
    spatial: SpatialQuery,
    eyes: Query<&GlobalTransform, With<XrCamera>>,
    enemies: Query<(Entity, &GlobalTransform, &Health), Or<(With<Enemy>, With<TurretAi>)>>,
    mut lock: ResMut<AimLock>,
    mut aim: ResMut<ExternalAim>,
) {
    let (Some(ray), Some(head)) = (pointing.0, crate::head(&eyes)) else {
        *lock = AimLock::default();
        return;
    };
    let head = head.translation;
    let level = SpatialQueryFilter::from_mask(GameLayer::Level);
    let visible = |middle: Vec3, origin: Vec3| {
        if cutaway.enabled {
            return cutaway
                .sight
                .sees_body(origin, VEHICLE_LIFT, VEHICLE_MARGIN);
        }
        let Ok(toward) = Dir3::new(middle - head) else {
            return true;
        };
        let distance = head.distance(middle);
        spatial
            .cast_ray(head, toward, distance, true, &level)
            .is_none_or(|hit| hit.distance >= distance - LINE_SLACK)
    };
    let scored: Vec<(Entity, f32, Vec3)> = enemies
        .iter()
        .filter(|(_, _, health)| !health.is_dead())
        .filter_map(|(entity, transform, _)| {
            let origin = transform.translation();
            let middle = origin + Vec3::Y * VEHICLE_LIFT;
            visible(middle, origin).then(|| (entity, separation(ray, head, middle), middle))
        })
        .collect();

    let angles: Vec<(Entity, f32)> = scored
        .iter()
        .map(|(entity, angle, _)| (*entity, *angle))
        .collect();
    let target = choose(lock.target, &angles);
    let point = target.and_then(|target| {
        scored
            .iter()
            .find(|(entity, ..)| *entity == target)
            .map(|(.., middle)| *middle)
    });
    *lock = AimLock { target, point };

    if let Some(point) = point
        && let Ok(toward) = Dir3::new(point - ray.origin)
    {
        aim.0 = Some(Ray3d::new(point - toward * LEAD_IN, toward));
    }
}

/// How far off the beam `target` is as the head at `head` sees it: the
/// angle between the target and the beam's nearest point to it.
fn separation(ray: Ray3d, head: Vec3, target: Vec3) -> f32 {
    let along = (target - ray.origin).dot(*ray.direction).max(0.0);
    let nearest = ray.get_point(along);
    let (to_target, to_beam) = (target - head, nearest - head);
    if to_target.length_squared() <= f32::EPSILON || to_beam.length_squared() <= f32::EPSILON {
        return 0.0;
    }
    to_target.angle_between(to_beam)
}

/// The enemy to lock onto, given the one locked on now and every visible
/// enemy's angle off the beam: the locked one while it is within
/// [`RELEASE`] and no other is nearer by [`SWITCH`]; otherwise the nearest
/// within [`SNAP`].
fn choose(current: Option<Entity>, angles: &[(Entity, f32)]) -> Option<Entity> {
    let nearest = angles
        .iter()
        .copied()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(_, angle)| *angle <= SNAP);
    let held = current
        .and_then(|current| angles.iter().find(|(entity, _)| *entity == current))
        .filter(|(_, angle)| *angle <= RELEASE);
    match (held, nearest) {
        (Some(&(_, held_angle)), Some((other, angle))) if angle + SWITCH < held_angle => {
            Some(other)
        }
        (Some(&(held, _)), _) => Some(held),
        (None, nearest) => nearest.map(|(entity, _)| entity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn degrees(value: f32) -> f32 {
        value.to_radians()
    }

    #[test]
    fn an_enemy_near_the_beam_is_locked_and_held_through_a_wobble() {
        let (a, b) = (
            Entity::from_raw_u32(1).unwrap(),
            Entity::from_raw_u32(2).unwrap(),
        );
        assert_eq!(
            choose(None, &[(a, degrees(2.5)), (b, degrees(8.0))]),
            Some(a)
        );
        assert_eq!(choose(None, &[(a, degrees(3.5))]), None);
        // Held past the snap angle, let go past the release.
        assert_eq!(choose(Some(a), &[(a, degrees(4.0))]), Some(a));
        assert_eq!(choose(Some(a), &[(a, degrees(5.0))]), None);
    }

    #[test]
    fn a_close_neighbour_takes_the_lock_only_when_clearly_nearer() {
        let (a, b) = (
            Entity::from_raw_u32(1).unwrap(),
            Entity::from_raw_u32(2).unwrap(),
        );
        assert_eq!(
            choose(Some(a), &[(a, degrees(1.5)), (b, degrees(1.0))]),
            Some(a)
        );
        assert_eq!(
            choose(Some(a), &[(a, degrees(2.5)), (b, degrees(1.0))]),
            Some(b)
        );
        // A locked enemy no longer seen lets the lock go to the nearest.
        assert_eq!(choose(Some(a), &[(b, degrees(2.0))]), Some(b));
    }

    #[test]
    fn the_separation_is_the_angle_the_head_sees() {
        let head = Vec3::new(0.0, 1.0, 0.0);
        let beam = Ray3d::new(Vec3::new(0.2, 0.0, 0.0), Dir3::NEG_Z);
        // On the beam: no separation.
        assert!(separation(beam, head, Vec3::new(0.2, 0.0, -50.0)) < 1e-4);
        // A metre beside the beam 100 m out: about half a degree from the
        // head, however close the hand is.
        let beside = separation(beam, head, Vec3::new(1.2, 0.0, -100.0));
        assert!(
            (beside.to_degrees() - 0.57).abs() < 0.05,
            "{}",
            beside.to_degrees()
        );
    }
}
