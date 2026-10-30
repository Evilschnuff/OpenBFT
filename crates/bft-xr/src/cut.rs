//! The table's cutaway: the level drawn as the tank sees it, through
//! `bft-game`'s [`Cutaway`].
//!
//! While the table is up, the tank looks round every few frames, from the
//! height the original's chase camera rides over the hull (`camback`), so
//! it sees what the player is used to seeing: a ray in each of [`AZIMUTHS`]
//! by [`ELEVATIONS`] directions all round, whichever way the hull faces,
//! against the level alone, out to the level's fog distance (as far as the
//! original lets the player see). That [`Sight`] decides what is drawn:
//! what the tank sees is drawn as it is, what stands between the head and
//! the space the tank sees is cut (a tunnel's roof, the wall on the near
//! side, a hill in front of a valley), ground seen before is darkened
//! ([`Explored`]) and the rest darker still, so the side of a crate the
//! tank cannot see is dark rather than a window into it. The tank's own
//! meshes are tagged to keep; other vehicles, pickups and shots to show
//! only while the tank sees them. Only the eyes' views are cut, being the ones scaled up; the
//! window keeps the whole level.
//!
//! The laser goes through what is not drawn and lands on what the head
//! sees ([`aim_ray`]): its ray is moved on past every surface it meets that
//! is cut or faces away, and if a drawn surface hides the point it reaches
//! from the head, it lands on that surface instead.
//!
//! Recoil had no VR, so the sight's resolution and timing are presentation
//! choices.

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy_mod_xr::camera::XrCamera;
use bft_game::cutaway::{Cutaway, Explored, KEEP, Sight, when_seen};
use bft_sim::level::{GameLayer, WorldEnv};
use bft_sim::pickups::Pickup;
use bft_sim::vehicle::{Controlled, Vehicle, VehicleTable};
use bft_sim::weapons::Projectile;

use crate::View;

/// Directions round the horizon the tank looks in.
const AZIMUTHS: u32 = 256;

/// Directions from straight down to straight up.
const ELEVATIONS: u32 = 32;

/// Frames one look round is spread over: the rays are a frame's work
/// otherwise.
const SWEEP_FRAMES: u32 = 6;

/// How far the tank sees where the level gives no fog, metres.
const FALLBACK_REACH: f32 = 600.0;

/// How high over the hull's origin the tank looks from where its vehicle
/// gives no chase camera: the player's `camback[0]` height.
const FALLBACK_EYE_HEIGHT: f32 = 4.0;

/// How far under a ceiling the tank looks from at most, metres, so its
/// rays never start inside the rock of a low tunnel.
const CEILING_CLEARANCE: f32 = 0.5;

/// How many open-space columns round each the open space is grown by when
/// a look round completes, so a tunnel is cut out to its walls.
const WIDEN_COLUMNS: u32 = 1;

/// Directions low enough to count toward the ground seen: from this far
/// below the horizon (as the height of a unit direction)...
const GROUND_LOW: f32 = -0.6;

/// ...to this far above it.
const GROUND_HIGH: f32 = 0.1;

/// The smallest view scale that is cut: well under the table's smallest
/// scale (1:10), well over the window's 1.
const MIN_VIEW_SCALE: f32 = 2.0;

/// How bright ground seen before but not now is drawn.
const EXPLORED_SHADE: f32 = 0.5;

/// How bright what the tank has never seen is drawn.
const UNSEEN_SHADE: f32 = 0.25;

/// How far over a vehicle's origin its middle is, metres: vehicles stand
/// on their origins.
pub(crate) const VEHICLE_LIFT: f32 = 1.5;

/// How far past a wall's edge the middle of a vehicle may be and still be
/// seen, metres: about half a vehicle.
pub(crate) const VEHICLE_MARGIN: f32 = 3.0;

/// Other vehicles: shown while the tank sees their middle.
const VEHICLE_TAG: u32 = when_seen(VEHICLE_LIFT, VEHICLE_MARGIN);

