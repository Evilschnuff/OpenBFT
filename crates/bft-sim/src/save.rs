//! In-mission save games: capture the running mission, and put it back.
//!
//! The file format is `docs/SAVEGAME.md`; its types are [`format`]. This
//! module is the engine half: [`capture`] reads a running world into a
//! [`SaveFile`], [`restore`] writes one into a freshly loaded level, and the
//! envelope functions ([`read_save`], [`write_save`]) handle versions and
//! files.
//!
//! # A save is a difference from the fresh level
//!
//! Loading a save loads the level from the game data exactly as starting the
//! mission does, then applies the file. So the file holds only what changed:
//! the nodes the scripts moved or hid, the damage done, the VM's running
//! sequences, the vehicles, pickups and shells. [`SaveBaseline`] is how
//! capture knows what changed — the node transforms, parents and active
//! states as the mission set-up left them, recorded once per level — and
//! because a restored session records its own baseline *before* the restore,
//! a save of a loaded game diffs against the same fresh level.
//!
//! # The order a load happens in
//!
//! 1. `bft-game` sets the difficulty and loads the level with a
//!    [`RestoreGate`] in place, which holds the sim still ([`sim_running`]
//!    reads it): no tick may run between the spawn and the restore, or the
//!    AI and the VM would start the mission the save is about to replace.
//! 2. Once the mission is set up it records the baseline and calls
//!    [`restore`], which rebuilds the world and removes the gate.
//! 3. The first tick after that applies what the VM's own level-change reset
//!    would otherwise wipe ([`PendingAnimRestore`]) and, because the mission
//!    is marked resumed, calls `startanims.zrd`'s `load_game_anim`
//!    (`load_control`) instead of the intro — which is what the original did.
//!
//! [`sim_running`]: crate::states::sim_running

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::anim::{AnimCamera, AnimLight, AnimLights, AnimMotion, AnimMotions, AnimScreenFx};
use crate::level::{NodeActive, NodeName};
use crate::schedule::SimSet;

mod capture;
pub mod format;
pub mod migrate;
mod restore;

pub use capture::{CaptureMeta, SaveRefusal, can_save, capture, fingerprint, save_refusal};
pub use format::{CURRENT_VERSION, FORMAT_TAG, SaveFile, SaveHeader, SaveProbe, SaveVersion};
pub use restore::{RestoreReport, restore};

/// The file name of the quick save, in [`saves_dir`].
pub const QUICKSAVE_FILE: &str = "quicksave.ron";

/// The save systems: the deferred half of a restore.
pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SaveBaseline>().add_systems(
            FixedUpdate,
            // After the VM forgets the previous level's motions and lights —
            // which it does on the first tick a program is new, and that is
            // exactly the tick after a restore — and before anything steps
            // them.
            apply_pending_anim_restore
                .after(crate::anim::apply::forget_previous_level)
                .before(crate::anim::accumulate_hit_damage)
                .in_set(SimSet::Anim),
        );
    }
}

// ---------------------------------------------------------------------------
// The baseline
// ---------------------------------------------------------------------------

/// Every node as the mission set-up left it: what a save is a difference
/// from.
///
/// Keyed by entity rather than by node id so the enemies' model parts, which
/// have no id, are covered by the same map.
#[derive(Resource, Debug, Clone, Default)]
pub struct SaveBaseline {
    pub nodes: HashMap<Entity, BaselineNode>,
}

/// One node's recorded state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaselineNode {
    pub transform: Transform,
    pub parent: Option<Entity>,
    pub active: bool,
}

impl SaveBaseline {
    /// Record every named node as it is now.
    ///
    /// Call once a level's mission is set up, before a restore and before the
    /// scripts have moved anything.
    pub fn record(world: &mut World) {
        let mut query =
            world.query::<(Entity, &Transform, Option<&ChildOf>, Option<&NodeActive>)>();
        let mut named = world.query_filtered::<Entity, With<NodeName>>();
        let named: Vec<Entity> = named.iter(world).collect();

        let mut nodes = HashMap::with_capacity(named.len());
        for entity in named {
            let Ok((entity, transform, parent, active)) = query.get(world, entity) else {
                continue;
            };
            nodes.insert(
                entity,
                BaselineNode {
                    transform: *transform,
                    parent: parent.map(ChildOf::parent),
                    active: active.is_none_or(|active| active.0),
                },
            );
        }

        info!("save baseline: {} node(s)", nodes.len());
        world.insert_resource(SaveBaseline { nodes });
    }
}

