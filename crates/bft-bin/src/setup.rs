//! `open-bft setup`: get the game ready from whatever form the user has it
//! in, an installed copy, the disc's files, a disc image or a music folder,
//! and remember it.
//!
//! The chain (`bft_assets::source`): a disc image's data track holds the
//! disc's files; their InstallShield cabinet unpacks, with `unshield`, into
//! an install; the install extracts, with `unzbd`, into the cache. An image's
//! CD-audio tracks become the music folder. Each source enters the chain
//! where it fits, and each step is skipped when its result is already there.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bft_assets::cabinet::{self, UnpackProgress};
use bft_assets::disc::{self, DataTrack, Track, TrackKind, iso9660};
use bft_assets::extract;
use bft_assets::source::{self, Source};

use crate::{PathArgs, run_extract};

/// Run `setup` on the paths given (none: use or search for an install).
pub(crate) fn run(
    args: &PathArgs,
    given: &[PathBuf],
    yes: bool,
    unshield: Option<&Path>,
) -> Result<(), ExitCode> {
    // Recognise every path given, by content. Absolute, so what is
    // remembered still works from another directory.
    let mut sources = Vec::new();
    for path in given {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        match source::classify(&path) {
            Ok(found) => {
                println!("{}: {}", path.display(), found.describe());
                sources.push(found);
            }
            Err(e) => {
                eprintln!("open-bft: {e}");
                return Err(ExitCode::from(2));
            }
        }
    }

    // Music first: it needs nothing else, and a music folder on its own is
    // how music is added to a game set up earlier.
    let music_given = sources
        .iter()
        .any(|found| matches!(found, Source::Music(_)));
    for found in &sources {
        if let Source::Music(dir) = found {
            remember(|settings| {
                let changed = settings.audio.music_dir.as_deref() != Some(dir.as_path());
                settings.audio.music_dir = Some(dir.clone());
                changed.then(|| format!("the music folder {}", dir.display()))
            })?;
        }
    }

    let data = match sources.iter().find(|found| found.has_data()) {
        Some(found) => found.clone(),
        None => find_data(args, yes)?,
    };

    let install = match &data {
        Source::Install(path) => path.clone(),
        Source::DiscFolder(dir) => {
            let Some(cab) = source::find_file_ignoring_case(dir, "data1.cab") else {
                eprintln!("open-bft: {}: no DATA1.CAB", dir.display());
                return Err(ExitCode::from(2));
            };
            let install = install_from_cabinet(unshield, |_| Ok(cab.clone()))?;
            if !music_given {
                println!(
                    "\nMusic is not read from a mounted disc. Rip its four audio \
                     tracks with any CD ripper and run `open-bft setup <folder>`."
                );
            }
            install
        }
        Source::DiscImage { path, format } => {
            let tracks = disc::tracks(path, *format).map_err(failed)?;
            let install =
                install_from_cabinet(unshield, |staging| cabinet_from_image(&tracks, staging))?;
            if !music_given {
                music_from_image(&tracks)?;
            }
            install
        }
        Source::Music(_) => unreachable!("a music folder holds no data"),
    };

    let mut args = args.clone();
    args.data_dir = Some(install);

    // Validates the install, and stops with FR-D1's message if it is not one.
    let paths = args.resolve()?;
    remember(|settings| {
        let root = &paths.root.path;
        let changed = settings.data_root.as_deref() != Some(root.as_path());
        settings.data_root = Some(root.clone());
        changed.then(|| format!("the install {}", root.display()))
    })?;

    if paths.unzbd.is_none() {
        build_unzbd()?;
    }

    // Resolves again, which now finds the `unzbd` just built.
    run_extract(&args, false, false)?;

    println!("\nReady. Play with `open-bft`, or `cargo run` from a checkout.");
    Ok(())
}

/// Report a library error and fail.
fn failed(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("open-bft: {e}");
    ExitCode::FAILURE
}

/// The project's own data directory, where an unpacked install and the
/// disc's music go.
fn data_dir() -> Result<PathBuf, ExitCode> {
    bft_assets::cache::project_dirs()
        .map(|dirs| dirs.data_dir().to_path_buf())
        .ok_or_else(|| {
            eprintln!(
                "open-bft: no data directory on this platform to unpack into; \
                 install the game and run setup on the installed folder"
            );
            ExitCode::FAILURE
        })
}

