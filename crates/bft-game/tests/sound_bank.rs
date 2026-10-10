//! The sound bank, loaded through Bevy's asset server, and a
//! `PlaySound` carried through the real plugin to a playing entity.
//!
//! No game data: the archive is a ZIP built here around a 0.1 s tone this
//! test synthesises, the way `unzbd rc sounds` lays one out. Headless — no
//! audio device is opened, since Bevy's own `AudioPlugin` is not added; the
//! test checks what the audio plugin spawns, not what rodio does with it.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::asset::{AssetPlugin, LoadState};
use bevy::audio::AudioSource;
use bevy::prelude::*;
use bft_assets::config::AudioSettings;
use bft_assets::reader::sounds::{SoundDef, SoundTable};
use bft_assets::sounds::{SoundBank, SoundBankLoader, synth_wav, wav_info};
use bft_game::audio::{PlaySound, PlayingSound, SoundLibrary};

/// A directory under the system temp dir, removed when dropped.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(tag: &str) -> ScratchDir {
        let path = std::env::temp_dir().join(format!("open-bft-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create a scratch dir");
        ScratchDir(path)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `unzbd rc sounds`' layout: a manifest and one WAV per entry.
fn write_archive(dir: &std::path::Path) {
    let wav = synth_wav(22050, 2205); // 0.1 s
    let file = std::fs::File::create(dir.join("test.snd.zip")).expect("create");
    let mut zip = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.json", options).expect("start");
    zip.write_all(br#"[{"name":"Tone.wav"}]"#).expect("write");
    zip.start_file("Tone.wav", options).expect("start");
    zip.write_all(&wav).expect("write");
    zip.finish().expect("finish");
}

fn app_on(dir: &std::path::Path) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: dir.to_string_lossy().to_string(),
            ..default()
        },
    ));
    app.init_asset::<AudioSource>()
        .init_asset::<SoundBank>()
        .register_asset_loader(SoundBankLoader);
    app
}

fn load_bank(app: &mut App) -> Handle<SoundBank> {
    let handle: Handle<SoundBank> = app.world().resource::<AssetServer>().load("test.snd.zip");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.update();
        match app.world().resource::<AssetServer>().load_state(&handle) {
            LoadState::Loaded => break,
            LoadState::Failed(e) => panic!("the bank failed to load: {e}"),
            _ if Instant::now() > deadline => panic!("the bank never loaded"),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    handle
}

#[test]
fn a_hand_made_archive_loads_as_a_bank_of_playable_sources() {
    let dir = ScratchDir::new("sound-bank");
    write_archive(&dir.0);
    let mut app = app_on(&dir.0);

    let handle = load_bank(&mut app);

    let banks = app.world().resource::<Assets<SoundBank>>();
    let bank = banks.get(&handle).expect("loaded");
    assert_eq!(bank.sounds.len(), 1);
    let tone = bank
        .get("TONE")
        .expect("keyed by lower-cased stem, found in any case");

    let sources = app.world().resource::<Assets<AudioSource>>();
    let source = sources.get(tone).expect("the labelled sub-asset is loaded");
    let info = wav_info(&source.bytes).expect("still a PCM WAV");
    assert_eq!(info.sample_rate, 22050);
}

#[test]
fn a_play_sound_becomes_a_positional_entity_with_its_range() {
    let dir = ScratchDir::new("sound-play");
    write_archive(&dir.0);
    let mut app = app_on(&dir.0);
    app.insert_resource(AudioSettings::default());
    // `AssetsPlugin` registers this in a real run; the library reads it.
    app.init_asset::<bft_assets::reader::ReaderData>();
    app.add_plugins(bevy::state::app::StatesPlugin)
        .init_state::<bft_sim::states::AppState>()
        .add_plugins(bft_game::audio::AudioPlugin);

    let bank = load_bank(&mut app);
    let mut table = SoundTable::default();
    table.sounds.insert(
        "snd_tone".into(),
        SoundDef {
            name: "snd_tone".into(),
            file: "tone".into(),
            set: "COMMON".into(),
            positional: true,
            looped: false,
            range: Some((10.0, 40.0)),
            volume: 0.5,
            frequency: false,
            voice: false,
            purgeable: false,
            hardware: false,
        },
    );
    app.insert_resource(SoundLibrary {
        bank: Some(bank),
        table,
    });

    app.world_mut()
        .write_message(PlaySound::at("snd_tone", Vec3::new(1.0, 2.0, 3.0)));
    app.world_mut().write_message(PlaySound::ui("snd_missing"));
    app.update();

    let mut query = app
        .world_mut()
        .query::<(&PlayingSound, &PlaybackSettings, &Transform, &AudioPlayer)>();
    let playing: Vec<_> = query.iter(app.world()).collect();
    assert_eq!(playing.len(), 1, "the unknown name plays nothing");

    let (sound, settings, transform, _) = playing[0];
    assert_eq!(sound.range, Some((10.0, 40.0)));
    assert_eq!(sound.gain, 0.5);
    assert!(settings.spatial);
    assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
}
