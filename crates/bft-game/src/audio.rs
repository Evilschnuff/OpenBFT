//! Sound: the original's effects, positional, on the simulation's events,
//! plus the user's music (FR-S1–S3).
//!
//! # How a sound gets played
//!
//! Anything that wants a sound writes a [`PlaySound`] naming it logically —
//! `snd_exp_hit1`, as the reader and the anim events do. [`play_sounds`]
//! resolves the name through `sounds.zrd` ([`SoundLibrary::table`]) to a WAV
//! in the bank and to how it plays: positional or not, looped, its `RANGE`
//! and `VOLUME`. It then spawns an entity with Bevy's `AudioPlayer`, either at
//! a world position or as a child of the entity the sound belongs to, so it
//! follows that entity and dies with it. One-shots despawn when they finish.
//!
//! [`bridges`] turns the sim's messages into `PlaySound`s; [`engine`] keeps a
//! looping engine on every vehicle near enough to hear; [`music`] plays the
//! user's music directory.
//!
//! # Loudness is ours, panning is rodio's
//!
//! `bevy_audio`'s spatial sink attenuates by the inverse square of distance,
//! with no per-sound range. The data gives every positional sound its own
//! `RANGE [min, max]` — full volume inside `min`, silent beyond `max` — so
//! each positional sound is played with its positions scaled by `1 / max`,
//! which keeps rodio's own attenuation at exactly 1 everywhere it is audible
//! and leaves it only the left/right panning. [`apply_gains`] then sets every
//! sink's volume each frame from the range, the settings, the mute flag and
//! whether the node it hangs off is visible. That is also how muting in the
//! settings silences sounds that are already playing.
//!
//! Only `bevy_audio`: the one gap it has (no per-sound range) is closed by
//! setting volumes ourselves, so `bevy_kira_audio` is not needed.

use std::collections::HashSet;

use bevy::audio::{SpatialScale, Volume};
use bevy::prelude::*;
use bft_assets::config::{AudioSettings, Config};
use bft_assets::reader::ReaderData;
use bft_assets::reader::sounds::{self, SoundDef, SoundTable, wav_stem};
use bft_assets::sounds::SoundBank;
use bft_sim::states::AppState;

use crate::globals::Globals;

pub mod bridges;
pub mod cues;
pub mod engine;
pub mod music;
pub mod tuning;

pub use engine::EngineSound;
pub use music::MusicPlayer;
pub use tuning::SoundTuning;

/// Where `assets/tuning/sounds.ron` lives.
pub const SOUNDS_TUNING: &str = "tuning/sounds.ron";

/// Sound effects, engines and music.
///
/// Not Bevy's `AudioPlugin`, which
/// `DefaultPlugins` adds and this one relies on.
pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        // The persisted settings, unless something (a test, a settings
        // screen restored earlier) already put some in.
        if !app.world().contains_resource::<AudioSettings>() {
            let settings = Config::load().map(|c| c.audio).unwrap_or_else(|e| {
                warn!("cannot read the config ({e}); using the default audio settings");
                AudioSettings::default()
            });
            app.insert_resource(settings);
        }

        app.init_asset::<SoundTuning>()
            .register_asset_loader(bft_assets::RonAssetLoader::<SoundTuning>::new(&[
                "sounds.ron",
            ]))
            .init_resource::<SoundTuning>()
            .init_resource::<SoundLibrary>()
            .add_message::<PlaySound>()
            .add_systems(Startup, request_tuning)
            .add_systems(OnExit(AppState::InGame), stop_loose_sounds)
            .add_systems(
                Update,
                (
                    apply_tuning,
                    build_library,
                    attach_listener,
                    play_sounds.in_set(AudioSystems::Play),
                    apply_gains.in_set(AudioSystems::Gain),
                )
                    .chain(),
            )
            .configure_sets(
                Update,
                (AudioSystems::Bridge, AudioSystems::Play, AudioSystems::Gain).chain(),
            )
            .add_plugins((
                bridges::BridgesPlugin,
                cues::CuesPlugin,
                engine::EnginePlugin,
                music::MusicPlugin,
            ));
    }
}

/// The order the audio systems run in within `Update`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum AudioSystems {
    /// Sim messages become [`PlaySound`]s.
    Bridge,
    /// [`PlaySound`]s become playing entities.
    Play,
    /// Every playing sink gets its volume.
    Gain,
}

