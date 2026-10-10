//! Music from a directory the user names (FR-S3), or found next to the
//! game.
//!
//! The original streamed four CD-audio tracks from the disc. They are not
//! in the install and cannot be extracted like the rest of the data; the
//! user rips them (or brings any other music) and points
//! [`AudioSettings::music_dir`] at the result. Without that setting the
//! game looks close to the game data ([`find_music_dir`]): a `music`
//! folder in the install, or in a Recoil folder beside it — where a rip of
//! the CD naturally ends up. In game the tracks play
//! shuffled, one after another; in the menus a file whose stem is
//! the first of [`SoundTuning::menu_tracks`] there loops (a rip of the CD
//! has `track02`, the original's menu music), and nothing plays when there is none.
//!
//! # Reading and checking
//!
//! A ripped CD track is ~50 MB of WAV, so files are read on the IO task pool
//! rather than in a system. Their first bytes are then checked against the
//! formats this build decodes, because `bevy_audio`'s decoder panics on data
//! it cannot read (see [`bft_assets::sounds`]) — a stray `.ogg` holding Opus
//! would otherwise take the audio thread down mid-game. A file that fails
//! either step is skipped for the rest of the run.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bevy::audio::Volume;
use bevy::prelude::*;
use bevy::tasks::{IoTaskPool, Task, block_on, futures_lite::future};
use bft_assets::config::AudioSettings;
use bft_assets::sounds::wav_info;
use bft_sim::states::AppState;
use rand::seq::SliceRandom;

use super::{AudioSystems, PlayingSound, SoundKind, SoundTuning};

/// The extensions a music directory is scanned for.
pub const MUSIC_EXTENSIONS: [&str; 4] = ["ogg", "mp3", "wav", "flac"];

/// The user's music and where playback is.
#[derive(Resource, Debug, Default)]
pub struct MusicPlayer {
    /// The directory scanned, from the settings.
    pub dir: Option<PathBuf>,
    /// Every playable-looking file in it, sorted by name.
    pub tracks: Vec<PathBuf>,
    /// The track playing or loading, as an index into [`Self::tracks`].
    pub current: Option<usize>,
    /// The menu track, if the directory has one.
    pub menu: Option<usize>,
    /// In-game tracks still to play before the next shuffle.
    queue: Vec<usize>,
    /// Tracks that would not read or decode.
    failed: HashSet<usize>,
    /// The file being read.
    loading: Option<Task<std::io::Result<Vec<u8>>>>,
}

/// The entity playing music.
#[derive(Component, Debug, Clone, Copy)]
pub struct MusicTrack;

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MusicPlayer>()
            .add_systems(Update, rescan.before(AudioSystems::Play))
            .add_systems(
                Update,
                drive_music
                    .after(rescan)
                    .before(AudioSystems::Gain)
                    .run_if(in_state(AppState::FrontEnd).or_else(in_state(AppState::InGame))),
            )
            // The menu loop and the game shuffle are different music, so
            // crossing between them starts over.
            .add_systems(OnEnter(AppState::FrontEnd), stop_music)
            .add_systems(OnEnter(AppState::InGame), stop_music)
            .add_systems(OnExit(AppState::FrontEnd), stop_music)
            .add_systems(OnExit(AppState::InGame), stop_music);
    }
}

/// Every music file in `dir`, sorted, and which one is the menu track.
///
/// Not recursive: a ripped album is one flat directory, and descending into
/// a whole music library would take the shuffle somewhere unexpected.
pub fn scan_music_dir(dir: &Path, menu_stems: &[String]) -> (Vec<PathBuf>, Option<usize>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (Vec::new(), None);
    };

    let mut tracks: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| MUSIC_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    tracks.sort();

    let menu = menu_stems.iter().find_map(|stem| {
        tracks.iter().position(|path| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(stem))
        })
    });

    (tracks, menu)
}

/// Whether these bytes are a format this build decodes.
///
/// WAV must be integer PCM ([`wav_info`]); Ogg must carry Vorbis, since Ogg
/// Opus would get past the extension check and not past the decoder.
pub fn is_playable_music(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(64)];
    if bytes.starts_with(b"RIFF") {
        return wav_info(bytes).is_some();
    }
    if bytes.starts_with(b"OggS") {
        return head.windows(7).any(|w| w == b"\x01vorbis");
    }
    if bytes.starts_with(b"fLaC") || bytes.starts_with(b"ID3") {
        return true;
    }
    // A bare MPEG audio frame: 11 sync bits.
    bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0
}

