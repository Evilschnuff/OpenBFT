//! The HUD in VR: the original's clusters as separate tiles around the
//! seat, instead of one screen-shaped panel.
//!
//! # How
//!
//! The HUD keeps drawing into the panel's image exactly as it would into the
//! window, and nothing in `bft-game` changes; what changes is how much of
//! that image each quad shows. The original anchors every HUD element to a
//! third of the screen's width and a half of its height
//! (`bft_game::ui::hud::layout`), and its strips are cut at those same
//! seams, so the image falls apart into six regions that each hold one
//! cluster. Every frame the rectangles of the HUD's visible images and text
//! are clipped to those regions, and each region's bounding box becomes a
//! tile: a quad that samples that box of the image and hangs at the
//! region's place around the seat, turned to face the head. Nothing here
//! knows what the art is, so it follows the install's layout, the fallback
//! HUD and the HUD scale setting alike.
//!
//! # On the hands
//!
//! In play every cluster rides on a controller ([`HAND_MOUNTS`]): the
//! monitor (sensor view and message tab) on the left hand with the top
//! strip's left and middle over it, the health gauge and forms on the right
//! with the weapon strip and the top strip's right over them, since the
//! right hand holds the weapon, form and weak/strong controls. They are
//! pinned to the controller lying on its top, face up and their own tops
//! toward its front ([`HAND_TILT`]), so they read the right way up to a
//! glance down at the hands, and turn as the wrist does; smaller than at
//! the seat, since they are read from nearer. A hand's tiles fall back to
//! their places ahead of the seat while its controller is not tracked.
//!
//! Anything over the HUD (the pause menu, a result screen, the front end, a
//! cinematic) and the large automap need the whole picture, so then the
//! single panel comes back instead.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy_mod_openxr::spaces::OxrSpaceLocationFlags;
use bevy_mod_xr::session::XrTrackingRoot;
use bft_game::ui::GameplayGate;
use bft_game::ui::automap::AutomapLarge;
use bft_game::ui::hud::layout::{Anchor, Column, Row};
use bft_game::ui::hud::{HudMap, HudRoot};

use crate::hands::{Grip, Side, located_grip};
use crate::{PANEL_LAYER, Panel, PanelMaterial, spawn_panel};

/// How far in front of the head the tiles hang, metres.
///
/// A presentation choice, like everything below: closer than the menu
/// panel, since a tile is small and read at a glance.
const TILE_DISTANCE: f32 = 1.2;

/// How far left and right of straight ahead the side tiles' outer edges
/// sit, metres. About 35° out at [`TILE_DISTANCE`]: a glance away, clear of
/// where the aim goes.
const TILE_SPREAD: f32 = 0.85;

/// How far above eye level the top tiles' upper edges sit, metres. About
/// 27° up at [`TILE_DISTANCE`].
const TILE_RISE: f32 = 0.6;

/// How far below eye level the bottom tiles' lower edges sit, metres. About
/// 27° down at [`TILE_DISTANCE`].
const TILE_DROP: f32 = 0.6;

/// How tall the whole HUD image would be on the tiles, metres.
///
/// The HUD sizes itself by the screen's height, so a fixed height here
/// gives the original's clusters a fixed size in the headset whatever the
/// window, and the HUD scale setting still scales them.
const SCREEN_HEIGHT_METRES: f32 = 0.86;

/// How tall the whole HUD image would be on a hand, metres: smaller than
/// [`SCREEN_HEIGHT_METRES`] by about the ratio of a held-up hand's distance
/// (half a metre) to [`TILE_DISTANCE`], so a cluster looks about as large.
const HAND_SCREEN_HEIGHT_METRES: f32 = 0.35;

/// How far above the grip (the palm) a hand's tiles lie, metres: clear of
/// the controller's top.
const HAND_LIFT: f32 = 0.05;

/// The gap between tiles stacked on one hand, metres.
const HAND_GAP: f32 = 0.01;

/// How far a hand's tiles lean back from upright, radians. A quarter turn
/// lays them flat on the controller's top, facing up: read from above, as a
/// glance down at hands held near the body reads them. Less stands them up
/// toward the player.
const HAND_TILT: f32 = std::f32::consts::FRAC_PI_2;

/// Which hand each region rides on, each hand's from the bottom up.
const HAND_MOUNTS: [(Anchor, Side); 6] = [
    (anchor(Column::Left, Row::Bottom), Side::Left),
    (anchor(Column::Left, Row::Top), Side::Left),
    (anchor(Column::Centre, Row::Top), Side::Left),
    (anchor(Column::Right, Row::Bottom), Side::Right),
    (anchor(Column::Centre, Row::Bottom), Side::Right),
    (anchor(Column::Right, Row::Top), Side::Right),
];

