//! The persisted settings and applying them (FR-U3, DESIGN §9).
//!
//! # One file, two readers
//!
//! `config.ron` is read twice. `bft_assets::config::Config` reads the three
//! path keys and the audio table before the `App` exists — `bft-assets` has
//! no input or window types to deserialise the rest with, and the paths have
//! to be known before `AssetsPlugin` is built. [`Settings`] reads the whole
//! file: the same keys with the same types, plus video, input, the player's
//! name and the texture filter. Both ignore keys they do not know, so the
//! file stays one file and each reader sees its part. Saving always goes through [`Settings`], which carries the
//! path keys along so a save never drops them.
//!
//! # Where each setting takes effect
//!
//! [`Settings`] is the source of truth; the resources the rest of the game
//! reads are copies of its parts. [`sync_settings`] pushes a change out:
//! [`InputBindings`] for the input map, [`AudioSettings`] for the audio,
//! the primary [`Window`] for video, [`TextureFilter`] for the level
//! textures. Then it saves.

use std::path::{Path, PathBuf};

use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, VideoModeSelection, WindowMode};
use bft_assets::config::{AudioSettings, Config, ConfigError};
use bft_assets::{TextureFilter, TextureSet};
use serde::{Deserialize, Serialize};

use super::bindings::InputBindings;

/// Everything the player can set, as `config.ron` stores it.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub video: VideoSettings,
    /// The audio table, the same type `Config` reads.
    pub audio: AudioSettings,
    pub input: InputBindings,
    /// The name other players see.
    pub player_name: String,
    /// The data and cache path keys, read-only here: they are resolved before the
    /// `App` exists, so a change takes effect on the next start.
    pub data_root: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub unzbd: Option<PathBuf>,
    /// Linear (the default) or nearest-neighbour texture sampling (FR-W3).
    pub texture_filter_linear: bool,
    /// The in-game HUD.
    pub hud: HudSettings,
    /// The menus.
    pub menus: MenuSettings,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            video: VideoSettings::default(),
            audio: AudioSettings::default(),
            input: InputBindings::default(),
            player_name: "Player".to_string(),
            data_root: None,
            cache_dir: None,
            unzbd: None,
            texture_filter_linear: true,
            hud: HudSettings::default(),
            menus: MenuSettings::default(),
        }
    }
}

impl Settings {
    /// The texture filter this asks for.
    pub fn texture_filter(&self) -> TextureFilter {
        TextureFilter(if self.texture_filter_linear {
            ImageFilterMode::Linear
        } else {
            ImageFilterMode::Nearest
        })
    }
}

/// The in-game HUD's options.
///
/// `hud.zrd` lays out both the view without the strips (`TYPEI`) and the
/// view between them (`TYPEII`), and `detail.zrd` carries
/// `HUD_TYPEI`/`HUD_TYPEII` defaults, so `strips` chooses between them. The
/// clock toggle is this engine's own option.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct HudSettings {
    /// The original's art and layout, read from the install; off for
    /// the fallback HUD, which is also what shows without an install.
    pub original: bool,
    /// Draw the top and bottom strips (`TYPEII`) rather than floating
    /// panels over the whole view (`TYPEI`).
    pub strips: bool,
    /// A multiplier on the HUD's size, which otherwise keeps the share of
    /// the screen height it had at 640×480.
    pub scale: f32,
    /// Show the mission clock.
    pub clock: bool,
}

impl Default for HudSettings {
    fn default() -> HudSettings {
        HudSettings {
            original: true,
            strips: true,
            scale: 1.0,
            clock: true,
        }
    }
}

/// The menus' options.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct MenuSettings {
    /// The original's dialogs, read from the install; off for the simple
    /// menus, which are also what shows without an install.
    pub original: bool,
}

impl Default for MenuSettings {
    fn default() -> MenuSettings {
        MenuSettings { original: true }
    }
}

/// The range [`HudSettings::scale`] is kept in.
pub const HUD_SCALE_RANGE: (f32, f32) = (0.5, 1.5);

/// The window.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct VideoSettings {
    pub window_mode: WindowModeSetting,
    pub vsync: bool,
    /// Logical size of the window when windowed. Ignored in the fullscreen
    /// modes, which take the monitor's.
    pub resolution: (u32, u32),
}

impl Default for VideoSettings {
    /// Bevy's own window defaults, so a first run looks like it did before
    /// there were settings.
    fn default() -> VideoSettings {
        VideoSettings {
            window_mode: WindowModeSetting::Windowed,
            vsync: true,
            resolution: (1280, 720),
        }
    }
}