/// The sound bank and the table that names what is in it.
#[derive(Resource, Debug, Default)]
pub struct SoundLibrary {
    /// `soundsh`'s WAVs. There is one bank: the three archives are the same
    /// sounds at three fidelities and only the high one is extracted
    /// (docs/data/sounds.md), so there is one bank and no quality setting.
    pub bank: Option<Handle<SoundBank>>,
    /// `sounds.zrd`: logical name → file and playback flags.
    pub table: SoundTable,
}

impl SoundLibrary {
    /// A logical name's record and its WAV stem; a name that is already a
    /// stem resolves to itself with no record.
    pub fn resolve<'a>(&'a self, name: &str) -> (Option<&'a SoundDef>, String) {
        match self.table.get(name) {
            Some(def) => (Some(def), def.file.clone()),
            None => (None, wav_stem(name)),
        }
    }
}

/// Play a sound.
///
/// A `Message`, as the sim's events are in this Bevy: buffered and read by
/// several systems.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct PlaySound {
    /// The logical name (`snd_exp_hit1`), or a bare WAV stem.
    pub name: String,
    /// Where: a world position, or with [`Self::attached`] an offset in that
    /// entity's space. `None` with no attachment plays at the listener.
    pub at: Option<Vec3>,
    /// The entity the sound follows and dies with.
    pub attached: Option<Entity>,
    /// Loop until the entity it is attached to goes. A looped sound with no
    /// attachment plays until the level ends.
    pub looped: bool,
    /// A gain on top of the record's `VOLUME` and the settings.
    pub volume: f32,
}

impl PlaySound {
    /// A one-shot at a world position.
    pub fn at(name: impl Into<String>, position: Vec3) -> PlaySound {
        PlaySound {
            name: name.into(),
            at: Some(position),
            attached: None,
            looped: false,
            volume: 1.0,
        }
    }

    /// A one-shot that follows an entity.
    pub fn on(name: impl Into<String>, entity: Entity) -> PlaySound {
        PlaySound {
            name: name.into(),
            at: None,
            attached: Some(entity),
            looped: false,
            volume: 1.0,
        }
    }

    /// A one-shot at the listener, not positioned in the world.
    pub fn ui(name: impl Into<String>) -> PlaySound {
        PlaySound {
            name: name.into(),
            at: None,
            attached: None,
            looped: false,
            volume: 1.0,
        }
    }

    /// The same request, looping.
    pub fn looping(mut self) -> PlaySound {
        self.looped = true;
        self
    }
}

/// What kind of volume setting a sound answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    Effect,
    Music,
}

/// A sound this plugin started and is managing.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct PlayingSound {
    /// The name it was asked for by.
    pub name: String,
    /// Request volume × the record's `VOLUME`. [`engine`] changes it with
    /// speed.
    pub gain: f32,
    /// `RANGE [min, max]`: `Some` for a positional sound.
    pub range: Option<(f32, f32)>,
    pub looped: bool,
    pub kind: SoundKind,
}

/// How loud a positional sound is at `distance`: 1 inside `min`, 0 beyond
/// `max`, linear between.
///
/// // approximation: `sounds.zrd` gives the two distances but not the curve;
/// linear is the reading that makes both of them mean something.
pub fn falloff((min, max): (f32, f32), distance: f32) -> f32 {
    if distance <= min {
        1.0
    } else if distance >= max {
        0.0
    } else {
        1.0 - (distance - min) / (max - min)
    }
}

/// A sound's volume this frame.
///
/// `distance` is to the listener, `None` when there is no listener;
/// `audible` is whether the node the sound hangs off is visible.
pub fn sound_gain(
    sound: &PlayingSound,
    settings: &AudioSettings,
    distance: Option<f32>,
    audible: bool,
) -> f32 {
    if !audible {
        return 0.0;
    }
    let setting = match sound.kind {
        SoundKind::Effect => settings.effects_gain(),
        SoundKind::Music => settings.music_gain(),
    };
    let falloff = match (sound.range, distance) {
        (Some(range), Some(distance)) => falloff(range, distance),
        _ => 1.0,
    };
    (sound.gain * setting * falloff).max(0.0)
}

/// Everything needed to spawn one resolved sound.
#[derive(Debug, Clone)]
pub struct ResolvedSound {
    pub source: Handle<AudioSource>,
    pub playing: PlayingSound,
    pub settings: PlaybackSettings,
}

/// Why a [`PlaySound`] plays nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unresolved {
    /// Neither a logical name nor a WAV stem in the bank.
    UnknownName,
    /// The bank has not loaded (or failed to).
    NoBank,
}