/// An anchor, briefly.
const fn anchor(column: Column, row: Row) -> Anchor {
    Anchor { column, row }
}

/// The six regions the original anchors its HUD to.
const REGIONS: [Anchor; 6] = [
    Anchor {
        column: Column::Left,
        row: Row::Top,
    },
    Anchor {
        column: Column::Centre,
        row: Row::Top,
    },
    Anchor {
        column: Column::Right,
        row: Row::Top,
    },
    Anchor {
        column: Column::Left,
        row: Row::Bottom,
    },
    Anchor {
        column: Column::Centre,
        row: Row::Bottom,
    },
    Anchor {
        column: Column::Right,
        row: Row::Bottom,
    },
];

/// The HUD tiles.
pub(crate) struct HudTilesPlugin;

impl Plugin for HudTilesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_tiles.after(spawn_panel))
            .add_systems(Update, layout_tiles);
    }
}

/// The tile showing one region of the HUD.
#[derive(Component, Debug, Clone, Copy)]
struct HudTile(Anchor);

/// One tile per region, hidden until there is a HUD to show.
fn spawn_tiles(
    mut commands: Commands,
    panel: Res<Panel>,
    root: Single<Entity, With<XrTrackingRoot>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PanelMaterial>>,
) {
    let mesh = meshes.add(Rectangle::new(1.0, 1.0));
    for anchor in REGIONS {
        commands.spawn((
            Name::new("VR HUD tile"),
            HudTile(anchor),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(materials.add(PanelMaterial::whole(panel.image.clone()))),
            Transform::default(),
            Visibility::Hidden,
            RenderLayers::layer(PANEL_LAYER),
            ChildOf(*root),
        ));
    }
}

/// Show the HUD as tiles while playing, and the whole panel otherwise.
// Twelve parameters: what decides the mode, the HUD's nodes, the hands, the
// tiles and the panel.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn layout_tiles(
    gate: GameplayGate,
    large_map: Res<AutomapLarge>,
    panel: Res<Panel>,
    images: Res<Assets<Image>>,
    huds: Query<(Entity, &InheritedVisibility), With<HudRoot>>,
    maps: Query<&InheritedVisibility, With<HudMap>>,
    children: Query<&Children>,
    drawn: Query<
        (&ComputedNode, &UiGlobalTransform, &InheritedVisibility),
        Or<(With<ImageNode>, With<Text>)>,
    >,
    grips: Query<(&Grip, &Transform, &OxrSpaceLocationFlags), Without<HudTile>>,
    mut tiles: Query<(
        &HudTile,
        &MeshMaterial3d<PanelMaterial>,
        &mut Transform,
        &mut Visibility,
    )>,
    mut panels: Query<&mut Visibility, Without<HudTile>>,
    mut materials: ResMut<Assets<PanelMaterial>>,
) {
    let size = images
        .get(&panel.image)
        .map(|image| image.size().as_vec2())
        .filter(|size| size.min_element() > 1.0);
    let hud = huds
        .iter()
        .find(|(_, visibility)| visibility.get())
        .map(|(entity, _)| entity);
    let map_large = large_map.0 && maps.iter().any(|visibility| visibility.get());
    let split = size.zip(hud).filter(|_| gate.enabled() && !map_large);

    if let Ok(mut visibility) = panels.get_mut(panel.entity) {
        visibility.set_if_neq(match split {
            Some(_) => Visibility::Hidden,
            None => Visibility::Inherited,
        });
    }
    let Some((size, hud)) = split else {
        for (.., mut visibility) in &mut tiles {
            visibility.set_if_neq(Visibility::Hidden);
        }
        return;
    };

    // The image target's scale factor is 1, so the nodes' physical pixels
    // are the image's pixels.
    let rects: Vec<Rect> = children
        .iter_descendants(hud)
        .filter_map(|entity| drawn.get(entity).ok())
        .filter(|(_, _, visibility)| visibility.get())
        .map(|(node, transform, _)| {
            Rect::from_center_size(transform.affine().translation, node.size())
        })
        .collect();
    let bounds_of = |anchor: Anchor| cluster_bounds(region_rect(anchor, size), &rects);
    let metres_per_pixel = SCREEN_HEIGHT_METRES / size.y;
    let hand_metres_per_pixel = HAND_SCREEN_HEIGHT_METRES / size.y;

    for (tile, material, mut transform, mut visibility) in &mut tiles {
        let Some(bounds) = bounds_of(tile.0) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };

        let uv_rect = Vec4::new(
            bounds.min.x / size.x,
            bounds.min.y / size.y,
            bounds.width() / size.x,
            bounds.height() / size.y,
        );
        if materials
            .get(&material.0)
            .is_some_and(|current| current.uv_rect != uv_rect)
            && let Some(mut current) = materials.get_mut(&material.0)
        {
            current.uv_rect = uv_rect;
        }

        let on_hand = HAND_MOUNTS
            .iter()
            .find(|(anchor, _)| *anchor == tile.0)
            .and_then(|&(_, side)| Some((side, located_grip(&grips, side)?)));
        let pose = match on_hand {
            Some((side, grip)) => {
                // The heights of the tiles below this one on the same hand.
                let below: f32 = HAND_MOUNTS
                    .iter()
                    .filter(|(_, other)| *other == side)
                    .take_while(|(anchor, _)| *anchor != tile.0)
                    .filter_map(|&(anchor, _)| bounds_of(anchor))
                    .map(|below| below.height() * hand_metres_per_pixel + HAND_GAP)
                    .sum();
                hand_tile_pose(grip, below, bounds.size() * hand_metres_per_pixel)
            }
            None => tile_pose(tile.0, bounds.size() * metres_per_pixel),
        };
        if *transform != pose {
            *transform = pose;
        }
        visibility.set_if_neq(Visibility::Inherited);
    }
}