/// How the window occupies the screen.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowModeSetting {
    Windowed,
    /// A borderless window the size of the monitor: fast to switch, and what
    /// macOS calls fullscreen.
    Borderless,
    /// Exclusive fullscreen, at the monitor's current video mode.
    Fullscreen,
}

impl WindowModeSetting {
    /// The next mode, for a button that cycles them.
    pub fn next(self) -> WindowModeSetting {
        match self {
            WindowModeSetting::Windowed => WindowModeSetting::Borderless,
            WindowModeSetting::Borderless => WindowModeSetting::Fullscreen,
            WindowModeSetting::Fullscreen => WindowModeSetting::Windowed,
        }
    }

    /// What the settings screen calls it.
    pub fn label(self) -> &'static str {
        match self {
            WindowModeSetting::Windowed => "Windowed",
            WindowModeSetting::Borderless => "Borderless",
            WindowModeSetting::Fullscreen => "Fullscreen",
        }
    }
}

/// The resolutions the settings screen cycles through when windowed.
///
/// A fixed list rather than the monitor's modes: in a window any size is
/// valid, and these are the common 16:9 and 16:10 steps. A size typed into
/// `config.ron` by hand is kept and shown; the button moves on from it.
pub const RESOLUTIONS: [(u32, u32); 7] = [
    (1024, 768),
    (1280, 720),
    (1280, 800),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
];

/// The resolution after `current` in [`RESOLUTIONS`], wrapping.
pub fn next_resolution(current: (u32, u32)) -> (u32, u32) {
    let index = RESOLUTIONS
        .iter()
        .position(|resolution| *resolution == current)
        .map(|index| (index + 1) % RESOLUTIONS.len())
        .unwrap_or(0);
    RESOLUTIONS[index]
}

impl VideoSettings {
    /// Write these settings onto a window.
    ///
    /// `bft-bin` calls it on the window it creates, so the first frame is
    /// already the right size and mode, and [`sync_settings`] calls it again
    /// when the player changes something.
    pub fn apply_to(&self, window: &mut Window) {
        window.mode = match self.window_mode {
            WindowModeSetting::Windowed => WindowMode::Windowed,
            WindowModeSetting::Borderless => {
                WindowMode::BorderlessFullscreen(MonitorSelection::Current)
            }
            WindowModeSetting::Fullscreen => {
                WindowMode::Fullscreen(MonitorSelection::Current, VideoModeSelection::Current)
            }
        };
        window.present_mode = if self.vsync {
            PresentMode::AutoVsync
        } else {
            PresentMode::AutoNoVsync
        };
        if self.window_mode == WindowModeSetting::Windowed {
            let (width, height) = self.resolution;
            // Only when it differs: `set` marks the window changed, and a
            // resize request to the size it already has still costs a
            // swapchain rebuild on some platforms.
            if window.resolution.width() as u32 != width
                || window.resolution.height() as u32 != height
            {
                window.resolution.set(width as f32, height as f32);
            }
        }
    }
}

impl VideoSettings {
    /// Write onto a window only what differs from `before`, so switching
    /// vsync does not also re-request a size the OS has clamped.
    pub fn apply_changes(&self, before: &VideoSettings, window: &mut Window) {
        if self.window_mode != before.window_mode || self.resolution != before.resolution {
            self.apply_to(window);
            return;
        }
        if self.vsync != before.vsync {
            window.present_mode = if self.vsync {
                PresentMode::AutoVsync
            } else {
                PresentMode::AutoNoVsync
            };
        }
    }
}

/// Where the settings are read from and written to.
///
/// `None` keeps them in memory: the unit tests, and a platform with no
/// config directory. The default is `config.ron` in the platform config
/// directory, the file `Config` reads.
#[derive(Resource, Debug, Clone)]
pub struct SettingsFile(pub Option<PathBuf>);

impl Default for SettingsFile {
    fn default() -> SettingsFile {
        SettingsFile(Config::default_path())
    }
}

/// Read the settings from `config.ron` in the platform config directory.
///
/// Never fails: a missing file is a first run and gets the defaults, and a
/// broken one is reported and replaced by the defaults rather than stopping
/// the game from starting.
pub fn load_settings() -> Settings {
    match Config::default_path() {
        Some(path) => load_settings_or_default(&path),
        None => Settings::default(),
    }
}