/// The install unpacked from the disc's cabinet, at `<data dir>/recoil`.
///
/// Skipped when that folder already holds an install. Otherwise `cabinet`
/// names the `DATA1.CAB` to unpack, given a scratch folder it may copy it
/// into; the folder is removed afterwards.
fn install_from_cabinet(
    unshield: Option<&Path>,
    cabinet: impl FnOnce(&Path) -> Result<PathBuf, ExitCode>,
) -> Result<PathBuf, ExitCode> {
    let install = data_dir()?.join("recoil");
    if bft_assets::root::validate(&install).is_ok() {
        println!("Already unpacked: {}", install.display());
        return Ok(install);
    }

    // Before any copying: without `unshield` there is nothing to do.
    let Some(unshield) = cabinet::find_unshield(unshield) else {
        eprintln!(
            "open-bft: unpacking the disc's installer needs `unshield`. Install it \
             with {}, or pass --unshield <file>.",
            cabinet::install_hint()
        );
        return Err(ExitCode::FAILURE);
    };

    let scratch = install.with_extension("disc");
    std::fs::create_dir_all(&scratch).map_err(|e| failed(format!("{}: {e}", scratch.display())))?;
    let result = cabinet(&scratch).and_then(|cab| {
        println!("Unpacking the installer into {}", install.display());
        cabinet::unpack(&unshield, &cab, &install, |progress| match progress {
            UnpackProgress::Group(n, of, name) => println!("  [{n}/{of}] {name}"),
            UnpackProgress::Unknown(name) => println!(
                "  note: left out the cabinet's group \"{name}\", which setup does not \
                 know; please report it"
            ),
        })
        .map_err(failed)
    });
    let _ = std::fs::remove_dir_all(&scratch);
    result?;

    Ok(install)
}

/// Copy `DATA1.CAB` from an image's data track into `scratch`.
fn cabinet_from_image(tracks: &[Track], scratch: &Path) -> Result<PathBuf, ExitCode> {
    let Some(data) = tracks.iter().find(|track| track.kind == TrackKind::Data) else {
        eprintln!("open-bft: the image has no data track");
        return Err(ExitCode::from(2));
    };

    let mut volume = DataTrack::open(data)
        .and_then(iso9660::Volume::open)
        .map_err(failed)?;
    let Some(entry) = volume.find("data1.cab").map_err(failed)? else {
        eprintln!(
            "open-bft: the image's data track (volume \"{}\") has no DATA1.CAB; \
             it does not look like the Recoil disc",
            volume.label
        );
        return Err(ExitCode::from(2));
    };

    let dest = scratch.join("data1.cab");
    let total = entry.size.max(1);
    let mut shown = u64::MAX;
    volume
        .copy(&entry, &dest, |done| {
            let percent = done * 100 / total;
            if percent != shown {
                shown = percent;
                print!("\r\x1b[KCopying DATA1.CAB out of the image: {percent}%");
                let _ = std::io::stdout().flush();
            }
        })
        .map_err(failed)?;
    println!();

    Ok(dest)
}

/// Write an image's CD-audio tracks to `<data dir>/music` as WAV, named by
/// track number (`track02.wav` loops in the menus, README "Music"), and
/// make that the music folder unless the player already chose one.
fn music_from_image(tracks: &[Track]) -> Result<(), ExitCode> {
    let audio: Vec<&Track> = tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .collect();
    if audio.is_empty() {
        println!(
            "\nThe image has no music tracks (an .iso holds only the data track). \
             Add music later with `open-bft setup <folder of ripped tracks>`."
        );
        return Ok(());
    }

    let dir = data_dir()?.join("music");
    std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
    println!(
        "\nWriting the disc's {} music tracks to {}",
        audio.len(),
        dir.display()
    );

    for track in audio {
        let dest = dir.join(format!("track{:02}.wav", track.number));
        let expected = 44 + track.sectors * u64::from(disc::AUDIO_SECTOR);
        if std::fs::metadata(&dest).is_ok_and(|m| m.len() == expected) {
            println!("  track {:02}: already there", track.number);
            continue;
        }
        let written = disc::write_wav(track, &dest).map_err(failed)?;
        let seconds = written.seconds.round() as u64;
        println!(
            "  track {:02}: {}:{:02}",
            track.number,
            seconds / 60,
            seconds % 60
        );
    }

    remember(|settings| match &settings.audio.music_dir {
        None => {
            settings.audio.music_dir = Some(dir.clone());
            Some(format!("the music folder {}", dir.display()))
        }
        Some(own) if own == &dir => None,
        Some(own) => {
            println!(
                "  Kept your music folder {}; the disc's tracks are in {}",
                own.display(),
                dir.display()
            );
            None
        }
    })
}

/// Where the game's data is when no path given holds it: the install the
/// engine already finds (`--data-dir`, `BFT_DATA_DIR`, the config, the usual
/// places), else the first thing a search turns up that the user accepts.
fn find_data(args: &PathArgs, yes: bool) -> Result<Source, ExitCode> {
    // An explicit --data-dir is a decision: report it if it is wrong
    // rather than searching past it (FR-D1).
    if args.data_dir.is_some() {
        return Ok(Source::Install(args.resolve()?.root.path));
    }
    if let Ok(paths) = args.resolve_quietly() {
        return Ok(Source::Install(paths.root.path));
    }

    println!("Looking for Recoil...");
    let found = source::candidates();

    let chosen = match found.as_slice() {
        [] => None,
        [only] => {
            println!("Found {} at {}", only.describe(), only.path().display());
            confirm(yes)?.then(|| only.clone())
        }
        several => {
            println!("Found:");
            for (n, found) in several.iter().enumerate() {
                println!(
                    "  {}. {} at {}",
                    n + 1,
                    found.describe(),
                    found.path().display()
                );
            }
            pick(several.len(), yes)?.map(|n| several[n].clone())
        }
    };

    chosen.ok_or_else(|| {
        if found.is_empty() {
            eprintln!(
                "open-bft: no copy of Recoil found in the usual install places, on \
                 mounted media, or as a disc image in this folder, your \
                 Downloads or your Desktop.\n\nName it: open-bft setup <install, \
                 disc folder or disc image>"
            );
        }
        ExitCode::from(2)
    })
}