/// Music close to the game data at `root`, when the settings name none:
/// the install's own `music` folder, else one in a sibling folder whose
/// name says Recoil (`Recoil CD/music` beside `Recoil`). Only a
/// folder with at least one music file counts.
pub fn find_music_dir(root: &Path) -> Option<PathBuf> {
    let music_in = |dir: &Path| -> Option<PathBuf> {
        std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("music"))
                    && !scan_music_dir(path, &[]).0.is_empty()
            })
    };
    if let Some(found) = music_in(root) {
        return Some(found);
    }
    let parent = root.parent()?;
    let mut siblings: Vec<PathBuf> = std::fs::read_dir(parent)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path != root)
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().contains("recoil"))
        })
        .collect();
    siblings.sort();
    siblings.iter().find_map(|sibling| music_in(sibling))
}

/// Pick up the settings' music directory, and a change to it.
// The settings, the tuning and the data paths it looks near, the player it
// fills, and what it remembers between frames.
#[allow(clippy::too_many_arguments)]
fn rescan(
    settings: Res<AudioSettings>,
    tuning: Res<SoundTuning>,
    paths: Option<Res<bft_assets::DataPaths>>,
    mut player: ResMut<MusicPlayer>,
    mut commands: Commands,
    playing: Query<Entity, With<MusicTrack>>,
    mut scanned: Local<bool>,
    mut found: Local<Option<Option<PathBuf>>>,
) {
    // The settings' folder, or the one found beside the game (looked for
    // once).
    let dir = settings.music_dir.clone().or_else(|| {
        found
            .get_or_insert_with(|| {
                paths
                    .as_ref()
                    .and_then(|paths| find_music_dir(&paths.root.path))
            })
            .clone()
    });
    if *scanned && player.dir == dir {
        return;
    }
    *scanned = true;

    let (tracks, menu) = match &dir {
        Some(dir) => scan_music_dir(dir, &tuning.menu_tracks),
        None => (Vec::new(), None),
    };
    if let Some(dir) = &dir {
        info!(
            "{} music track(s) in {}{}",
            tracks.len(),
            dir.display(),
            if menu.is_some() {
                ", with a menu track"
            } else {
                ""
            }
        );
    }

    *player = MusicPlayer {
        dir,
        tracks,
        menu,
        ..default()
    };
    for entity in &playing {
        commands.entity(entity).despawn();
    }
}

/// Keep something playing: start the next track when the last one ends.
#[allow(clippy::too_many_arguments)]
fn drive_music(
    mut commands: Commands,
    mut player: ResMut<MusicPlayer>,
    state: Res<State<AppState>>,
    mut sources: ResMut<Assets<AudioSource>>,
    playing: Query<(), With<MusicTrack>>,
) {
    if !playing.is_empty() {
        return;
    }
    let in_menu = *state.get() == AppState::FrontEnd;

    // A read in flight: see whether it has landed.
    if let Some(task) = player.loading.as_mut() {
        let Some(result) = block_on(future::poll_once(task)) else {
            return;
        };
        player.loading = None;
        let Some(index) = player.current else {
            return;
        };
        let path = player.tracks[index].clone();

        match result {
            Ok(bytes) if is_playable_music(&bytes) => {
                let source = sources.add(AudioSource {
                    bytes: bytes.into(),
                });
                let mut settings = if in_menu {
                    PlaybackSettings::LOOP
                } else {
                    PlaybackSettings::DESPAWN
                };
                settings.volume = Volume::SILENT;
                commands.spawn((
                    Name::new(format!("music {}", path.display())),
                    MusicTrack,
                    AudioPlayer(source),
                    settings,
                    PlayingSound {
                        name: path.display().to_string(),
                        gain: 1.0,
                        range: None,
                        looped: in_menu,
                        kind: SoundKind::Music,
                    },
                    Transform::default(),
                ));
                info!("playing {}", path.display());
            }
            Ok(_) => {
                warn!(
                    "{}: not a format this build plays; skipping it",
                    path.display()
                );
                player.failed.insert(index);
                player.current = None;
            }
            Err(e) => {
                warn!("{}: cannot read it ({e}); skipping it", path.display());
                player.failed.insert(index);
                player.current = None;
            }
        }
        return;
    }

    let next = if in_menu {
        // The menu track loops, so once it is going nothing more happens
        // here; a failed or absent one leaves the menus quiet.
        player
            .menu
            .filter(|index| !player.failed.contains(index) && player.current != Some(*index))
    } else {
        next_in_game(&mut player)
    };
    let Some(index) = next else {
        return;
    };

    let path = player.tracks[index].clone();
    player.current = Some(index);
    player.loading = Some(IoTaskPool::get().spawn(async move { std::fs::read(path) }));
}