// ---------------------------------------------------------------------------
// Holding the sim for a restore
// ---------------------------------------------------------------------------

/// A save is waiting to be applied: the sim does not step.
///
/// Inserted before the level loads and removed by [`restore`].
#[derive(Resource, Debug, Clone, Default)]
pub struct RestoreGate;

/// What a restore could not apply at once, for the first tick after it.
///
/// The VM clears its motions, lights, camera and screen state on the first
/// tick a level's program is new ([`forget_previous_level`]), and a restore
/// happens before that tick. So these wait here.
///
/// [`forget_previous_level`]: crate::anim::apply::forget_previous_level
#[derive(Resource, Debug, Clone, Default)]
pub struct PendingAnimRestore {
    pub motions: Vec<(Entity, AnimMotion)>,
    pub lights: Vec<((Entity, String), AnimLight)>,
    pub camera: Option<AnimCamera>,
    pub screen: Option<AnimScreenFx>,
}

/// Apply a restore's deferred anim state.
pub fn apply_pending_anim_restore(
    mut commands: Commands,
    pending: Option<ResMut<PendingAnimRestore>>,
    mut motions: ResMut<AnimMotions>,
    mut lights: ResMut<AnimLights>,
    mut camera: ResMut<AnimCamera>,
    mut screen: ResMut<AnimScreenFx>,
) {
    let Some(mut pending) = pending else {
        return;
    };
    motions.0.extend(pending.motions.drain(..));
    lights.0.extend(pending.lights.drain(..));
    if let Some(restored) = pending.camera.take() {
        *camera = restored;
    }
    if let Some(restored) = pending.screen.take() {
        *screen = restored;
    }
    commands.remove_resource::<PendingAnimRestore>();
}

// ---------------------------------------------------------------------------
// Reading and writing
// ---------------------------------------------------------------------------