/// Pickups: shown while the tank sees just over their origin, with little
/// slack, so one inside a crate stays hidden until the crate is broken.
const PICKUP_TAG: u32 = when_seen(0.5, 0.5);

/// Shots: shown while the tank sees where they are, with a little slack.
const SHOT_TAG: u32 = when_seen(0.0, 2.0);

/// How many surfaces the laser goes through before it gives up: cut ones,
/// and the backs of ones it has gone into.
const MAX_PASSES: usize = 16;

/// How far short of the surface it lands on the aim's ray starts, metres.
const LEAD_IN: f32 = 0.1;

/// How much nearer than the laser's point a surface seen from the head has
/// to be to stand in front of it, metres.
const IN_FRONT: f32 = 0.5;

/// How far past a cut surface the laser starts again, metres, so it does
/// not meet the same one twice.
const STEP_PAST: f32 = 0.05;

/// Cuts the level open at the table.
pub(crate) struct CutPlugin;

impl Plugin for CutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Sweep>().add_systems(
            Update,
            (
                // After the table has moved this frame (it moves before the
                // hand's ray is taken), before the laser is cast through it.
                (look_round, place_cut)
                    .chain()
                    .after(crate::pointer::HandRaySet)
                    .before(crate::choose_aim),
                tag_meshes,
            ),
        );
    }
}

/// A look round in progress.
#[derive(Resource, Debug, Default)]
struct Sweep {
    /// The sight being filled in, from where the tank was when it began.
    sight: Option<Sight>,
    /// The next direction to cast.
    next: u32,
}

/// The local tank: its entity and where it looks from.
fn local_tank(
    hulls: &Query<(Entity, &Transform, &Vehicle, &Controlled)>,
    table: Option<&VehicleTable>,
) -> Option<(Entity, Vec3)> {
    let (entity, transform, vehicle, _) = hulls
        .iter()
        .find(|(_, _, _, controlled)| **controlled == Controlled::Local)?;
    let height = table
        .and_then(|table| table.kind(&vehicle.kind))
        .map_or(FALLBACK_EYE_HEIGHT, |def| def.camback[0].y);
    Some((entity, transform.translation + Vec3::Y * height))
}

/// `eye`, a look-out over the hull at `hull`, brought down under whatever
/// ceiling is lower.
fn under_ceiling(spatial: &SpatialQuery, hull: Vec3, eye: Vec3) -> Vec3 {
    let height = eye.y - hull.y;
    if height <= 0.0 {
        return eye;
    }
    let filter = SpatialQueryFilter::from_mask(GameLayer::Level);
    match spatial.cast_ray(hull, Dir3::Y, height, true, &filter) {
        Some(hit) => hull + Vec3::Y * (hit.distance - CEILING_CLEARANCE).max(0.0),
        None => eye,
    }
}

