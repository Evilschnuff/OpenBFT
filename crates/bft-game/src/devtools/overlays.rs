//! The gizmo overlays [`DebugFlags`] switches, the selected node's
//! highlight, and Ctrl+click selection.
//!
//! Colliders and water are drawn by [`physics_debug`](crate::physics_debug)
//! and the vehicle by [`vehicle_debug`](crate::vehicle_debug); the
//! rest are here. Every system has a run condition on its flag, so an
//! overlay that is off is not scheduled at all.
//!
//! Gizmos cannot draw text, so the AI and anim labels are painted into the
//! egui background layer at the projected point instead.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::input::EguiWantsInput;
use bevy_egui::{EguiContext, EguiPrimaryContextPass, PrimaryEguiContext};
use bft_assets::axes::AxisConvention;
use bft_assets::reader::ReaderData;
use bft_sim::ai::{AiBrain, AiState, TurretAi};
use bft_sim::anim::{AnimInstance, AnimProgram};
use bft_sim::level::{NodeName, PartitionGrid};
use bft_sim::vehicle::SpawnPoint;

use super::{DebugFlags, SelectedNode, egui};
use crate::level_view::LoadedLevel;
use crate::mesh_builder::ModelPart;
use crate::viewer::CELL_SIZE;

/// How far from the camera a box or label is still drawn, as the collider
/// overlay does (`physics_debug::DRAW_RANGE`).
const DRAW_RANGE: f32 = crate::physics_debug::DRAW_RANGE;

/// How far a Ctrl+click looks for something to select.
const PICK_RANGE: f32 = 4000.0;

const SELECTED_COLOR: Color = Color::srgb(1.0, 0.9, 0.1);
const BBOX_COLOR: Color = Color::srgb(0.6, 0.4, 1.0);
const PARTITION_COLOR: Color = Color::srgba(0.9, 0.9, 0.9, 0.5);
const EMPTY_PARTITION_COLOR: Color = Color::srgba(0.9, 0.9, 0.9, 0.12);
const PLAYER_SPAWN_COLOR: Color = Color::srgb(0.2, 1.0, 0.3);
const ENEMY_SPAWN_COLOR: Color = Color::srgb(1.0, 0.3, 0.3);
const AI_COLOR: Color = Color::srgb(1.0, 0.5, 0.1);
const ANIM_COLOR: Color = Color::srgb(0.3, 0.9, 1.0);

/// The overlays' systems.
pub(super) struct OverlaysPlugin;

impl Plugin for OverlaysPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                draw_selected.run_if(|selected: Res<SelectedNode>| selected.0.is_some()),
                draw_node_bboxes.run_if(|flags: Res<DebugFlags>| flags.node_bboxes),
                draw_partitions.run_if(|flags: Res<DebugFlags>| flags.partitions),
                draw_spawns.run_if(|flags: Res<DebugFlags>| flags.spawns),
                draw_ai.run_if(|flags: Res<DebugFlags>| flags.ai),
                draw_anim.run_if(|flags: Res<DebugFlags>| flags.anim),
                pick_node,
            ),
        )
        .add_systems(
            EguiPrimaryContextPass,
            draw_labels.run_if(|flags: Res<DebugFlags>| flags.ai || flags.anim),
        );
    }
}

/// A mesh part's bounds as an oriented box in world space.
fn part_box(aabb: &Aabb, global: &GlobalTransform) -> Transform {
    let local = Transform::from_translation(aabb.center.into())
        .with_scale(Vec3::from(aabb.half_extents) * 2.0);
    global.mul_transform(local).compute_transform()
}

/// The eye of whichever 3D camera is drawing: the viewer's or the chase one.
fn eye(cameras: &Query<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>) -> Option<Vec3> {
    cameras.iter().next().map(GlobalTransform::translation)
}

/// Outline the selected node's meshes and mark its origin.
fn draw_selected(
    mut gizmos: Gizmos,
    selected: Res<SelectedNode>,
    transforms: Query<&GlobalTransform>,
    children: Query<&Children>,
    parts: Query<(&Aabb, &GlobalTransform), With<ModelPart>>,
) {
    let Some(entity) = selected.0 else {
        return;
    };
    let Ok(origin) = transforms.get(entity) else {
        return;
    };

    gizmos.axes(origin.compute_transform(), 4.0);
    gizmos.sphere(
        Isometry3d::from_translation(origin.translation()),
        0.75,
        SELECTED_COLOR,
    );

    for descendant in std::iter::once(entity).chain(children.iter_descendants(entity)) {
        if let Ok((aabb, global)) = parts.get(descendant) {
            gizmos.cube(part_box(aabb, global), SELECTED_COLOR);
        }
    }
}

