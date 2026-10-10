//! The mouse cursor: the reticle in play, a pointer everywhere else.
//!
//! Aiming is a free cursor and the turret slews to where it points, so in
//! play **the cursor is the reticle**. Under the original HUD it is the
//! game's own `ret_off`/`ret_range`/`ret_on`, in the look
//! [`reticle_texture`] picks; the original drew no second mark at the
//! turret's aim point, and neither do we. Under the simple HUD, without an
//! install, or when one of those images is missing, it is our own
//! crosshair, drawn in code ([`crosshair`]) so that it needs no file at
//! all. It has the same three [`ReticleState`]s, in the original's colours:
//! grey with nothing in range, red with something in range, green on
//! something it can hurt.
//!
//! In menus, overlays and cinematics it is a plain pointer: the game's own
//! (`dialog.zrd`'s `CURSOR`, an arrow like the system one) when the install
//! has it, the system arrow otherwise.
//!
//! Always an OS cursor image (`CursorIcon::Custom`) rather than a UI node
//! chasing the pointer: the OS draws it at the pointer with no frame of lag,
//! which a software cursor cannot match, and if an image never loads the
//! platform keeps its own arrow — there is no state with no cursor at all.
//! The game's images are 640×480-era sizes, so they are scaled up by a whole
//! number close to the HUD's scale (nearest, so they stay crisp).

use bevy::asset::RenderAssetUsages;
use bevy::image::TextureFormatPixelInfo;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorIcon, CustomCursor, CustomCursorImage, PrimaryWindow, SystemCursorIcon};
use bft_sim::damage::AimResult;
use bft_sim::vehicle::PlayerVehicle;

use super::GameplayGate;
use super::hud::original::{HudArt, reticle_texture};
use super::hud::{HudStyle, uses_original};
use super::settings::Settings;

/// The crosshair's side in texels; odd, so it has a centre texel.
const CROSSHAIR_SIZE: u32 = 33;
/// The crosshair's hotspot: its centre texel.
const CROSSHAIR_HOTSPOT: (u16, u16) = (16, 16);
/// The crosshair's colour in each [`ReticleState`], and its outline's,
/// which keeps it readable against bright ground.
const CROSSHAIR_OFF: [u8; 4] = [230, 230, 230, 255];
const CROSSHAIR_RANGE: [u8; 4] = [255, 60, 40, 255];
const CROSSHAIR_ON: [u8; 4] = [60, 230, 60, 255];
const CROSSHAIR_OUTLINE: [u8; 4] = [0, 0, 0, 170];

/// What the reticle says about the aim: the original's three looks
/// (`ret_off`, `ret_range`, `ret_on`), which our crosshair follows too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReticleState {
    /// Nothing within the selected weapon's range.
    Off,
    /// A shot would hit something, but nothing it can hurt.
    Range,
    /// A shot would hit something it can hurt.
    On,
}

impl ReticleState {
    /// The state for this tick's aim. The sim casts the aim ray to the
    /// weapon's range, so any hit is "in range".
    pub fn of(aim: &AimResult) -> ReticleState {
        match (aim.hit, aim.damageable) {
            (None, _) => ReticleState::Off,
            (Some(_), false) => ReticleState::Range,
            (Some(_), true) => ReticleState::On,
        }
    }
}

/// Our crosshair's images, one per [`ReticleState`].
#[derive(Resource, Debug, Clone)]
struct CrosshairImages {
    off: Handle<Image>,
    range: Handle<Image>,
    on: Handle<Image>,
}

impl CrosshairImages {
    fn look(&self, state: ReticleState) -> Look {
        let image = match state {
            ReticleState::Off => &self.off,
            ReticleState::Range => &self.range,
            ReticleState::On => &self.on,
        };
        Look::Image {
            image: image.clone(),
            hotspot: CROSSHAIR_HOTSPOT,
        }
    }
}

/// The game's cursor images, scaled for the window, by texture name.
#[derive(Resource, Debug, Default)]
struct ScaledCursors {
    factor: u32,
    images: HashMap<String, Handle<Image>>,
}

/// What the window's cursor should be.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Look {
    System,
    Image {
        image: Handle<Image>,
        hotspot: (u16, u16),
    },
}

/// Which look is on the window.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
struct Applied(Option<Look>);

/// The cursor's systems.
pub fn plugin(app: &mut App) {
    app.init_resource::<Applied>()
        .init_resource::<ScaledCursors>()
        .add_systems(Startup, load_crosshair)
        .add_systems(Update, set_cursor);
}

fn load_crosshair(mut commands: Commands, images: Option<ResMut<Assets<Image>>>) {
    let Some(mut images) = images else {
        return;
    };
    commands.insert_resource(CrosshairImages {
        off: images.add(crosshair(CROSSHAIR_OFF)),
        range: images.add(crosshair(CROSSHAIR_RANGE)),
        on: images.add(crosshair(CROSSHAIR_ON)),
    });
}