/// Cast the next share of the tank's look round, and hand the sight over
/// once it is complete, with the ground it saw marked explored.
#[allow(clippy::too_many_arguments)]
fn look_round(
    view: Res<View>,
    env: Option<Res<WorldEnv>>,
    table: Option<Res<VehicleTable>>,
    spatial: SpatialQuery,
    hulls: Query<(Entity, &Transform, &Vehicle, &Controlled)>,
    mut sweep: ResMut<Sweep>,
    mut cutaway: ResMut<Cutaway>,
) {
    // A new level forgets the old one's ground.
    if let Some(env) = env.as_ref().filter(|env| env.is_changed()) {
        let (left, top, right, bottom) = env.area;
        let (left, top, right, bottom) = (left as f32, top as f32, right as f32, bottom as f32);
        cutaway.explored = Explored::covering(
            Vec2::new(left.min(right), top.min(bottom)),
            Vec2::new(left.max(right), top.max(bottom)),
        );
        cutaway.sight = Sight::default();
        *sweep = Sweep::default();
    }

    let tank = local_tank(&hulls, table.as_deref());
    let (Some((entity, eye)), View::Table) = (tank, *view) else {
        *sweep = Sweep::default();
        return;
    };
    let hull = hulls
        .get(entity)
        .map_or(eye, |(_, transform, ..)| transform.translation);
    let reach = env.as_ref().map_or(FALLBACK_REACH, |env| env.fog_range.1);
    let sweep = &mut *sweep;
    let sight = sweep.sight.get_or_insert_with(|| {
        let origin = under_ceiling(&spatial, hull, eye);
        Sight::open(origin, reach, AZIMUTHS, ELEVATIONS)
    });

    let total = sight.azimuths * sight.elevations;
    let end = (sweep.next + total.div_ceil(SWEEP_FRAMES)).min(total);
    let filter = SpatialQueryFilter::from_mask(GameLayer::Level);
    for index in sweep.next..end {
        let (column, row) = (index % sight.azimuths, index / sight.azimuths);
        let Ok(direction) = Dir3::new(sight.direction(column, row)) else {
            continue;
        };
        let depth = spatial
            .cast_ray(sight.origin, direction, sight.reach, true, &filter)
            .map_or(sight.reach, |hit| hit.distance);
        sight.record(column, row, depth);
    }
    sweep.next = end;
    if end < total {
        return;
    }

    let mut sight = sweep.sight.take().unwrap_or_default();
    sweep.next = 0;
    sight.columns.widen(WIDEN_COLUMNS);
    mark_seen_ground(&sight, &mut cutaway.explored);
    cutaway.sight = sight;
}

/// Mark the ground the sight reaches across as explored: in each column,
/// as far out as its low rays reach before meeting something.
fn mark_seen_ground(sight: &Sight, explored: &mut Explored) {
    if sight.is_empty() || explored.cell <= 0.0 {
        return;
    }
    let reaches: Vec<f32> = (0..sight.azimuths)
        .map(|column| {
            (0..sight.elevations)
                .filter_map(|row| {
                    let direction = sight.direction(column, row);
                    (GROUND_LOW..=GROUND_HIGH).contains(&direction.y).then(|| {
                        sight.depths[(row * sight.azimuths + column) as usize]
                            * direction.xz().length()
                    })
                })
                .fold(0.0, f32::max)
        })
        .collect();
    let centre = sight.origin.xz();
    let furthest = reaches.iter().copied().fold(0.0, f32::max);
    let cell = explored.cell;
    let steps = (furthest / cell).ceil() as i32;
    for z in -steps..=steps {
        for x in -steps..=steps {
            let point = centre + Vec2::new(x as f32, z as f32) * cell;
            let offset = point - centre;
            let column = ((offset.y.atan2(offset.x) + std::f32::consts::PI) / std::f32::consts::TAU
                * sight.azimuths as f32)
                .floor() as usize
                % sight.azimuths as usize;
            if offset.length() <= reaches[column] {
                explored.mark(point);
            }
        }
    }
}

/// Aim the cutaway from the head, while the table is up; take it away at
/// the seat.
#[allow(clippy::too_many_arguments)]
fn place_cut(
    view: Res<View>,
    eyes: Query<&GlobalTransform, With<XrCamera>>,
    table: Option<Res<VehicleTable>>,
    hulls: Query<(Entity, &Transform, &Vehicle, &Controlled)>,
    mut cutaway: ResMut<Cutaway>,
) {
    let tank = local_tank(&hulls, table.as_deref());
    let head = crate::head(&eyes);
    let enabled = *view == View::Table && tank.is_some() && head.is_some();
    if cutaway.enabled != enabled {
        cutaway.enabled = enabled;
    }
    if let Some(head) = head.filter(|_| enabled) {
        cutaway.min_view_scale = MIN_VIEW_SCALE;
        cutaway.explored_shade = EXPLORED_SHADE;
        cutaway.unseen_shade = UNSEEN_SHADE;
        cutaway.eye = head.translation;
    }
}