/// Outline every drawn mesh part near the camera.
///
/// Bevy's own `Aabb`, the bounds the renderer culls with, rather than the
/// data's `node_bbox`: that one is in the node's local space and includes the
/// hidden prototypes (see `viewer::level_bounds`), so it answers a different
/// question than "what is drawn here".
fn draw_node_bboxes(
    mut gizmos: Gizmos,
    cameras: Query<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>,
    parts: Query<(&Aabb, &GlobalTransform, &InheritedVisibility), With<ModelPart>>,
) {
    let Some(eye) = eye(&cameras) else {
        return;
    };

    for (aabb, global, visibility) in &parts {
        if !visibility.get() {
            continue;
        }
        let transform = part_box(aabb, global);
        if transform.translation.distance(eye) > DRAW_RANGE {
            continue;
        }
        gizmos.cube(transform, BBOX_COLOR);
    }
}

/// The partition grid: one column per 256 m cell, spanning its height band.
fn draw_partitions(
    mut gizmos: Gizmos,
    grid: Option<Res<PartitionGrid>>,
    axes: Option<Res<AxisConvention>>,
) {
    let Some(grid) = grid else {
        return;
    };
    let axes = axes.map(|axes| *axes).unwrap_or_default();

    for cell in grid.cells() {
        let min = axes.point(Vec3::new(cell.x as f32, cell.y_min, cell.z as f32));
        let max = axes.point(Vec3::new(
            cell.x as f32 + CELL_SIZE,
            cell.y_max,
            cell.z as f32 + CELL_SIZE,
        ));
        let (lo, hi) = (min.min(max), min.max(max));
        // A flat cell still gets a visible slab.
        let size = (hi - lo).max(Vec3::splat(0.5));

        gizmos.cube(
            Transform::from_translation((lo + hi) * 0.5).with_scale(size),
            if cell.nodes.is_empty() {
                EMPTY_PARTITION_COLOR
            } else {
                PARTITION_COLOR
            },
        );
    }
}

/// A ring and a heading arrow at a start.
fn spawn_marker(gizmos: &mut Gizmos, position: Vec3, facing: Vec3, color: Color) {
    gizmos.circle(
        Isometry3d::new(position, Quat::from_rotation_arc(Vec3::Z, Vec3::Y)),
        3.0,
        color,
    );
    gizmos.arrow(position, position + facing.normalize_or_zero() * 8.0, color);
    gizmos.line(position, position + Vec3::Y * 12.0, color);
}

/// The player start, the enemies' homes, and any placed spawn point.
///
/// Nothing spawns [`SpawnPoint`] entities yet (a deathmatch spawn ring
/// would), so the player start is read from the level's `aiv.zrd`, the same
/// record the viewer's camera starts from.
fn draw_spawns(
    mut gizmos: Gizmos,
    loaded: Option<Res<LoadedLevel>>,
    readers: Res<Assets<ReaderData>>,
    points: Query<(&SpawnPoint, &GlobalTransform)>,
    brains: Query<&AiBrain>,
) {
    if let Some(start) = loaded
        .as_ref()
        .and_then(|loaded| loaded.reader.as_ref())
        .and_then(|handle| readers.get(handle))
        .and_then(ReaderData::player_start)
    {
        spawn_marker(
            &mut gizmos,
            start.position,
            start.facing(),
            PLAYER_SPAWN_COLOR,
        );
    }

    for (point, global) in &points {
        let facing = Quat::from_rotation_y(point.heading) * Vec3::NEG_Z;
        spawn_marker(
            &mut gizmos,
            global.translation(),
            facing,
            PLAYER_SPAWN_COLOR,
        );
    }

    for brain in &brains {
        gizmos.circle(
            Isometry3d::new(brain.home, Quat::from_rotation_arc(Vec3::Z, Vec3::Y)),
            2.0,
            ENEMY_SPAWN_COLOR,
        );
    }
}

