//! The automap (FR-U1, FR-M3 display): the level's `zmap` polylines, the
//! player, and the active objectives, in a corner of the HUD.
//!
//! # Rasterised on the CPU, not drawn by a second camera
//!
//! The task suggested a top-down 2D camera rendering gizmo polylines into a
//! `RenderTarget` image. The data made that more machinery than it is worth:
//! a real zmap is 5–16 polylines of 28–256 vertices (docs/data/coordinates.md, measured
//! over m1–m6), which rasterise into a 512-texel image in well under a
//! millisecond. So the lines are drawn once into an [`Image`] when the level
//! loads, the active objectives into a second image that is redrawn only when
//! the set of active objectives changes, and the player marker is a UI node
//! placed over both. No extra camera, no render layer to keep the level's
//! gizmos out of, and the world → map mapping is a pure function the tests
//! pin down.
//!
//! # Which levels have a map
//!
//! Only those whose catalog entry [`has_real_zmap`]: m7–m12 ship
//! byte-identical copies of m1's and m13 has none (docs/data/quirks.md), and
//! showing m1's coastline on a deathmatch map would be worse than nothing.
//!
//! [`has_real_zmap`]: bft_assets::catalog::LevelEntry::has_real_zmap

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bft_assets::catalog::{LevelCatalog, LevelId};
use bft_assets::level::ZMap;
use bft_sim::mission::{MissionState, ObjectiveState};
use bft_sim::states::AppState;
use bft_sim::vehicle::PlayerVehicle;

use super::bindings::Action;
use super::gameplay_input_enabled;
use super::hud::{HudMap, HudRoot};
use super::theme::{ACCENT, CYAN, TEXT_DIM};
use crate::level_view::LoadedLevel;

/// The longer side of the map image, in texels.
const MAP_TEXELS: u32 = 512;
/// The map's longer side on screen, small and enlarged, logical pixels.
const SMALL_SIZE: f32 = 220.0;
/// Enlarged, as a fraction of the window's height.
const LARGE_FRACTION: f32 = 0.8;
/// Line thickness in texels: the base map and the highlighted objectives.
const LINE_RADIUS: f32 = 1.5;
const HIGHLIGHT_RADIUS: f32 = 3.5;

/// How world `x`/`z` map onto the image.
///
/// World `x` runs right and world `z` runs **down** the image. Seen from
/// above (+y) in a right-handed frame with x to the right, +z points toward
/// the bottom of the screen, so this is the unmirrored top-down view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapProjection {
    /// The zmap's extent in world `x`/`z`.
    pub min: Vec2,
    pub max: Vec2,
    /// The image size in texels, aspect-matched to the extent.
    pub size: UVec2,
}

impl MapProjection {
    /// Fit an extent into an image whose longer side is `texels`.
    pub fn fit(min: Vec3, max: Vec3, texels: u32) -> MapProjection {
        let min = Vec2::new(min.x, min.z);
        let mut max = Vec2::new(max.x, max.z);
        // A degenerate extent (a zmap with no area) must not divide by zero.
        max = max.max(min + Vec2::ONE);

        let extent = max - min;
        let scale = texels as f32 / extent.max_element();
        let size = (extent * scale).round().max(Vec2::ONE).as_uvec2();

        MapProjection { min, max, size }
    }

    /// Where a world point lands on the image, in texels from the top left.
    pub fn world_to_map(&self, world: Vec3) -> Vec2 {
        let fraction = (Vec2::new(world.x, world.z) - self.min) / (self.max - self.min);
        fraction * self.size.as_vec2()
    }

    /// The same, as a 0–1 fraction of the image, for placing UI over it.
    pub fn world_to_fraction(&self, world: Vec3) -> Vec2 {
        self.world_to_map(world) / self.size.as_vec2()
    }
}

/// An RGBA8 image being drawn into.
#[derive(Debug, Clone)]
pub struct Raster {
    pub size: UVec2,
    pub pixels: Vec<[u8; 4]>,
}