/// Resolve a request against the library, without touching the world.
///
/// Public so the bridges and the engine share one rule, and so a test can
/// check it without an audio device.
pub fn resolve_sound(
    request: &PlaySound,
    library: &SoundLibrary,
    bank: Option<&SoundBank>,
    tuning: &SoundTuning,
) -> Result<ResolvedSound, Unresolved> {
    let bank = bank.ok_or(Unresolved::NoBank)?;
    let (def, stem) = library.resolve(&request.name);
    let source = bank.get(&stem).cloned().ok_or(Unresolved::UnknownName)?;

    // Positional only when the record says `3D` *and* the request says
    // where. A bare stem with no record is positional whenever placed.
    let placed = request.at.is_some() || request.attached.is_some();
    let positional = placed && def.is_none_or(|def| def.positional);
    let range = positional.then(|| {
        def.and_then(|def| def.range)
            .unwrap_or(tuning.default_range)
    });

    let mut settings = if request.looped {
        PlaybackSettings::LOOP
    } else {
        PlaybackSettings::DESPAWN
    };
    settings.spatial = positional;
    if let Some((_, max)) = range {
        // Scale positions so `max` is 1: rodio's inverse-square rolloff is
        // then clamped to 1 everywhere the sound is in range, and only its
        // panning is left. See the module docs.
        settings.spatial_scale = Some(SpatialScale::new(1.0 / max.max(1.0)));
    }
    // Silent until `apply_gains` has placed it, so nothing blares at full
    // volume for the frame between spawn and the first gain.
    settings.volume = Volume::SILENT;

    Ok(ResolvedSound {
        source,
        playing: PlayingSound {
            name: request.name.clone(),
            gain: request.volume * def.map_or(1.0, |def| def.volume),
            range,
            looped: request.looped,
            kind: SoundKind::Effect,
        },
        settings,
    })
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// The tuning handle, held so it is not dropped.
#[derive(Resource, Debug, Clone)]
struct SoundTuningHandle(Handle<SoundTuning>);

fn request_tuning(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(SoundTuningHandle(server.load(SOUNDS_TUNING)));
}

/// Copy the tuning file into the resource when it loads or is edited.
fn apply_tuning(
    handle: Option<Res<SoundTuningHandle>>,
    mut events: MessageReader<AssetEvent<SoundTuning>>,
    assets: Res<Assets<SoundTuning>>,
    mut tuning: ResMut<SoundTuning>,
) {
    let Some(handle) = handle else {
        return;
    };
    for event in events.read() {
        if (event.is_loaded_with_dependencies(&handle.0) || event.is_modified(&handle.0))
            && let Some(loaded) = assets.get(&handle.0)
        {
            *tuning = loaded.clone();
        }
    }
}

/// Fill the library once boot's globals have loaded.
///
/// The bank is one of boot's globals ([`Globals::sounds`]); the table is
/// parsed from the global reader the first time it is available.
fn build_library(
    globals: Option<Res<Globals>>,
    readers: Res<Assets<ReaderData>>,
    mut library: ResMut<SoundLibrary>,
    mut failed: Local<bool>,
) {
    let Some(globals) = globals else {
        return;
    };
    if library.bank.is_none() {
        library.bank = globals.sounds.clone();
    }
    if !library.table.is_empty() || *failed {
        return;
    }
    let Some(reader) = globals.reader.as_ref().and_then(|h| readers.get(h)) else {
        return;
    };

    match sounds::parse(reader) {
        Ok(table) => {
            info!("{} sound names from sounds.zrd", table.len());
            library.table = table;
        }
        Err(e) => {
            // Not fatal: requests by bare WAV stem still play.
            error!("cannot read the sound table: {e}; logical sound names will be silent");
            *failed = true;
        }
    }
}

/// Hear from wherever the 3D camera is.
fn attach_listener(
    mut commands: Commands,
    cameras: Query<
        Entity,
        (
            Without<SpatialListener>,
            crate::ui::hud::sensor::MainCamera3d,
        ),
    >,
    tuning: Res<SoundTuning>,
) {
    for camera in &cameras {
        commands
            .entity(camera)
            .insert(SpatialListener::new(tuning.listener_gap));
    }
}

/// Turn requests into playing entities.
#[allow(clippy::too_many_arguments)]
pub fn play_sounds(
    mut commands: Commands,
    mut requests: MessageReader<PlaySound>,
    library: Res<SoundLibrary>,
    banks: Res<Assets<SoundBank>>,
    tuning: Res<SoundTuning>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    places: Query<&GlobalTransform>,
    playing: Query<(&PlayingSound, Option<&ChildOf>)>,
    mut warned: Local<HashSet<String>>,
) {
    let bank = library.bank.as_ref().and_then(|handle| banks.get(handle));
    let ear = listener.iter().next().map(GlobalTransform::translation);
    let mut voices = playing.iter().count();

    for request in requests.read() {
        let resolved = match resolve_sound(request, &library, bank, &tuning) {
            Ok(resolved) => resolved,
            Err(reason) => {
                let key = match reason {
                    Unresolved::NoBank => "<no bank>".to_string(),
                    Unresolved::UnknownName => request.name.clone(),
                };
                if warned.insert(key) {
                    match reason {
                        Unresolved::NoBank => {
                            warn!("the sound bank is not loaded; sounds are silent")
                        }
                        Unresolved::UnknownName => {
                            warn!("no sound named {:?}; it will be silent", request.name)
                        }
                    }
                }
                continue;
            }
        };

        // Attached to something already gone: nothing to follow.
        let parent = match request.attached {
            Some(entity) => match places.get(entity) {
                Ok(transform) => Some((entity, transform)),
                Err(_) => continue,
            },
            None => None,
        };

        // A loop already running on the same node is not started twice: a
        // `SoundNode` re-run by its definition would otherwise stack.
        if request.looped
            && let Some((entity, _)) = parent
            && playing.iter().any(|(sound, child_of)| {
                sound.name == request.name && child_of.is_some_and(|c| c.parent() == entity)
            })
        {
            continue;
        }

        if !request.looped {
            if voices >= tuning.max_voices {
                debug!("{} voices playing; dropping {}", voices, request.name);
                continue;
            }
            // Out of range now is out of range for all of a one-shot's life.
            let world = match parent {
                Some((_, transform)) => {
                    Some(transform.transform_point(request.at.unwrap_or(Vec3::ZERO)))
                }
                None => request.at,
            };
            if let (Some((_, max)), Some(ear), Some(world)) = (resolved.playing.range, ear, world)
                && ear.distance(world) >= max
            {
                continue;
            }
        }

        let mut entity = commands.spawn((
            Name::new(format!("sound {}", request.name)),
            AudioPlayer(resolved.source),
            resolved.settings,
            resolved.playing,
            Transform::from_translation(request.at.unwrap_or(Vec3::ZERO)),
        ));
        if let Some((parent, _)) = parent {
            entity.insert(ChildOf(parent));
        }
        voices += 1;
    }
}

/// A managed sound and whichever of Bevy's two sinks it has, once playing.
type SoundSinks<'a> = (
    &'a PlayingSound,
    &'a GlobalTransform,
    Option<&'a ChildOf>,
    Option<&'a mut AudioSink>,
    Option<&'a mut SpatialAudioSink>,
);