/// A line from each enemy and turret to its target.
fn draw_ai(
    mut gizmos: Gizmos,
    brains: Query<(&AiBrain, &GlobalTransform)>,
    turrets: Query<(&TurretAi, &GlobalTransform)>,
    transforms: Query<&GlobalTransform>,
) {
    let targets = brains
        .iter()
        .filter(|(brain, _)| brain.state != AiState::Dead)
        .map(|(brain, global)| (brain.target, global))
        .chain(
            turrets
                .iter()
                .filter(|(turret, _)| !turret.deactivated)
                .map(|(turret, global)| (turret.target, global)),
        );

    for (target, global) in targets {
        let from = global.translation();
        gizmos.sphere(Isometry3d::from_translation(from), 1.0, AI_COLOR);

        if let Some(to) = target.and_then(|target| transforms.get(target).ok()) {
            gizmos.line(from, to.translation(), AI_COLOR);
        }
    }
}

/// A marker at the root of every running anim instance.
fn draw_anim(
    mut gizmos: Gizmos,
    instances: Query<&AnimInstance>,
    transforms: Query<&GlobalTransform>,
) {
    for instance in &instances {
        if instance.running.is_empty() {
            continue;
        }
        let Some(root) = instance.root.and_then(|root| transforms.get(root).ok()) else {
            continue;
        };
        gizmos.sphere(
            Isometry3d::from_translation(root.translation()),
            1.5,
            ANIM_COLOR,
        );
    }
}

/// The AI states and running anim names, as text next to their markers.
fn draw_labels(world: &mut World) {
    let flags = *world.resource::<DebugFlags>();

    let mut contexts = world.query_filtered::<&mut EguiContext, With<PrimaryEguiContext>>();
    let Ok(mut context) = contexts.single_mut(world) else {
        return;
    };
    let ctx = context.get_mut().clone();

    let mut cameras = world.query::<(&Camera, &GlobalTransform)>();
    let Some((camera, camera_transform)) = cameras
        .iter(world)
        .find(|(camera, _)| camera.is_active && camera.order == 0)
        .map(|(camera, transform)| (camera.clone(), *transform))
    else {
        return;
    };
    let eye = camera_transform.translation();

    let mut labels: Vec<(Vec3, String, Color)> = Vec::new();

    if flags.ai {
        let mut brains = world.query::<(&AiBrain, &GlobalTransform)>();
        for (brain, global) in brains.iter(world) {
            labels.push((
                global.translation() + Vec3::Y * 3.0,
                format!("{:?}", brain.state),
                AI_COLOR,
            ));
        }
    }

    if flags.anim
        && let Some(program) = world.get_resource::<AnimProgram>()
    {
        let program = program.clone();
        let mut instances = world.query::<&AnimInstance>();
        let running: Vec<(usize, Entity)> = instances
            .iter(world)
            .filter(|instance| !instance.running.is_empty())
            .filter_map(|instance| Some((instance.def, instance.root?)))
            .collect();
        for (def, root) in running {
            let (Some(def), Some(global)) =
                (program.0.defs.get(def), world.get::<GlobalTransform>(root))
            else {
                continue;
            };
            labels.push((
                global.translation() + Vec3::Y * 2.0,
                def.anim_name.clone(),
                ANIM_COLOR,
            ));
        }
    }

    let painter = ctx.layer_painter(egui::LayerId::background());
    for (position, text, color) in labels {
        if position.distance(eye) > DRAW_RANGE {
            continue;
        }
        let Ok(screen) = camera.world_to_viewport(&camera_transform, position) else {
            continue;
        };
        let [r, g, b, _] = color.to_srgba().to_u8_array();
        painter.text(
            egui::pos2(screen.x, screen.y),
            egui::Align2::CENTER_BOTTOM,
            text,
            egui::FontId::monospace(12.0),
            egui::Color32::from_rgb(r, g, b),
        );
    }
}