/// Ask whether to use the one thing found. `--yes` answers for the user; with
/// no terminal to ask on, the answer is no.
fn confirm(yes: bool) -> Result<bool, ExitCode> {
    if yes {
        return Ok(true);
    }
    let answer = ask("Use it? [Y/n] ")?;

    Ok(matches!(answer.as_str(), "" | "y" | "yes"))
}

/// Ask which of `count` things to use, 0-based. Enter takes the first, which
/// is the most direct source (an install before a disc before an image).
fn pick(count: usize, yes: bool) -> Result<Option<usize>, ExitCode> {
    if yes {
        return Ok(Some(0));
    }
    let answer = ask(&format!("Which one? [1-{count}, Enter for 1, n for none] "))?;

    Ok(match answer.as_str() {
        "" => Some(0),
        text => text
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=count).contains(n))
            .map(|n| n - 1),
    })
}

/// One line from the terminal, trimmed and lower-cased.
fn ask(question: &str) -> Result<String, ExitCode> {
    use std::io::IsTerminal as _;

    if !std::io::stdin().is_terminal() {
        eprintln!("open-bft: not asking without a terminal; pass --yes or name the source");
        return Err(ExitCode::from(2));
    }

    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| {
        eprintln!("open-bft: {e}");
        ExitCode::FAILURE
    })?;

    Ok(line.trim().to_ascii_lowercase())
}

/// Change `config.ron` so later runs find things unaided. `change` edits
/// the settings and names what it set, or returns `None` when nothing
/// changed and the file is left as it is.
///
/// Goes through the game's own [`Settings`](bft_game::ui::Settings), the
/// struct that owns the whole file, so the player's other settings survive.
/// A file that does not parse is left alone: the game would replace it with
/// defaults, but `setup` is not the place to throw away someone's bindings.
fn remember(
    change: impl FnOnce(&mut bft_game::ui::Settings) -> Option<String>,
) -> Result<(), ExitCode> {
    use bft_game::ui::settings::{load_settings_from, save_settings_to};

    let Some(file) = bft_assets::Config::default_path() else {
        println!("No config directory on this platform; pass the paths on each run.");
        return Ok(());
    };

    let mut settings = load_settings_from(&file).map_err(|e| {
        eprintln!(
            "open-bft: {e}\n\nThe config file does not parse, so setup leaves it \
             alone. Fix or remove it, then run setup again."
        );
        ExitCode::FAILURE
    })?;

    let Some(what) = change(&mut settings) else {
        return Ok(());
    };

    save_settings_to(&settings, &file).map_err(|e| {
        eprintln!("open-bft: cannot save the config: {e}");
        ExitCode::FAILURE
    })?;

    println!("Remembered {what} in {}", file.display());
    Ok(())
}

/// Build the pinned `unzbd` with cargo into the platform data directory,
/// where [`extract::find_unzbd`] looks last.
fn build_unzbd() -> Result<(), ExitCode> {
    let Some(root) = extract::managed_unzbd_root() else {
        eprintln!(
            "open-bft: no data directory on this platform to build `unzbd` into; \
             build it yourself (docs/data/install.md) and pass --unzbd"
        );
        return Err(ExitCode::FAILURE);
    };

    // The cargo that ran us, under `cargo run`; else whichever is on PATH.
    let cargo = std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from);
    let mut command = extract::unzbd_install_command(&cargo, &root);
    let by_hand = extract::display_command(&extract::unzbd_install_command(
        std::path::Path::new("cargo"),
        &root,
    ));

    println!(
        "No `unzbd` found. Building it from mech3ax {} with cargo; this takes a \
         minute or two.\n",
        &extract::MECH3AX_COMMIT[..12]
    );

    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "\nopen-bft: building `unzbd` failed ({status}). The command was:\n  {by_hand}"
            );
            return Err(ExitCode::FAILURE);
        }
        Err(e) => {
            eprintln!(
                "open-bft: cannot run cargo ({e}). Install Rust from https://rustup.rs, \
                 or build `unzbd` some other way and pass --unzbd. With cargo, the \
                 command is:\n  {by_hand}"
            );
            return Err(ExitCode::FAILURE);
        }
    }

    match extract::managed_unzbd() {
        Some(unzbd) if unzbd.is_file() => {
            println!("\nBuilt {}", unzbd.display());
            Ok(())
        }
        _ => {
            eprintln!(
                "open-bft: cargo finished, but no `unzbd` is under {}",
                root.display()
            );
            Err(ExitCode::FAILURE)
        }
    }
}