/// The part of an image `size` pixels large that `anchor`'s region covers:
/// a third of its width, a half of its height.
fn region_rect(anchor: Anchor, size: Vec2) -> Rect {
    let third = size.x / 3.0;
    let x = match anchor.column {
        Column::Left => 0.0,
        Column::Centre => third,
        Column::Right => 2.0 * third,
    };
    let y = match anchor.row {
        Row::Top => 0.0,
        Row::Bottom => size.y / 2.0,
    };
    Rect::new(x, y, x + third, y + size.y / 2.0)
}

/// The box around everything drawn inside `region`, or `None` when nothing
/// is.
///
/// Anything covering more than half the screen is a full-screen layer, not
/// part of a cluster, and is left out; counting it would stretch every tile
/// to its whole region.
fn cluster_bounds(region: Rect, rects: &[Rect]) -> Option<Rect> {
    let screen_area = region.width() * region.height() * 6.0;
    rects
        .iter()
        .filter(|rect| rect.width() * rect.height() <= screen_area / 2.0)
        .map(|rect| rect.intersect(region))
        .filter(|clipped| !clipped.is_empty())
        .reduce(|bounds, clipped| bounds.union(clipped))
}

/// Where a tile `size` metres large hangs for `anchor`'s region, in the
/// seat's space (the head at the origin, looking down −z).
///
/// Its anchored side sits at a fixed point (a left region's left edge, a
/// top region's top edge, a centre region's middle), so a cluster that
/// grows, like the message tab sliding out, grows away from it as it does
/// on screen. The tile turns to face the head.
fn tile_pose(anchor: Anchor, size: Vec2) -> Transform {
    let corner = Vec3::new(
        match anchor.column {
            Column::Left => -TILE_SPREAD,
            Column::Centre => 0.0,
            Column::Right => TILE_SPREAD,
        },
        match anchor.row {
            Row::Top => TILE_RISE,
            Row::Bottom => -TILE_DROP,
        },
        -TILE_DISTANCE,
    );
    let to_centre = Vec2::new(
        match anchor.column {
            Column::Left => size.x / 2.0,
            Column::Centre => 0.0,
            Column::Right => -size.x / 2.0,
        },
        match anchor.row {
            Row::Top => -size.y / 2.0,
            Row::Bottom => size.y / 2.0,
        },
    );
    // Looking away from the head turns the quad's face toward it.
    let rotation = Transform::default().looking_to(corner, Vec3::Y).rotation;
    Transform::from_translation(corner + rotation * to_centre.extend(0.0))
        .with_rotation(rotation)
        .with_scale(size.extend(1.0))
}