impl Raster {
    /// A transparent image.
    pub fn new(size: UVec2) -> Raster {
        Raster {
            size,
            pixels: vec![[0; 4]; (size.x * size.y) as usize],
        }
    }

    /// The pixel at a texel, if it is inside.
    pub fn get(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        (x < self.size.x && y < self.size.y).then(|| self.pixels[(y * self.size.x + x) as usize])
    }

    /// Draw a polyline of the given radius.
    ///
    /// Each segment is walked in half-texel steps stamping a disc: crude next
    /// to a real line rasteriser, but thick lines, joins and caps all come out
    /// right, and a map this size is a few thousand stamps.
    pub fn polyline(&mut self, points: &[Vec2], colour: [u8; 4], radius: f32) {
        match points {
            [] => {}
            [only] => self.disc(*only, colour, radius),
            _ => {
                for pair in points.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    let steps = (a.distance(b) * 2.0).ceil().max(1.0) as usize;
                    for step in 0..=steps {
                        self.disc(a.lerp(b, step as f32 / steps as f32), colour, radius);
                    }
                }
            }
        }
    }

    fn disc(&mut self, centre: Vec2, colour: [u8; 4], radius: f32) {
        let low = (centre - radius).floor().max(Vec2::ZERO);
        let high = (centre + radius).ceil().min(self.size.as_vec2() - 1.0);
        if high.x < low.x || high.y < low.y {
            return;
        }
        for y in low.y as u32..=high.y as u32 {
            for x in low.x as u32..=high.x as u32 {
                let texel = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                if texel.distance_squared(centre) <= radius * radius {
                    self.pixels[(y * self.size.x + x) as usize] = colour;
                }
            }
        }
    }

    /// Into a Bevy image the UI can show.
    pub fn into_image(self) -> Image {
        let data = self.pixels.into_iter().flatten().collect();
        Image::new(
            Extent3d {
                width: self.size.x,
                height: self.size.y,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    }
}

/// The base map: every feature in its own colour (the data-dependent item
/// says "use `color` as-is"), over a translucent backing.
pub fn draw_base(zmap: &ZMap, projection: &MapProjection) -> Raster {
    let mut raster = Raster::new(projection.size);
    for feature in &zmap.features {
        let points: Vec<Vec2> = feature
            .vertices
            .iter()
            .map(|vertex| projection.world_to_map(*vertex))
            .collect();
        let [r, g, b] = feature.color;
        raster.polyline(&points, [r, g, b, 255], LINE_RADIUS);
    }
    raster
}

/// The highlight layer: the features whose `objective` is one of `active`,
/// thicker and in the accent colour.
pub fn draw_objectives(zmap: &ZMap, projection: &MapProjection, active: &[u8]) -> Raster {
    let mut raster = Raster::new(projection.size);
    let colour = ACCENT.to_srgba().to_u8_array();
    for feature in &zmap.features {
        let Some(objective) = feature.objective else {
            continue;
        };
        if !active.iter().any(|index| i32::from(*index) == objective) {
            continue;
        }
        let points: Vec<Vec2> = feature
            .vertices
            .iter()
            .map(|vertex| projection.world_to_map(*vertex))
            .collect();
        raster.polyline(&points, colour, HIGHLIGHT_RADIUS);
    }
    raster
}

/// The indices of the objectives the player is working on.
pub fn active_objectives(state: Option<&MissionState>) -> Vec<u8> {
    state
        .map(|state| {
            state
                .objectives
                .iter()
                .enumerate()
                .filter(|(_, objective)| **objective == ObjectiveState::Active)
                .map(|(index, _)| index as u8)
                .collect()
        })
        .unwrap_or_default()
}

/// The built map for the current level.
#[derive(Resource, Debug, Clone)]
pub struct Automap {
    pub level: LevelId,
    pub projection: MapProjection,
    zmap: Handle<ZMap>,
    highlight: Handle<Image>,
    /// The objective set the highlight was drawn for.
    drawn_for: Vec<u8>,
}

/// Whether the map is enlarged.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutomapLarge(pub bool);

#[derive(Component, Debug, Clone, Copy)]
struct HighlightLayer;

#[derive(Component, Debug, Clone, Copy)]
struct PlayerMarker;

/// The automap's systems.
pub fn plugin(app: &mut App) {
    app.init_resource::<AutomapLarge>()
        .add_systems(OnExit(AppState::InGame), forget_map)
        .add_systems(
            Update,
            (
                build_map,
                toggle_size.run_if(gameplay_input_enabled),
                redraw_objectives,
                place_marker,
                size_map,
            )
                .chain()
                .run_if(in_state(AppState::InGame)),
        );
}

fn forget_map(mut commands: Commands, mut large: ResMut<AutomapLarge>) {
    commands.remove_resource::<Automap>();
    large.0 = false;
}

/// Build the map once the level's zmap has loaded.
// Seven parameters: the level, what the catalog says of it, the two asset
// stores and the HUD it attaches to.
#[allow(clippy::too_many_arguments)]
fn build_map(
    mut commands: Commands,
    loaded: Option<Res<LoadedLevel>>,
    built: Option<Res<Automap>>,
    catalog: Res<LevelCatalog>,
    zmaps: Res<Assets<ZMap>>,
    mut images: ResMut<Assets<Image>>,
    huds: Query<Entity, With<HudRoot>>,
    state: Option<Res<MissionState>>,
) {
    let Some(loaded) = loaded else {
        return;
    };
    if built.is_some_and(|built| built.level == loaded.id) {
        return;
    }
    // docs/data/quirks.md: m7–m12's maps are m1's, and m13 has none.
    if !catalog
        .level(loaded.id)
        .is_some_and(|entry| entry.has_real_zmap())
    {
        return;
    }
    let Some(handle) = loaded.zmap.clone() else {
        return;
    };
    let Some(zmap) = zmaps.get(&handle) else {
        return;
    };
    let Some(hud) = huds.iter().next() else {
        return;
    };

    let projection = MapProjection::fit(zmap.min, zmap.max, MAP_TEXELS);
    let active = active_objectives(state.as_deref());
    let base = images.add(draw_base(zmap, &projection).into_image());
    let highlight = images.add(draw_objectives(zmap, &projection, &active).into_image());

    info!(
        "automap for {}: {} feature(s), {}x{} texels",
        loaded.id,
        zmap.features.len(),
        projection.size.x,
        projection.size.y
    );

    let map = commands
        .spawn((
            HudMap,
            Name::new("automap"),
            Node {
                position_type: PositionType::Absolute,
                top: px(16),
                right: px(16),
                border: UiRect::all(px(1)),
                ..default()
            },
            BorderColor::all(TEXT_DIM),
            BackgroundColor(Color::srgba(0.0, 0.02, 0.04, 0.6)),
            Pickable::IGNORE,
        ))
        .with_children(|frame| {
            frame.spawn((
                ImageNode::new(base),
                Node {
                    width: percent(100),
                    height: percent(100),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            frame.spawn((
                ImageNode::new(highlight.clone()),
                HighlightLayer,
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    height: percent(100),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            frame.spawn((
                PlayerMarker,
                Node {
                    position_type: PositionType::Absolute,
                    width: px(10),
                    height: px(10),
                    // Centred on its position: half its size back.
                    margin: UiRect::new(px(-5), px(0), px(-5), px(0)),
                    justify_content: JustifyContent::Center,
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BackgroundColor(CYAN),
                UiTransform::IDENTITY,
                Visibility::Hidden,
                Pickable::IGNORE,
                // The nose, pointing along the hull's heading.
                children![(
                    Node {
                        position_type: PositionType::Absolute,
                        width: px(2),
                        height: px(9),
                        top: px(-8),
                        ..default()
                    },
                    BackgroundColor(CYAN),
                    Pickable::IGNORE,
                )],
            ));
        })
        .id();
    commands.entity(hud).add_child(map);

    commands.insert_resource(Automap {
        level: loaded.id,
        projection,
        zmap: handle,
        highlight,
        drawn_for: active,
    });
}

fn toggle_size(input: super::bindings::ActionInput, mut large: ResMut<AutomapLarge>) {
    if input.just_pressed(Action::ToggleMap) {
        large.0 = !large.0;
    }
}

/// Redraw the objective layer when the active set changes, and pulse it.
fn redraw_objectives(
    time: Res<Time<Real>>,
    map: Option<ResMut<Automap>>,
    state: Option<Res<MissionState>>,
    zmaps: Res<Assets<ZMap>>,
    mut images: ResMut<Assets<Image>>,
    mut layers: Query<&mut ImageNode, With<HighlightLayer>>,
) {
    let Some(mut map) = map else {
        return;
    };

    if state.as_ref().is_some_and(|state| state.is_changed()) {
        let active = active_objectives(state.as_deref());
        if active != map.drawn_for
            && let Some(zmap) = zmaps.get(&map.zmap)
        {
            let raster = draw_objectives(zmap, &map.projection, &active);
            if let Some(mut image) = images.get_mut(&map.highlight) {
                *image = raster.into_image();
            }
            map.drawn_for = active;
        }
    }

    let pulse = 0.7 + 0.3 * (time.elapsed_secs() * 4.0).sin();
    for mut layer in &mut layers {
        layer.color = Color::WHITE.with_alpha(pulse);
    }
}

/// The marker's rotation for a hull rotation: clockwise from "up the map".
///
/// Forward is −z. On the map +x is right and +z is down, so the hull's
/// heading on the map is its forward's (x, z); `UiTransform` rotates
/// clockwise, and a clockwise turn of θ takes "up" (0, −1) to
/// (sin θ, −cos θ).
pub fn marker_angle(rotation: Quat) -> f32 {
    let forward = rotation * Vec3::NEG_Z;
    forward.x.atan2(-forward.z)
}

fn place_marker(
    map: Option<Res<Automap>>,
    players: Query<&GlobalTransform, With<PlayerVehicle>>,
    mut markers: Query<(&mut Node, &mut UiTransform, &mut Visibility), With<PlayerMarker>>,
) {
    let (Some(map), Some(player)) = (map, players.iter().next()) else {
        return;
    };
    let (_, rotation, translation) = player.to_scale_rotation_translation();
    let fraction = map.projection.world_to_fraction(translation);
    let inside = fraction.cmpge(Vec2::ZERO).all() && fraction.cmple(Vec2::ONE).all();

    for (mut node, mut transform, mut visibility) in &mut markers {
        visibility.set_if_neq(if inside {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        node.left = percent(fraction.x * 100.0);
        node.top = percent(fraction.y * 100.0);
        transform.rotation = Rot2::radians(marker_angle(rotation));
    }
}

/// Small in the corner, or large in the middle of the screen.
///
/// Under the original HUD there is no corner map — the original's area map
/// is an overlay on its own key ("Bring Up the Area Map M", manual) — so
/// the map shows only large.
fn size_map(
    large: Res<AutomapLarge>,
    map: Option<Res<Automap>>,
    windows: Query<&Window>,
    style: Option<Res<super::hud::HudStyle>>,
    settings: Option<Res<super::settings::Settings>>,
    mut frames: Query<&mut Node, With<HudMap>>,
) {
    let original = style
        .as_deref()
        .zip(settings.as_deref())
        .is_some_and(|(style, settings)| super::hud::uses_original(style, settings));
    let display = if original && !large.0 {
        Display::None
    } else {
        Display::Flex
    };
    for mut node in &mut frames {
        if node.display != display {
            node.display = display;
        }
    }

    let Some(map) = map else {
        return;
    };
    let window_height = windows.iter().next().map(Window::height).unwrap_or(720.0);
    let longest = if large.0 {
        window_height * LARGE_FRACTION
    } else {
        SMALL_SIZE
    };
    let size = map.projection.size.as_vec2() / map.projection.size.max_element() as f32 * longest;

    for mut node in &mut frames {
        node.width = px(size.x);
        node.height = px(size.y);
        if large.0 {
            node.top = percent(50);
            node.right = percent(50);
            node.margin = UiRect::new(px(0), px(-size.x / 2.0), px(-size.y / 2.0), px(0));
        } else {
            node.top = px(16);
            node.right = px(16);
            node.margin = UiRect::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::level::ZMapFeature;

    /// World → map pixel. m1's real extent is 0…2768.8 × 0…3423.5
    /// (docs/data/coordinates.md); z is the longer side, so it gets the full 512.
    #[test]
    fn world_points_land_on_the_right_map_pixels() {
        let projection = MapProjection::fit(Vec3::ZERO, Vec3::new(2768.7866, 0.0, 3423.5288), 512);

        assert_eq!(projection.size, UVec2::new(414, 512));

        // Corners: the origin is the top left, the far corner the bottom
        // right. Height is ignored.
        assert_eq!(
            projection.world_to_map(Vec3::new(0.0, 99.0, 0.0)),
            Vec2::ZERO
        );
        let far = projection.world_to_map(Vec3::new(2768.7866, 0.0, 3423.5288));
        assert!((far - Vec2::new(414.0, 512.0)).length() < 1e-3, "{far}");

        // +x is right, +z is down.
        let centre = projection.world_to_map(Vec3::new(1384.3933, 0.0, 1711.7644));
        assert!(
            (centre - Vec2::new(207.0, 256.0)).length() < 1e-3,
            "{centre}"
        );
        let east = projection.world_to_map(Vec3::new(2000.0, 0.0, 1711.7644));
        let south = projection.world_to_map(Vec3::new(1384.3933, 0.0, 3000.0));
        assert!(east.x > centre.x && (east.y - centre.y).abs() < 1e-3);
        assert!(south.y > centre.y && (south.x - centre.x).abs() < 1e-3);
    }

    #[test]
    fn a_degenerate_extent_does_not_divide_by_zero() {
        let projection = MapProjection::fit(Vec3::ONE, Vec3::ONE, 64);
        let point = projection.world_to_map(Vec3::ONE);
        assert!(point.is_finite());
    }

    fn feature(colour: [u8; 3], objective: Option<i32>) -> ZMapFeature {
        ZMapFeature {
            color: colour,
            vertices: vec![Vec3::new(10.0, 0.0, 50.0), Vec3::new(90.0, 0.0, 50.0)],
            objective,
        }
    }

    #[test]
    fn features_are_drawn_in_their_own_colour_and_objectives_on_top() {
        let zmap = ZMap {
            min: Vec3::ZERO,
            max: Vec3::new(100.0, 0.0, 100.0),
            features: vec![
                feature([0, 0, 255], None),
                feature([255, 255, 255], Some(2)),
            ],
        };
        let projection = MapProjection::fit(zmap.min, zmap.max, 100);

        let base = draw_base(&zmap, &projection);
        // The second feature overdraws the first along the same line.
        assert_eq!(base.get(50, 50), Some([255, 255, 255, 255]));
        assert_eq!(base.get(50, 10), Some([0, 0, 0, 0]));

        let none = draw_objectives(&zmap, &projection, &[0, 1]);
        assert_eq!(none.get(50, 50), Some([0, 0, 0, 0]));
        let some = draw_objectives(&zmap, &projection, &[2]);
        assert_eq!(some.get(50, 50).map(|pixel| pixel[3]), Some(255));
    }

    #[test]
    fn the_marker_points_along_the_hull() {
        // Facing −z, which is up the map.
        assert!(marker_angle(Quat::IDENTITY).abs() < 1e-6);
        // A left turn (positive yaw) faces −x, which is left on the map: a
        // clockwise rotation of −90°.
        let left = marker_angle(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        assert!((left + std::f32::consts::FRAC_PI_2).abs() < 1e-5, "{left}");
    }
}
