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
//! Level parts no more than [`SPARE_SIZE`] across (posts, pillars, crates,
//! sheds) are tagged [`SPARE`]: darkened where the tank does not see them
//! but never cut, since the player leans to look past something that
//! small. Where a cut opens onto nothing at all, the level's fog colour
//! shows ([`table`](crate::table)), and the edge of every cut thins out
//! toward that colour ([`RIM_SHADE`]).
//!
//! A press of X on the Steam Frame's own controllers ([`SeeOnly`]) switches
//! to drawing only what the tank sees, everything else cut.
//!
//! The laser goes through what is not drawn and lands on what the head
//! sees ([`aim_ray`]): its ray is moved on past every surface it meets that
//! is cut or faces away, and if a drawn surface hides the point it reaches
//! from the head, it lands on that surface instead.
//!
//! Recoil had no VR, so the sight's resolution and timing are presentation
//! choices.

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::camera::primitives::Aabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy_mod_openxr::action_binding::{OxrSendActionBindings, OxrSuggestActionBinding};
use bevy_mod_openxr::action_set_attaching::OxrAttachActionSet;
use bevy_mod_openxr::action_set_syncing::{OxrActionSetSyncSet, OxrSyncActionSet};
use bevy_mod_openxr::openxr_session_running;
use bevy_mod_openxr::resources::OxrInstance;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_xr::camera::XrCamera;
use bevy_mod_xr::session::{XrSessionCreated, session_available};
use bft_game::cutaway::{Cutaway, Explored, KEEP, SPARE, Sight, when_seen};
use bft_game::mesh_builder::ModelPart;
use bft_sim::level::{GameLayer, WorldEnv};
use bft_sim::pickups::Pickup;
use bft_sim::vehicle::{Controlled, Vehicle, VehicleTable};
use bft_sim::weapons::Projectile;

use crate::View;
use crate::profiles::{FRAME, FrameControllers};

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

/// How far round its origin the sight may lean to see past what is in the
/// way, metres of the level ([`Sight::lean`]): as a player shifts their
/// head to look past a post. Posts and pillars a few metres across then
/// hide nothing beyond the sight's own slack, and a wall still hides what
/// is behind it.
const SIGHT_LEAN: f32 = 6.0;

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

/// How bright the edge of a cut is tinted, as a share of the level's fog
/// colour.
const RIM_SHADE: f32 = 1.0;

/// How far across a level part may be and still be spared cutting, metres
/// of the level, its larger horizontal extent: a pillar, a post, a crate, a
/// shed, but not a hill or a building's wall.
const SPARE_SIZE: f32 = 6.0;

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
        app.init_resource::<Sweep>()
            .init_resource::<SeeOnly>()
            .add_systems(Startup, create_actions.run_if(session_available))
            .add_systems(OxrSendActionBindings, suggest_bindings)
            .add_systems(XrSessionCreated, attach_actions)
            .add_systems(
                PreUpdate,
                (
                    sync_actions
                        .before(OxrActionSetSyncSet)
                        .run_if(openxr_session_running),
                    switch_mode
                        .after(OxrActionSetSyncSet)
                        .run_if(openxr_session_running),
                ),
            )
            .add_systems(
                Update,
                (
                    // After the table has moved this frame (it moves before
                    // the hand's ray is taken), before the laser is cast
                    // through it.
                    (look_round, place_cut)
                        .chain()
                        .after(crate::pointer::HandRaySet)
                        .before(crate::choose_aim),
                    tag_meshes,
                    spare_small_parts,
                ),
            );
    }
}

/// Whether to draw only what the tank sees ([`Cutaway::only_seen`]).
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SeeOnly(pub(crate) bool);

/// The switch between the cutaway and seeing only what the tank sees.
#[derive(Resource)]
struct ModeActions {
    set: openxr::ActionSet,
    switch: openxr::Action<bool>,
}

/// Make the action set and the switch.
fn create_actions(instance: Res<OxrInstance>, mut commands: Commands) {
    let created = instance
        .create_action_set("cutaway", "Cutaway", 0)
        .and_then(|set| {
            Ok(ModeActions {
                switch: set.create_action(
                    "see_only_what_the_tank_sees",
                    "Show only what the tank sees",
                    &[],
                )?,
                set,
            })
        });
    match created {
        Ok(actions) => commands.insert_resource(actions),
        Err(error) => warn!("cutaway: could not create the switch: {error}"),
    }
}

/// X on the Frame's own controllers; the Touch layout has no button to
/// spare, so there it is for the player to bind.
fn suggest_bindings(
    actions: Option<Res<ModeActions>>,
    frame: Res<FrameControllers>,
    mut bindings: MessageWriter<OxrSuggestActionBinding>,
) {
    if let Some(actions) = actions.filter(|_| frame.0) {
        bindings.write(OxrSuggestActionBinding {
            action: actions.switch.as_raw(),
            interaction_profile: FRAME.into(),
            bindings: vec!["/user/hand/right/input/x/click".into()],
        });
    }
}

/// Attach the set to the new session.
fn attach_actions(
    actions: Option<Res<ModeActions>>,
    mut attach: MessageWriter<OxrAttachActionSet>,
) {
    if let Some(actions) = actions {
        attach.write(OxrAttachActionSet(actions.set.clone()));
    }
}