/// Where a tile `size` metres large sits on a hand whose grip pose is
/// `grip`, with `below` metres of other tiles under it.
///
/// In the grip's frame (+y out of the top of the controller, −z along it):
/// [`HAND_LIFT`] over the grip, leaning back by [`HAND_TILT`] (flat, face
/// up, its top toward the controller's front), and stacked along its own
/// up, away from the hand.
fn hand_tile_pose(grip: Transform, below: f32, size: Vec2) -> Transform {
    let lean = Quat::from_rotation_x(-HAND_TILT);
    let up = lean * Vec3::Y;
    let centre = Vec3::Y * HAND_LIFT + up * (below + size.y / 2.0);
    grip * Transform::from_translation(centre)
        .with_rotation(lean)
        .with_scale(size.extend(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Vec2 = Vec2::new(1200.0, 800.0);

    fn region(column: Column, row: Row) -> Rect {
        region_rect(Anchor { column, row }, SCREEN)
    }

    /// The six regions tile the screen: thirds across, halves down.
    #[test]
    fn the_regions_are_thirds_and_halves() {
        assert_eq!(
            region(Column::Left, Row::Top),
            Rect::new(0.0, 0.0, 400.0, 400.0)
        );
        assert_eq!(
            region(Column::Centre, Row::Bottom),
            Rect::new(400.0, 400.0, 800.0, 800.0)
        );
        assert_eq!(
            region(Column::Right, Row::Bottom),
            Rect::new(800.0, 400.0, 1200.0, 800.0)
        );
    }

    /// A strip across the whole width is cut at the seams, so each third
    /// shows its own part of it and no tile overlaps another.
    #[test]
    fn a_strip_across_the_width_is_cut_at_the_seams() {
        let strip = [Rect::new(0.0, 760.0, 1200.0, 800.0)];
        for column in [Column::Left, Column::Centre, Column::Right] {
            let region = region(column, Row::Bottom);
            assert_eq!(
                cluster_bounds(region, &strip),
                Some(Rect::new(region.min.x, 760.0, region.max.x, 800.0))
            );
        }
        assert_eq!(cluster_bounds(region(Column::Left, Row::Top), &strip), None);
    }

    /// A tile is only as large as what is drawn in its region, and a
    /// full-screen layer does not stretch it.
    #[test]
    fn a_tile_hugs_its_cluster() {
        let drawn = [
            Rect::new(10.0, 700.0, 110.0, 790.0),
            Rect::new(120.0, 750.0, 200.0, 780.0),
            Rect::new(0.0, 0.0, 1200.0, 800.0),
        ];
        assert_eq!(
            cluster_bounds(region(Column::Left, Row::Bottom), &drawn),
            Some(Rect::new(10.0, 700.0, 200.0, 790.0))
        );
        assert_eq!(
            cluster_bounds(region(Column::Right, Row::Top), &drawn),
            None
        );
    }

    #[test]
    fn a_hand_tile_lies_face_up_on_the_controller() {
        let size = Vec2::new(0.1, 0.05);

        // A controller held level, pointing ahead.
        let level = hand_tile_pose(Transform::default(), 0.06, size);
        let face = level.rotation * Vec3::Z;
        assert!(face.y > 0.99, "faces up: {face:?}");
        assert!((level.translation.y - HAND_LIFT).abs() < 1e-6, "{level:?}");
        // Read from above and behind, its top is toward the front and its
        // image runs left to right across the hand.
        assert!((level.rotation * Vec3::Y).z < -0.99);
        assert!((level.rotation * Vec3::X).x > 0.99);
        // A tile stacked on it lies further forward.
        let lower = hand_tile_pose(Transform::default(), 0.0, size);
        assert!(level.translation.z < lower.translation.z);

        // Turning the controller turns the tile with it. (`angle_between`
        // goes through `acos`, good to about 3e-4 near zero in `f32`.)
        let turned = Transform::from_rotation(Quat::from_rotation_y(1.0));
        let pose = hand_tile_pose(turned, 0.06, size);
        assert!(
            pose.rotation
                .angle_between(turned.rotation * level.rotation)
                < 1e-3
        );
    }

    /// Every tile turns its face to the head, and keeps its anchored side
    /// where the region's anchor point is.
    #[test]
    fn tiles_face_the_head_from_their_anchors() {
        let size = Vec2::new(0.3, 0.1);
        for anchor in REGIONS {
            let pose = tile_pose(anchor, size);
            let to_head = -pose.translation.normalize();
            assert!(
                pose.back().dot(to_head) > 0.95,
                "{anchor:?} faces away from the head"
            );
        }

        let bottom_left = tile_pose(
            Anchor {
                column: Column::Left,
                row: Row::Bottom,
            },
            size,
        );
        let left_edge = bottom_left.translation - bottom_left.right() * size.x / 2.0;
        let bottom_edge = left_edge - bottom_left.up() * size.y / 2.0;
        assert!(
            (bottom_edge - Vec3::new(-TILE_SPREAD, -TILE_DROP, -TILE_DISTANCE)).length() < 1e-5
        );
    }
}