/// Write the settings to `config.ron` in the platform config directory,
/// reporting rather than returning a failure.
pub fn save_settings(settings: &Settings) {
    match Config::default_path() {
        Some(path) => {
            if let Err(e) = save_settings_to(settings, &path) {
                warn!("cannot save the settings: {e}");
            }
        }
        None => warn!("no config directory on this platform; the settings are not saved"),
    }
}

/// [`load_settings`] from a specific file.
pub fn load_settings_or_default(path: &Path) -> Settings {
    match load_settings_from(path) {
        Ok(settings) => settings,
        Err(e) => {
            warn!("cannot read the settings ({e}); using the defaults");
            Settings::default()
        }
    }
}

/// Read settings from a file. A missing file is the defaults.
pub fn load_settings_from(path: &Path) -> Result<Settings, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut settings: Settings = ron::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    settings.input.fill_missing_defaults();
    Ok(settings)
}

/// Write settings to a file, creating its directory.
pub fn save_settings_to(settings: &Settings, path: &Path) -> Result<(), ConfigError> {
    let text = ron::ser::to_string_pretty(settings, ron::ser::PrettyConfig::default())
        .map_err(Box::new)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    std::fs::write(path, text).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Push a change to the settings out to everything that reads a part of
/// them, then save.
///
/// Only on a real change after the first frame: the resources were seeded
/// from the same values when the plugin was built, and saving an unchanged
/// file at every start would be a write for nothing.
// Six parameters because this is where every consumer of the settings meets;
// one system keeps "changed" and "saved" in the same frame.
#[allow(clippy::too_many_arguments)]
///
/// The window is touched only when the video settings themselves changed.
/// Re-applying them on every change (a HUD toggle, a volume step) asked the
/// OS for the configured size again even when it had clamped the window
/// smaller — a 2560×1440 request on a 1710×1042 screen — and resizing the
/// swapchain in the middle of a frame left the chase camera's depth buffer
/// the old size, which wgpu rejects and Bevy quits on.
pub fn sync_settings(
    settings: Res<Settings>,
    file: Res<SettingsFile>,
    mut bindings: ResMut<InputBindings>,
    mut audio: Option<ResMut<AudioSettings>>,
    mut filter: Option<ResMut<TextureFilter>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut applied_video: Local<Option<VideoSettings>>,
) {
    if settings.is_added() {
        *applied_video = Some(settings.video.clone());
    }
    if !settings.is_changed() || settings.is_added() {
        return;
    }

    bindings.set_if_neq(settings.input.clone());
    if let Some(audio) = audio.as_mut() {
        audio.set_if_neq(settings.audio.clone());
    }
    if let Some(filter) = filter.as_mut() {
        filter.set_if_neq(settings.texture_filter());
    }
    if applied_video.as_ref() != Some(&settings.video) {
        for mut window in &mut windows {
            match applied_video.as_ref() {
                Some(before) => settings.video.apply_changes(before, &mut window),
                None => settings.video.apply_to(&mut window),
            }
        }
        *applied_video = Some(settings.video.clone());
    }

    if let Some(path) = &file.0
        && let Err(e) = save_settings_to(&settings, path)
    {
        warn!("cannot save the settings: {e}");
    }
}

/// Re-sample the level textures when the filter changes, and fix up any
/// archive that loads afterwards.
///
/// The texture loader bakes the filter in when it is registered, because
/// an `AssetLoader` cannot read the world, which suggests a runtime change
/// would mean re-registering and reloading. It does not: an [`Image`]'s
/// sampler is plain data, so rewriting it on the loaded images re-uploads
/// them with the new filter, and an archive loaded later — by a loader still
/// holding the old filter — is patched the same way as it arrives.
pub fn apply_texture_filter(
    filter: Option<Res<TextureFilter>>,
    mut events: MessageReader<AssetEvent<TextureSet>>,
    sets: Res<Assets<TextureSet>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(filter) = filter else {
        return;
    };

    let targets: Vec<AssetId<TextureSet>> = if filter.is_changed() && !filter.is_added() {
        events.clear();
        sets.ids().collect()
    } else {
        events
            .read()
            .filter_map(|event| match event {
                AssetEvent::LoadedWithDependencies { id } | AssetEvent::Added { id } => Some(*id),
                _ => None,
            })
            .collect()
    };

    for id in targets {
        let Some(set) = sets.get(id) else {
            continue;
        };
        for handle in set.images.values() {
            // Read first, write only on a mismatch: `get_mut` marks the image
            // changed, and that re-uploads it.
            if images
                .get(handle)
                .is_some_and(|image| !samples_with(image, filter.0))
                && let Some(mut image) = images.get_mut(handle)
            {
                set_filter(&mut image, filter.0);
            }
        }
    }
}

/// Whether an image already samples with this filter.
fn samples_with(image: &Image, filter: ImageFilterMode) -> bool {
    match &image.sampler {
        ImageSampler::Descriptor(descriptor) => {
            descriptor.mag_filter == filter && descriptor.min_filter == filter
        }
        ImageSampler::Default => false,
    }
}

/// Change an image's filter, keeping its address modes.
fn set_filter(image: &mut Image, filter: ImageFilterMode) {
    let mut descriptor = match &image.sampler {
        ImageSampler::Descriptor(descriptor) => descriptor.clone(),
        ImageSampler::Default => ImageSamplerDescriptor::default(),
    };
    descriptor.mag_filter = filter;
    descriptor.min_filter = filter;
    descriptor.mipmap_filter = filter;
    image.sampler = ImageSampler::Descriptor(descriptor);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::bindings::{Action, Binding};

    /// A directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "bft-ui-settings-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = Scratch::new();
        let path = dir.path().join("config.ron");
        let mut settings = Settings {
            player_name: "Tester".into(),
            data_root: Some(PathBuf::from("/games/recoil")),
            texture_filter_linear: false,
            ..Settings::default()
        };
        settings.video.vsync = false;
        settings.video.window_mode = WindowModeSetting::Borderless;
        settings
            .input
            .rebind(Action::NextForm, Binding::Key(KeyCode::KeyG));

        save_settings_to(&settings, &path).unwrap();

        assert_eq!(load_settings_from(&path).unwrap(), settings);
    }

    /// The same file feeds `bft_assets::config::Config` before the `App`
    /// exists, so a file written by the settings screen has to read back
    /// there with its paths and audio intact — and without tripping on the
    /// input and video tables it has no types for.
    #[test]
    fn a_settings_file_still_reads_as_the_asset_config() {
        let dir = Scratch::new();
        let path = dir.path().join("config.ron");
        let mut settings = Settings {
            data_root: Some(PathBuf::from("/games/recoil")),
            unzbd: Some(PathBuf::from("/bin/unzbd")),
            ..Settings::default()
        };
        settings.audio.music = 0.25;

        save_settings_to(&settings, &path).unwrap();
        let config = Config::load_from(&path).unwrap();

        assert_eq!(config.data_root, settings.data_root);
        assert_eq!(config.unzbd, settings.unzbd);
        assert_eq!(config.audio, settings.audio);
    }

    /// And the reverse: a file `Config` wrote, with only its keys,
    /// loads as settings with everything else at its default.
    #[test]
    fn an_older_config_loads_with_defaults_for_the_rest() {
        let dir = Scratch::new();
        let path = dir.path().join("config.ron");
        std::fs::write(
            &path,
            r#"(data_root: Some("/games/recoil"), audio: (music: 0.3))"#,
        )
        .unwrap();

        let settings = load_settings_from(&path).unwrap();

        assert_eq!(settings.data_root, Some(PathBuf::from("/games/recoil")));
        assert!((settings.audio.music - 0.3).abs() < 1e-6);
        assert_eq!(settings.input, InputBindings::default());
        assert_eq!(settings.video, VideoSettings::default());
    }

    #[test]
    fn a_broken_file_falls_back_to_the_defaults() {
        let dir = Scratch::new();
        let path = dir.path().join("config.ron");
        std::fs::write(&path, "(video: (vsync: maybe))").unwrap();

        assert!(load_settings_from(&path).is_err());
        assert_eq!(load_settings_or_default(&path), Settings::default());
    }

    #[test]
    fn the_resolution_button_cycles_and_recovers_from_a_custom_size() {
        assert_eq!(next_resolution((1280, 720)), (1280, 800));
        assert_eq!(next_resolution((3840, 2160)), (1024, 768));
        assert_eq!(next_resolution((1111, 999)), RESOLUTIONS[0]);
    }

    #[test]
    fn video_settings_reach_the_window() {
        let mut window = Window::default();
        let video = VideoSettings {
            window_mode: WindowModeSetting::Windowed,
            vsync: false,
            resolution: (1600, 900),
        };

        video.apply_to(&mut window);

        assert_eq!(window.present_mode, PresentMode::AutoNoVsync);
        assert_eq!(window.mode, WindowMode::Windowed);
        assert_eq!(window.resolution.width() as u32, 1600);
        assert_eq!(window.resolution.height() as u32, 900);
    }

    /// A settings change reaches the resources the game reads and the file,
    /// in the same frame.
    #[test]
    fn a_change_is_pushed_out_and_saved() {
        let dir = Scratch::new();
        let path = dir.path().join("config.ron");

        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .insert_resource(Settings::default())
            .insert_resource(SettingsFile(Some(path.clone())))
            .insert_resource(InputBindings::default())
            .insert_resource(AudioSettings::default())
            .insert_resource(TextureFilter::default())
            .add_systems(Update, sync_settings);
        app.update();
        assert!(!path.exists(), "an unchanged start must not write the file");

        {
            let mut settings = app.world_mut().resource_mut::<Settings>();
            settings
                .input
                .rebind(Action::Fire, Binding::Key(KeyCode::KeyK));
            settings.audio.muted = true;
            settings.texture_filter_linear = false;
        }
        app.update();

        let world = app.world();
        assert_eq!(
            world.resource::<InputBindings>().get(Action::Fire)[0],
            Binding::Key(KeyCode::KeyK)
        );
        assert!(world.resource::<AudioSettings>().muted);
        assert_eq!(
            world.resource::<TextureFilter>().0,
            ImageFilterMode::Nearest
        );
        // A rebinding survives a restart, which is the file.
        assert_eq!(
            load_settings_from(&path).unwrap().input.get(Action::Fire)[0],
            Binding::Key(KeyCode::KeyK)
        );
    }

    /// A change that is not video leaves the window alone: re-requesting the
    /// configured size of a window the OS had clamped resized the swapchain
    /// mid-frame and crashed the renderer (toggling HUD strips in game).
    #[test]
    fn only_a_video_change_touches_the_window() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .insert_resource(Settings::default())
            .insert_resource(SettingsFile(None))
            .insert_resource(InputBindings::default())
            .add_systems(Update, sync_settings);
        // The OS gave the window less than the settings asked for.
        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: bevy::window::WindowResolution::new(800, 600),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        app.update();

        app.world_mut().resource_mut::<Settings>().hud.strips = false;
        app.update();
        let size = |app: &App| {
            app.world()
                .get::<Window>(window)
                .map(|w| w.resolution.width())
        };
        assert_eq!(size(&app), Some(800.0), "a HUD toggle must not resize");

        app.world_mut().resource_mut::<Settings>().video.vsync = false;
        app.update();
        let applied = app.world().get::<Window>(window).map(|w| w.present_mode);
        assert_eq!(applied, Some(PresentMode::AutoNoVsync));
        assert_eq!(size(&app), Some(800.0), "nor a vsync switch");

        app.world_mut().resource_mut::<Settings>().video.resolution = (1024, 768);
        app.update();
        assert_eq!(size(&app), Some(1024.0), "a new resolution does");
    }

    #[test]
    fn a_filter_change_resamples_loaded_textures() {
        let mut app = App::new();
        app.add_plugins((
            bevy::MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::image::ImagePlugin::default(),
        ))
        .init_asset::<TextureSet>()
        .insert_resource(TextureFilter::default())
        .add_systems(Update, apply_texture_filter);

        let image = Image {
            sampler: ImageSampler::Descriptor(ImageSamplerDescriptor {
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                address_mode_u: bevy::image::ImageAddressMode::Repeat,
                ..default()
            }),
            ..default()
        };
        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let mut set = TextureSet::default();
        set.images.insert("tex".into(), handle.clone());
        // Held: an asset whose last strong handle drops is freed.
        let _set = app
            .world_mut()
            .resource_mut::<Assets<TextureSet>>()
            .add(set);
        app.update();

        app.world_mut().resource_mut::<TextureFilter>().0 = ImageFilterMode::Nearest;
        app.update();

        let images = app.world().resource::<Assets<Image>>();
        let ImageSampler::Descriptor(descriptor) = &images.get(&handle).unwrap().sampler else {
            panic!("a descriptor sampler");
        };
        assert_eq!(descriptor.mag_filter, ImageFilterMode::Nearest);
        assert_eq!(
            descriptor.address_mode_u,
            bevy::image::ImageAddressMode::Repeat,
            "the address modes must survive"
        );
    }
}