/// Ctrl+click selects the node under the cursor.
///
/// The ray is tested against the drawn parts' bounding boxes, not their
/// triangles or colliders: meshes are uploaded render-world-only, so there
/// are no triangles on the CPU to test, and avian's spatial query is only
/// kept current while the sim steps, which it does not in the viewer. Boxes
/// overlap — a terrain tile's box covers what stands on it — so clicking
/// again at the same spot steps to the next box along the ray.
#[allow(clippy::too_many_arguments)]
fn pick_node(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    egui_input: Option<Res<EguiWantsInput>>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    cameras: Query<(&Camera, &GlobalTransform), crate::ui::hud::sensor::MainCamera3d>,
    parts: Query<(Entity, &Aabb, &GlobalTransform, &InheritedVisibility), With<ModelPart>>,
    parents: Query<&ChildOf>,
    names: Query<(), With<NodeName>>,
    mut selected: ResMut<SelectedNode>,
    mut last: Local<Option<(Vec2, usize)>>,
) {
    if !mouse.just_pressed(MouseButton::Left)
        || !(keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
    {
        return;
    }
    if egui_input.is_some_and(|input| input.wants_pointer_input()) {
        return;
    }
    let Some(cursor) = window.and_then(|window| window.cursor_position()) else {
        return;
    };
    let Some((camera, transform)) = cameras.iter().find(|(camera, _)| camera.is_active) else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
        return;
    };

    let mut hits: Vec<(f32, Entity)> = parts
        .iter()
        .filter(|(.., visibility)| visibility.get())
        .filter_map(|(entity, aabb, global, _)| {
            let (entry, _) = ray_box(ray.origin, *ray.direction, &part_box(aabb, global))?;
            // A box the camera is inside is the ground it flies over, not
            // what the click is aimed at.
            (entry > 0.0 && entry < PICK_RANGE).then_some((entry, entity))
        })
        .collect();
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));

    // The same spot again: the next hit along the ray.
    let index = match *last {
        Some((at, index)) if at.distance(cursor) < 3.0 && !hits.is_empty() => {
            (index + 1) % hits.len()
        }
        _ => 0,
    };
    *last = Some((cursor, index));

    let Some(&(_, part)) = hits.get(index) else {
        return;
    };

    // A part hangs off the node that draws it: select that node.
    let mut current = part;
    loop {
        if names.contains(current) {
            selected.0 = Some(current);
            return;
        }
        match parents.get(current) {
            Ok(child_of) => current = child_of.parent(),
            Err(_) => return,
        }
    }
}

/// Where a ray enters and leaves a box, as distances along it.
///
/// The box is a unit cube under `transform`, as [`part_box`] makes it. The
/// ray is taken into the box's space, where the test is the plain slab one;
/// an affine map keeps the ray parameter, so the distances are world ones.
fn ray_box(origin: Vec3, direction: Vec3, transform: &Transform) -> Option<(f32, f32)> {
    let inverse = transform.compute_affine().inverse();
    let origin = inverse.transform_point3(origin);
    let direction = inverse.transform_vector3(direction);

    let mut entry = f32::NEG_INFINITY;
    let mut exit = f32::INFINITY;

    for axis in 0..3 {
        let (o, d) = (origin[axis], direction[axis]);
        if d.abs() < 1e-9 {
            if !(-0.5..=0.5).contains(&o) {
                return None;
            }
            continue;
        }
        let (a, b) = ((-0.5 - o) / d, (0.5 - o) / d);
        entry = entry.max(a.min(b));
        exit = exit.min(a.max(b));
    }

    (entry <= exit && exit >= 0.0).then_some((entry, exit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_enters_and_leaves_a_box_at_its_faces() {
        let cube =
            Transform::from_translation(Vec3::new(0.0, 0.0, -10.0)).with_scale(Vec3::splat(2.0));
        let (entry, exit) = ray_box(Vec3::ZERO, Vec3::NEG_Z, &cube).expect("hits");

        assert!((entry - 9.0).abs() < 1e-5);
        assert!((exit - 11.0).abs() < 1e-5);
    }

    #[test]
    fn a_ray_past_a_box_misses_it() {
        let cube = Transform::from_translation(Vec3::new(5.0, 0.0, -10.0));

        assert!(ray_box(Vec3::ZERO, Vec3::NEG_Z, &cube).is_none());
    }

    #[test]
    fn a_box_behind_the_ray_is_missed() {
        let cube = Transform::from_translation(Vec3::new(0.0, 0.0, 10.0));

        assert!(ray_box(Vec3::ZERO, Vec3::NEG_Z, &cube).is_none());
    }

    #[test]
    fn a_rotated_box_is_tested_in_its_own_space() {
        // A long thin box turned 90 degrees: along x it is 10 wide, not 1.
        let plank = Transform::from_translation(Vec3::new(4.0, 0.0, -10.0))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::new(1.0, 1.0, 10.0));

        assert!(ray_box(Vec3::ZERO, Vec3::NEG_Z, &plank).is_some());
    }

    /// The camera inside a box starts the ray inside it: the entry is behind.
    #[test]
    fn a_ray_from_inside_enters_behind_its_origin() {
        let ground = Transform::from_scale(Vec3::splat(100.0));
        let (entry, exit) = ray_box(Vec3::ZERO, Vec3::NEG_Z, &ground).expect("hits");

        assert!(entry < 0.0 && exit > 0.0);
    }
}