/// Tag the local tank's meshes to keep, and other vehicles', pickups' and
/// shots' to show only while seen, as their parts arrive.
#[allow(clippy::type_complexity)]
fn tag_meshes(
    mut commands: Commands,
    vehicles: Query<(Entity, &Controlled), With<Vehicle>>,
    pickups: Query<Entity, With<Pickup>>,
    shots: Query<Entity, With<Projectile>>,
    children: Query<&Children>,
    meshes: Query<Option<&MeshTag>, With<Mesh3d>>,
) {
    let roots = vehicles
        .iter()
        .map(|(entity, controlled)| {
            let tag = if *controlled == Controlled::Local {
                KEEP
            } else {
                VEHICLE_TAG
            };
            (entity, tag)
        })
        .chain(pickups.iter().map(|entity| (entity, PICKUP_TAG)))
        .chain(shots.iter().map(|entity| (entity, SHOT_TAG)));
    for (root, wanted) in roots {
        for entity in std::iter::once(root).chain(children.iter_descendants(root)) {
            if let Ok(tag) = meshes.get(entity)
                && tag.is_none_or(|tag| tag.0 != wanted)
            {
                commands.entity(entity).insert(MeshTag(wanted));
            }
        }
    }
}

/// The first surface along `ray` that is drawn and faces it: where it
/// starts and the point it meets, past the surfaces the cutaway does not
/// draw and the backs of the ones it goes into. `exclude` is the shooter's
/// own hull, which the game's aim also looks through.
///
/// Not solid: a ray started just inside a cut rock goes on to its far side
/// rather than meeting it at once, and that far side, facing away, is not
/// drawn either.
fn first_drawn(
    spatial: &SpatialQuery,
    cutaway: &Cutaway,
    ray: Ray3d,
    exclude: Option<Entity>,
    range: f32,
) -> (Vec3, Option<Vec3>) {
    let filter = SpatialQueryFilter::default().with_excluded_entities(exclude);
    let mut origin = ray.origin;
    for _ in 0..MAX_PASSES {
        let left = range - origin.distance(ray.origin);
        let Some(hit) = spatial.cast_ray(origin, ray.direction, left, false, &filter) else {
            return (origin, None);
        };
        let point = origin + ray.direction * hit.distance;
        let facing = hit.normal.dot(*ray.direction) < 0.0;
        if facing && !cutaway.cuts(point, hit.normal) {
            return (origin, Some(point));
        }
        origin = point + ray.direction * STEP_PAST;
    }
    (origin, None)
}

/// The ray the turret aims along, for the laser along `ray`: to the first
/// surface along it that the cutaway draws, and if a drawn surface stands
/// between the head and that point, to that surface instead, so the beam
/// ends on what the head sees where it ends. `exclude` is the shooter's own
/// hull, which the game's aim also looks through.
pub(crate) fn aim_ray(
    spatial: &SpatialQuery,
    cutaway: &Cutaway,
    ray: Ray3d,
    exclude: Option<Entity>,
    range: f32,
) -> Ray3d {
    if !cutaway.enabled {
        return ray;
    }
    let (origin, point) = first_drawn(spatial, cutaway, ray, exclude, range);
    let Some(point) = point else {
        return Ray3d::new(origin, ray.direction);
    };
    // Just short of the point: the game's cast is solid, and from inside a
    // cut rock it would stop where it started.
    let laser = Ray3d::new(point - ray.direction * LEAD_IN, ray.direction);
    let Ok(look) = Dir3::new(point - cutaway.eye) else {
        return laser;
    };
    let distance = cutaway.eye.distance(point);
    let (_, seen) = first_drawn(
        spatial,
        cutaway,
        Ray3d::new(cutaway.eye, look),
        exclude,
        distance,
    );
    match seen {
        Some(seen) if cutaway.eye.distance(seen) < distance - IN_FRONT => {
            Ray3d::new(seen - look * LEAD_IN, look)
        }
        _ => laser,
    }
}