/// The next in-game track: the shuffled queue, refilled when it runs out.
///
/// The menu track sits out of the shuffle unless it is the only music there
/// is. A refill never starts with the track that just ended.
fn next_in_game(player: &mut MusicPlayer) -> Option<usize> {
    if player.queue.is_empty() {
        let mut pool: Vec<usize> = (0..player.tracks.len())
            .filter(|index| !player.failed.contains(index))
            .filter(|index| Some(*index) != player.menu)
            .collect();
        if pool.is_empty() {
            pool = player
                .menu
                .filter(|index| !player.failed.contains(index))
                .into_iter()
                .collect();
        }
        pool.shuffle(&mut rand::rng());
        if pool.len() > 1 && pool.last() == player.current.as_ref() {
            let last = pool.len() - 1;
            pool.swap(0, last);
        }
        player.queue = pool;
    }
    player.queue.pop()
}

/// Stop the music and forget where it was.
fn stop_music(
    mut commands: Commands,
    mut player: ResMut<MusicPlayer>,
    playing: Query<Entity, With<MusicTrack>>,
) {
    for entity in &playing {
        commands.entity(entity).despawn();
    }
    player.current = None;
    player.loading = None;
    player.queue.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::sounds::synth_wav;

    /// A directory under the system temp dir, removed when dropped.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> ScratchDir {
            let path = std::env::temp_dir().join(format!(
                "open-bft-music-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create a scratch dir");
            ScratchDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_scan_finds_music_sorted_and_names_the_menu_track() {
        let dir = ScratchDir::new("scan");
        for name in [
            "track03.wav",
            "Menu.OGG",
            "track02.flac",
            "cover.jpg",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").expect("write");
        }
        std::fs::create_dir(dir.path().join("extras.mp3")).expect("a directory");

        let (tracks, menu) = scan_music_dir(dir.path(), &["menu".to_string()]);
        let names: Vec<_> = tracks
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["Menu.OGG", "track02.flac", "track03.wav"]);
        assert_eq!(menu, Some(0));
    }

    #[test]
    fn a_missing_directory_is_no_music() {
        let (tracks, menu) =
            scan_music_dir(Path::new("/definitely/not/here"), &["menu".to_string()]);
        assert!(tracks.is_empty());
        assert_eq!(menu, None);
    }

    #[test]
    fn only_decodable_formats_pass_the_sniff() {
        assert!(is_playable_music(&synth_wav(44100, 10)));
        assert!(is_playable_music(b"fLaC\0\0\0\x22"));
        assert!(is_playable_music(b"ID3\x04\0\0\0\0\0\0"));
        assert!(is_playable_music(&[0xFF, 0xFB, 0x90, 0x00]));

        let mut vorbis = b"OggS\0\x02".to_vec();
        vorbis.extend_from_slice(&[0; 22]);
        vorbis.extend_from_slice(b"\x01vorbis");
        assert!(is_playable_music(&vorbis));

        let mut opus = b"OggS\0\x02".to_vec();
        opus.extend_from_slice(&[0; 22]);
        opus.extend_from_slice(b"OpusHead");
        assert!(!is_playable_music(&opus));

        assert!(!is_playable_music(b"RIFF\0\0\0\0AVI "));
        assert!(!is_playable_music(b"hello"));
    }

    #[test]
    fn the_shuffle_plays_everything_but_the_menu_before_repeating() {
        let mut player = MusicPlayer {
            tracks: (0..4).map(|i| PathBuf::from(format!("t{i}"))).collect(),
            menu: Some(2),
            ..default()
        };
        let mut seen = Vec::new();
        for _ in 0..3 {
            let next = next_in_game(&mut player).expect("a track");
            player.current = Some(next);
            seen.push(next);
        }
        seen.sort();
        assert_eq!(seen, [0, 1, 3]);
    }

    #[test]
    fn a_lone_menu_track_plays_in_game_too() {
        let mut player = MusicPlayer {
            tracks: vec![PathBuf::from("menu.ogg")],
            menu: Some(0),
            ..default()
        };
        assert_eq!(next_in_game(&mut player), Some(0));
    }

    #[test]
    fn failed_tracks_leave_the_shuffle() {
        let mut player = MusicPlayer {
            tracks: vec![PathBuf::from("a"), PathBuf::from("b")],
            ..default()
        };
        player.failed.insert(0);
        assert_eq!(next_in_game(&mut player), Some(1));
        player.failed.insert(1);
        assert_eq!(next_in_game(&mut player), None);
    }

    /// With no setting, music is found in the install or in a Recoil
    /// folder beside it, and only where there is music.
    #[test]
    fn music_is_found_beside_the_game_data() {
        let base = std::env::temp_dir().join(format!("bft-music-{}", std::process::id()));
        let install = base.join("Recoil");
        let cd = base.join("Recoil CD").join("music");
        let empty = base.join("Recoil AAA").join("music");
        for dir in [&install, &cd, &empty] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(cd.join("track02.wav"), b"").unwrap();

        assert_eq!(find_music_dir(&install), Some(cd.clone()));

        let own = install.join("Music");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("a.ogg"), b"").unwrap();
        assert_eq!(find_music_dir(&install), Some(own));

        std::fs::remove_dir_all(&base).unwrap();
    }
}