/// What can go wrong reading or writing a save.
#[derive(Debug, thiserror::Error)]
pub enum SaveFileError {
    #[error("cannot read or write the save: {0}")]
    Io(#[from] std::io::Error),
    #[error("the save is not valid RON: {0}")]
    Parse(#[from] ron::de::SpannedError),
    #[error("cannot serialise the save: {0}")]
    Serialise(#[from] ron::Error),
    #[error("not an open-bft save (format `{0}`)")]
    NotASave(String),
    #[error("written by a newer open-bft (save format {found}, this build reads up to {current})")]
    TooNew {
        found: SaveVersion,
        current: SaveVersion,
    },
    #[error("cannot upgrade a save from format {from}: {reason}")]
    Migration { from: SaveVersion, reason: String },
    #[error("this platform has no data directory to keep saves in")]
    NoDataDir,
}

/// Read a save's envelope and header without reading the rest.
///
/// Only the head of the file is parsed: RON skips a field it does not want
/// through its untyped path, which is quadratic in the field's size — two
/// seconds and more to step over a 350 KB save's records, where parsing the
/// whole file into its types takes milliseconds. Every writer puts
/// `format`, `version` and `header` first (SAVEGAME §4), so the head is
/// everything up to the comma after the header. A file where that cut
/// cannot be found is parsed whole, slowly but correctly.
pub fn probe_save(text: &str) -> Result<SaveProbe, SaveFileError> {
    let head = save_head(text);
    let text = head.as_deref().unwrap_or(text);

    let envelope: format::SaveEnvelope = ron::from_str(text)?;
    if envelope.format != FORMAT_TAG {
        return Err(SaveFileError::NotASave(envelope.format));
    }
    let header = ron::from_str::<format::HeaderOnly>(text)
        .ok()
        .map(|only| only.header);
    Ok(SaveProbe {
        format: envelope.format,
        version: envelope.version,
        header,
    })
}

/// The file's top-level struct cut after its `header` field and closed
/// again: `(format: …, version: …, header: (…))`.
///
/// A scan for the comma that ends `header` at the top level, aware of
/// strings and their escapes. `None` when there is no such field, or the
/// file has a comment (the writer never makes one) that could hide a
/// bracket.
fn save_head(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    // Where the current top-level field's name starts.
    let mut field_start: Option<usize> = None;

    for (i, &byte) in bytes.iter().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'/' if bytes
                .get(i + 1)
                .is_some_and(|next| *next == b'/' || *next == b'*') =>
            {
                return None;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth == 1 {
                    field_start = Some(i + 1);
                }
            }
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b',' if depth == 1 => {
                let field = text.get(field_start?..i)?;
                let name = field.split(':').next()?.trim();
                if name == "header" {
                    return Some(format!("{})", &text[..i]));
                }
                field_start = Some(i + 1);
            }
            _ => {}
        }
    }
    None
}

/// Parse a save of any version this build can read.
///
/// The same major loads directly — unknown fields ignored, missing ones
/// defaulted — an older major is migrated first, and a newer major is
/// refused (SAVEGAME §4).
pub fn read_save(text: &str) -> Result<SaveFile, SaveFileError> {
    let probe = probe_save(text)?;
    let version = probe.version;

    if version.major > CURRENT_VERSION.major {
        return Err(SaveFileError::TooNew {
            found: version,
            current: CURRENT_VERSION,
        });
    }

    let mut save: SaveFile = if version.major < CURRENT_VERSION.major {
        migrate::upgrade(text, version)?
    } else {
        if version.minor > CURRENT_VERSION.minor {
            info!(
                "save format {version} is newer than {CURRENT_VERSION}; \
                 reading what this build knows and ignoring the rest"
            );
        }
        ron::from_str(text)?
    };

    // What the file now is, in memory: re-saving it writes this build's
    // format.
    save.version = CURRENT_VERSION;
    Ok(save)
}

/// Serialise a save as the file holds it.
pub fn save_to_ron(save: &SaveFile) -> Result<String, SaveFileError> {
    // One line per record: the sections and their lists are laid out, and
    // everything deeper is written inline. Readable, diffable, and a third
    // the size of pretty-printing every vector across four lines.
    let config = ron::ser::PrettyConfig::default()
        .struct_names(false)
        .depth_limit(3)
        .indentor("  ".to_string());
    Ok(ron::ser::to_string_pretty(save, config)?)
}

/// Write a save to `path`, atomically: a crash mid-write leaves the old
/// file, never half a new one.
pub fn write_save(save: &SaveFile, path: &Path) -> Result<(), SaveFileError> {
    let text = save_to_ron(save)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("ron.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Read and parse the save at `path`.
pub fn load_save(path: &Path) -> Result<SaveFile, SaveFileError> {
    read_save(&std::fs::read_to_string(path)?)
}

/// Where saves live: `<data dir>/saves`.
pub fn saves_dir() -> Result<PathBuf, SaveFileError> {
    directories::ProjectDirs::from("org", "open-bft", "open-bft")
        .map(|dirs| dirs.data_dir().join("saves"))
        .ok_or(SaveFileError::NoDataDir)
}

/// One save on disk, as the load list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveEntry {
    pub path: PathBuf,
    /// `None` when the header could not be read at all.
    pub header: Option<SaveHeader>,
    pub version: Option<SaveVersion>,
    /// Why this build cannot load it, if it cannot.
    pub problem: Option<String>,
}

impl SaveEntry {
    /// Whether this build can load it.
    pub fn is_loadable(&self) -> bool {
        self.problem.is_none()
    }
}

/// Every save in `dir`, newest first.
///
/// A file that will not parse is listed with its problem rather than
/// hidden, so a player can see that a save exists and why it will not load.
pub fn list_saves(dir: &Path) -> Vec<SaveEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut entries: Vec<SaveEntry> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ron"))
        .map(|path| {
            let probe = std::fs::read_to_string(&path)
                .map_err(SaveFileError::from)
                .and_then(|text| probe_save(&text));
            match probe {
                Ok(probe) => {
                    let problem = (probe.version.major > CURRENT_VERSION.major)
                        .then(|| format!("written by a newer open-bft (format {})", probe.version));
                    SaveEntry {
                        path,
                        header: probe.header,
                        version: Some(probe.version),
                        problem,
                    }
                }
                Err(e) => SaveEntry {
                    path,
                    header: None,
                    version: None,
                    problem: Some(e.to_string()),
                },
            }
        })
        .collect();

    entries.sort_by(|a, b| {
        let created = |entry: &SaveEntry| entry.header.as_ref().map_or(0, |header| header.created);
        created(b)
            .cmp(&created(a))
            .then_with(|| a.path.cmp(&b.path))
    });
    entries
}

/// The path a new, non-quick save goes to.
pub fn new_save_path(dir: &Path, created: u64) -> PathBuf {
    let mut path = dir.join(format!("save-{created}.ron"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("save-{created}-{n}.ron"));
        n += 1;
    }
    path
}

/// Seconds since the Unix epoch, now.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// FNV-1a, 64 bit: a hash that is the same on every platform and every Rust
/// release, which `std`'s is not promised to be.
#[derive(Debug, Clone, Copy)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Fnv64 {
        Fnv64(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv64 {
    pub fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests;