/// Set every sink's volume for this frame. See the module docs.
pub fn apply_gains(
    settings: Res<AudioSettings>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    mut sounds: Query<SoundSinks>,
    visible: Query<&InheritedVisibility>,
) {
    let ear = listener.iter().next().map(GlobalTransform::translation);

    for (sound, transform, child_of, sink, spatial) in &mut sounds {
        // A sound on a node the anim VM has switched off is silent, and
        // comes back if the node does.
        let audible = child_of
            .and_then(|c| visible.get(c.parent()).ok())
            .is_none_or(|v| v.get());
        let distance = ear.map(|ear| ear.distance(transform.translation()));
        let volume = Volume::Linear(sound_gain(sound, &settings, distance, audible));

        if let Some(mut sink) = sink {
            sink.set_volume(volume);
        }
        if let Some(mut sink) = spatial {
            sink.set_volume(volume);
        }
    }
}

/// Stop every effect sound that is not attached to anything.
///
/// Attached ones go with their entity; this catches the loose one-shots and
/// the world-placed loops when a level ends.
pub fn stop_loose_sounds(
    mut commands: Commands,
    sounds: Query<(Entity, &PlayingSound), Without<ChildOf>>,
) {
    for (entity, sound) in &sounds {
        if sound.kind == SoundKind::Effect {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::audio::PlaybackMode;
    use bft_assets::reader::sounds::SoundDef;

    fn def(name: &str, file: &str) -> SoundDef {
        SoundDef {
            name: name.to_string(),
            file: file.to_string(),
            set: "COMMON".to_string(),
            positional: true,
            looped: false,
            range: Some((60.0, 180.0)),
            volume: 0.75,
            frequency: false,
            voice: false,
            purgeable: false,
            hardware: false,
        }
    }

    fn library() -> (SoundLibrary, SoundBank) {
        let mut library = SoundLibrary::default();
        library
            .table
            .sounds
            .insert("snd_glass".into(), def("snd_glass", "glassbreak"));
        let mut ui = def("snd_click", "click");
        ui.positional = false;
        ui.range = None;
        library.table.sounds.insert("snd_click".into(), ui);

        let mut bank = SoundBank::default();
        bank.sounds.insert("glassbreak".into(), Handle::default());
        bank.sounds.insert("click".into(), Handle::default());
        bank.sounds.insert("bare".into(), Handle::default());
        (library, bank)
    }

    #[test]
    fn falloff_is_flat_inside_min_and_silent_past_max() {
        assert_eq!(falloff((60.0, 180.0), 10.0), 1.0);
        assert_eq!(falloff((60.0, 180.0), 60.0), 1.0);
        assert!((falloff((60.0, 180.0), 120.0) - 0.5).abs() < 1e-6);
        assert_eq!(falloff((60.0, 180.0), 180.0), 0.0);
        assert_eq!(falloff((60.0, 180.0), 1000.0), 0.0);
    }

    #[test]
    fn a_logical_name_resolves_with_its_record() {
        let (library, bank) = library();
        let resolved = resolve_sound(
            &PlaySound::at("snd_glass", Vec3::ZERO),
            &library,
            Some(&bank),
            &SoundTuning::default(),
        )
        .expect("resolves");

        assert!(resolved.settings.spatial);
        assert_eq!(resolved.playing.range, Some((60.0, 180.0)));
        assert_eq!(resolved.playing.gain, 0.75);
        assert!(matches!(resolved.settings.mode, PlaybackMode::Despawn));
        let scale = resolved.settings.spatial_scale.expect("scaled").0;
        assert!((scale.x - 1.0 / 180.0).abs() < 1e-6);
    }

    /// `3D` in the record is not enough: a request with nowhere to be plays
    /// at the listener, and a non-`3D` record plays there even when placed.
    #[test]
    fn only_a_placed_3d_sound_is_positional() {
        let (library, bank) = library();
        let tuning = SoundTuning::default();

        let unplaced = resolve_sound(&PlaySound::ui("snd_glass"), &library, Some(&bank), &tuning)
            .expect("resolves");
        assert!(!unplaced.settings.spatial);
        assert_eq!(unplaced.playing.range, None);

        let flat = resolve_sound(
            &PlaySound::at("snd_click", Vec3::ONE),
            &library,
            Some(&bank),
            &tuning,
        )
        .expect("resolves");
        assert!(!flat.settings.spatial);
    }

    #[test]
    fn a_bare_stem_plays_with_the_default_range() {
        let (library, bank) = library();
        let tuning = SoundTuning::default();
        let resolved = resolve_sound(
            &PlaySound::at("BARE", Vec3::ZERO).looping(),
            &library,
            Some(&bank),
            &tuning,
        )
        .expect("resolves");
        assert_eq!(resolved.playing.range, Some(tuning.default_range));
        assert!(matches!(resolved.settings.mode, PlaybackMode::Loop));
    }

    #[test]
    fn unknown_names_and_a_missing_bank_are_distinguished() {
        let (library, bank) = library();
        let tuning = SoundTuning::default();
        assert_eq!(
            resolve_sound(&PlaySound::ui("snd_nope"), &library, Some(&bank), &tuning).err(),
            Some(Unresolved::UnknownName)
        );
        assert_eq!(
            resolve_sound(&PlaySound::ui("snd_glass"), &library, None, &tuning).err(),
            Some(Unresolved::NoBank)
        );
    }

    #[test]
    fn muting_and_hiding_both_silence_a_sound() {
        let sound = PlayingSound {
            name: "x".into(),
            gain: 1.0,
            range: Some((10.0, 20.0)),
            looped: true,
            kind: SoundKind::Effect,
        };
        let mut settings = AudioSettings::default();
        assert_eq!(sound_gain(&sound, &settings, Some(5.0), true), 1.0);
        assert!((sound_gain(&sound, &settings, Some(15.0), true) - 0.5).abs() < 1e-6);
        assert_eq!(sound_gain(&sound, &settings, Some(5.0), false), 0.0);

        settings.muted = true;
        assert_eq!(sound_gain(&sound, &settings, Some(5.0), true), 0.0);
    }
}