/// Ask the runtime for this frame's switch.
fn sync_actions(actions: Option<Res<ModeActions>>, mut sync: MessageWriter<OxrSyncActionSet>) {
    if let Some(actions) = actions {
        sync.write(OxrSyncActionSet(actions.set.clone()));
    }
}

/// Flip [`SeeOnly`] on each press, and say so.
fn switch_mode(
    actions: Option<Res<ModeActions>>,
    session: Res<OxrSession>,
    mut see_only: ResMut<SeeOnly>,
) {
    let pressed = actions.is_some_and(|actions| {
        actions
            .switch
            .state(&session, openxr::Path::NULL)
            .is_ok_and(|state| {
                state.is_active && state.changed_since_last_sync && state.current_state
            })
    });
    if pressed {
        see_only.0 = !see_only.0;
        info!(
            "cutaway: {}",
            if see_only.0 {
                "showing only what the tank sees"
            } else {
                "cutting away what hides the tank's view"
            }
        );
    }
}

/// Tag each level part once its bounds are known: [`SPARE`] when it is no
/// more than [`SPARE_SIZE`] across, untagged otherwise. Vehicles', pickups'
/// and shots' parts are retagged by [`tag_meshes`].
#[allow(clippy::type_complexity)]
fn spare_small_parts(
    mut commands: Commands,
    parts: Query<(Entity, &Aabb, &GlobalTransform), (With<ModelPart>, Without<MeshTag>)>,
) {
    for (entity, bounds, transform) in &parts {
        let tag = if horizontal_size(bounds, transform) <= SPARE_SIZE {
            SPARE
        } else {
            0
        };
        commands.entity(entity).insert(MeshTag(tag));
    }
}

/// How far across the ground a part reaches in the world, its larger
/// horizontal extent, from its own bounds and its transform.
fn horizontal_size(bounds: &Aabb, transform: &GlobalTransform) -> f32 {
    let matrix = transform.affine().matrix3;
    let half = bounds.half_extents;
    // The world box round a turned box: each axis takes the absolute
    // reach of every local axis along it.
    let world = Vec3::new(
        matrix.row(0).abs().dot(half),
        matrix.row(1).abs().dot(half),
        matrix.row(2).abs().dot(half),
    );
    2.0 * world.x.max(world.z)
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
/// once it is complete, with the ground it saw marked explored. The view
/// then fades from the old sight to the new one over the next sweep
/// ([`Cutaway::fade`]), so shadows move rather than jump.
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
        cutaway.previous = None;
        cutaway.fade = 1.0;
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
        Sight {
            lean: SIGHT_LEAN,
            ..Sight::open(origin, reach, AZIMUTHS, ELEVATIONS)
        }
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
    // The view fades from the last sight to this one over the time the next
    // takes to cast, so it has just got there when the next is handed over.
    if cutaway.fade < 1.0 {
        cutaway.fade = (cutaway.fade + 1.0 / SWEEP_FRAMES as f32).min(1.0);
    }
    if end < total {
        return;
    }

    let mut sight = sweep.sight.take().unwrap_or_default();
    sweep.next = 0;
    sight.columns.widen(WIDEN_COLUMNS);
    mark_seen_ground(&sight, &mut cutaway.explored);
    let mut before = std::mem::replace(&mut cutaway.sight, sight);
    // Only its depths are faded over; its open space is not read.
    before.columns = Default::default();
    cutaway.previous = (!before.is_empty()).then_some(before);
    cutaway.fade = 0.0;
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
    see_only: Res<SeeOnly>,
    env: Option<Res<WorldEnv>>,
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
        cutaway.only_seen = see_only.0;
        cutaway.min_view_scale = MIN_VIEW_SCALE;
        cutaway.explored_shade = EXPLORED_SHADE;
        cutaway.unseen_shade = UNSEEN_SHADE;
        cutaway.eye = head.translation;
        // The edge of a cut thins out toward the level's own fog colour, a
        // little brighter than the backdrop behind the table, so it reads
        // as fading into it.
        let fog = env.as_ref().map_or(Color::BLACK, |env| env.fog_color);
        cutaway.rim_colour = (fog.to_linear() * RIM_SHADE).with_alpha(1.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A pillar a metre square and 6 m tall is small, however it is turned
    /// or how tall; a 20 m slab of ground is not.
    #[test]
    fn a_pillar_is_small_and_a_slab_of_ground_is_not() {
        let pillar = Aabb::from_min_max(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 6.0, 0.5));
        let turned = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_y(
            std::f32::consts::FRAC_PI_4,
        )));
        let size = horizontal_size(&pillar, &turned);
        assert!((size - std::f32::consts::SQRT_2).abs() < 1e-5, "{size}");
        assert!(size <= SPARE_SIZE);

        let slab = Aabb::from_min_max(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 1.0, 10.0));
        assert!(horizontal_size(&slab, &GlobalTransform::IDENTITY) > SPARE_SIZE);
        // Scaled up, the pillar is not small either.
        let scaled = GlobalTransform::from(Transform::from_scale(Vec3::splat(10.0)));
        assert!(horizontal_size(&pillar, &scaled) > SPARE_SIZE);
    }
}