/// Whether a texel `(dx, dy)` from the crosshair's centre is part of its
/// mark: four arms around a gap, and a dot in the middle.
fn crosshair_mark(dx: i32, dy: i32) -> bool {
    let arm = |along: i32, across: i32| across.abs() <= 1 && (4..=13).contains(&along.abs());
    (dx == 0 && dy == 0) || arm(dx, dy) || arm(dy, dx)
}

/// Our crosshair in `color`, outlined, on a transparent square.
pub fn crosshair(color: [u8; 4]) -> Image {
    let size = CROSSHAIR_SIZE as i32;
    let centre = size / 2;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x - centre, y - centre);
            let outlined = (-1..=1).any(|oy| (-1..=1).any(|ox| crosshair_mark(dx + ox, dy + oy)));
            data.extend_from_slice(&if crosshair_mark(dx, dy) {
                color
            } else if outlined {
                CROSSHAIR_OUTLINE
            } else {
                [0; 4]
            });
        }
    }
    Image::new(
        Extent3d {
            width: CROSSHAIR_SIZE,
            height: CROSSHAIR_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// The whole-number scale for the game's cursor images: the HUD's scale
/// (window height over the art's 480 lines), rounded, at least 1.
pub fn cursor_factor(window_height: f32, canvas_height: f32) -> u32 {
    (window_height / canvas_height.max(1.0)).round().max(1.0) as u32
}

/// An image scaled up `factor` times, nearest-neighbour. `None` for an
/// image without CPU data or not four bytes a pixel.
pub fn upscale(image: &Image, factor: u32) -> Option<Image> {
    let data = image.data.as_ref()?;
    let format = image.texture_descriptor.format;
    if format.pixel_size().ok()? != 4 {
        return None;
    }
    let (width, height) = (image.width() as usize, image.height() as usize);
    let factor = factor.max(1) as usize;
    let mut out = Vec::with_capacity(data.len() * factor * factor);
    for y in 0..height * factor {
        let row = &data[(y / factor) * width * 4..(y / factor + 1) * width * 4];
        for x in 0..width * factor {
            out.extend_from_slice(&row[(x / factor) * 4..(x / factor) * 4 + 4]);
        }
    }
    Some(Image::new(
        Extent3d {
            width: (width * factor) as u32,
            height: (height * factor) as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        out,
        format,
        RenderAssetUsages::default(),
    ))
}

impl ScaledCursors {
    /// `art` scaled by the current factor, made once per factor.
    fn get(
        &mut self,
        name: &str,
        art: &HudArt,
        images: &mut Assets<Image>,
    ) -> Option<(Handle<Image>, (u16, u16))> {
        let size = art.size * self.factor as f32;
        if let Some(handle) = self.images.get(name) {
            return Some((handle.clone(), (size.x as u16, size.y as u16)));
        }
        let scaled = upscale(images.get(&art.image)?, self.factor)?;
        let handle = images.add(scaled);
        self.images.insert(name.to_string(), handle.clone());
        Some((handle, (size.x as u16, size.y as u16)))
    }
}

// Nine parameters: the window, what is on it, what it should show, and the
// three image sources.
#[allow(clippy::too_many_arguments)]
fn set_cursor(
    mut commands: Commands,
    crosshair: Option<Res<CrosshairImages>>,
    style: Res<HudStyle>,
    settings: Res<Settings>,
    mut scaled: ResMut<ScaledCursors>,
    mut images: ResMut<Assets<Image>>,
    mut applied: ResMut<Applied>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    players: Query<&AimResult, With<PlayerVehicle>>,
    gate: GameplayGate,
) {
    let Ok((window, window_size)) = windows.single() else {
        return;
    };
    let original = match &*style {
        HudStyle::Original(hud) => Some(hud),
        _ => None,
    };

    if let Some(hud) = original {
        let factor = cursor_factor(window_size.height(), hud.canvas().y);
        if scaled.factor != factor {
            scaled.factor = factor;
            scaled.images.clear();
        }
    }

    let aim = players.iter().next().copied().unwrap_or_default();
    let wanted = if gate.enabled() {
        // The game's reticle where it is wanted and its image is there; our
        // crosshair otherwise, so play never shows a bare arrow.
        original
            .filter(|_| uses_original(&style, &settings))
            .and_then(|hud| {
                let name = reticle_texture(&hud.layout, &aim);
                let (image, size) = scaled.get(name, hud.art.get(name)?, &mut images)?;
                Some(Look::Image {
                    image,
                    hotspot: (size.0 / 2, size.1 / 2),
                })
            })
            .or_else(|| {
                crosshair
                    .as_ref()
                    .map(|crosshair| crosshair.look(ReticleState::of(&aim)))
            })
    } else {
        // The arrow's tip is its top-left texel.
        original
            .and_then(|hud| hud.pointer.as_ref())
            .and_then(|art| scaled.get("\0pointer", art, &mut images))
            .map(|(image, _)| Look::Image {
                image,
                hotspot: (0, 0),
            })
    }
    .unwrap_or(Look::System);

    if applied.0.as_ref() == Some(&wanted) {
        return;
    }
    let icon = match &wanted {
        Look::System => CursorIcon::System(SystemCursorIcon::Default),
        Look::Image { image, hotspot } => {
            CursorIcon::Custom(CustomCursor::Image(CustomCursorImage {
                handle: image.clone(),
                hotspot: *hotspot,
                ..default()
            }))
        }
    };
    commands.entity(window).insert(icon);
    applied.0 = Some(wanted);
}

#[cfg(test)]
mod tests {
    use super::super::hud::original::OriginalHud;
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bft_sim::states::AppState;

    #[test]
    fn upscaling_repeats_each_texel() {
        let image = Image::new(
            Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![1, 2, 3, 4, 5, 6, 7, 8],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        let big = upscale(&image, 2).expect("four bytes a texel");
        assert_eq!(big.size(), UVec2::new(4, 2));
        let row = [1, 2, 3, 4, 1, 2, 3, 4, 5, 6, 7, 8, 5, 6, 7, 8];
        assert_eq!(big.data.as_deref(), Some([row, row].concat().as_slice()));
    }

    #[test]
    fn the_crosshair_is_marked_at_its_hotspot_and_clear_in_its_gap() {
        let image = crosshair(CROSSHAIR_ON);
        assert_eq!(image.size(), UVec2::splat(CROSSHAIR_SIZE));
        let texel = |x: u16, y: u16| {
            let i = (y as usize * CROSSHAIR_SIZE as usize + x as usize) * 4;
            image.data.as_ref().expect("CPU data")[i..i + 4].to_vec()
        };
        let (cx, cy) = CROSSHAIR_HOTSPOT;
        assert_eq!(texel(cx, cy), CROSSHAIR_ON);
        assert_eq!(texel(cx + 1, cy), CROSSHAIR_OUTLINE);
        assert_eq!(texel(cx + 2, cy), [0; 4]);
        assert_eq!(texel(cx + 8, cy), CROSSHAIR_ON);
        assert_eq!(texel(0, 0), [0; 4]);
    }

    #[test]
    fn the_reticle_is_off_in_range_or_on() {
        let mut aim = AimResult::default();
        assert_eq!(ReticleState::of(&aim), ReticleState::Off);
        aim.damageable = true;
        assert_eq!(
            ReticleState::of(&aim),
            ReticleState::Off,
            "no hit, no target"
        );
        aim.hit = Some(Entity::from_raw_u32(7).expect("a valid index"));
        assert_eq!(ReticleState::of(&aim), ReticleState::On);
        aim.damageable = false;
        assert_eq!(ReticleState::of(&aim), ReticleState::Range);
    }

    /// A world in play with a window, our crosshair, and `style`.
    fn in_play(style: HudStyle) -> World {
        let mut world = World::new();
        world.insert_resource(State::new(AppState::InGame));
        world.insert_resource(style);
        world.insert_resource(Settings::default());
        world.init_resource::<ScaledCursors>();
        world.init_resource::<Applied>();
        world.init_resource::<Assets<Image>>();
        world.spawn((Window::default(), PrimaryWindow));
        world.run_system_once(load_crosshair).expect("runs");
        world
    }

    fn cursor_in(world: &mut World, aim: AimResult) -> Look {
        world.spawn((aim, PlayerVehicle));
        world.run_system_once(set_cursor).expect("runs");
        world
            .resource::<Applied>()
            .0
            .clone()
            .expect("a look was applied")
    }

    #[test]
    fn the_crosshair_shows_each_state_under_the_simple_hud() {
        let target = AimResult {
            hit: Some(Entity::from_raw_u32(7).expect("a valid index")),
            damageable: true,
            ..default()
        };
        for (aim, state) in [
            (AimResult::default(), ReticleState::Off),
            (
                AimResult {
                    damageable: false,
                    ..target
                },
                ReticleState::Range,
            ),
            (target, ReticleState::On),
        ] {
            let mut world = in_play(HudStyle::Fallback);
            let expected = world.resource::<CrosshairImages>().look(state);
            assert_eq!(cursor_in(&mut world, aim), expected, "{state:?}");
        }
    }

    #[test]
    fn the_crosshair_stands_in_for_missing_reticle_art() {
        let hud = OriginalHud {
            layout: super::super::hud::original::tests::layout(),
            art: HashMap::default(),
            pointer: None,
        };
        let mut world = in_play(HudStyle::Original(Box::new(hud)));
        let expected = world.resource::<CrosshairImages>().look(ReticleState::Off);
        assert_eq!(cursor_in(&mut world, AimResult::default()), expected);
    }

    #[test]
    fn the_cursor_scale_follows_the_window_height() {
        assert_eq!(cursor_factor(480.0, 480.0), 1);
        assert_eq!(cursor_factor(1042.0, 480.0), 2);
        assert_eq!(cursor_factor(2160.0, 480.0), 5);
        assert_eq!(cursor_factor(300.0, 480.0), 1);
    }
}
