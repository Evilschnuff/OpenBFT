//! Driving `unzbd` to populate the cache (FR-D3, FR-D5).
//!
//! [`plan`] enumerates what an install offers and where each piece belongs in
//! the cache, marking the jobs a previous run already did. [`run`] executes
//! the plan one job at a time, which is how `tools/recon/extract.sh` proved
//! the command table in docs/data/install.md.
//!
//! No `.zbd` byte is parsed here or anywhere else in the engine: mech3ax owns
//! that, and this module only spawns it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cache::{CacheDir, CacheEntry, CacheManifest, ExtractKind};
use crate::root::{DataRoot, MAX_MISSION};

/// Every [`ExtractKind`], for tests and for iterating the command table.
pub const ALL_KINDS: [ExtractKind; 8] = [
    ExtractKind::Sounds,
    ExtractKind::Interp,
    ExtractKind::Reader,
    ExtractKind::Messages,
    ExtractKind::Textures,
    ExtractKind::Gamez,
    ExtractKind::Anim,
    ExtractKind::ZMap,
];

/// What extracting an install would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractPlan {
    /// Every job, in the order [`run`] will execute it.
    pub jobs: Vec<ExtractJob>,
}

impl ExtractPlan {
    /// The jobs that still have to run.
    pub fn stale(&self) -> impl Iterator<Item = &ExtractJob> {
        self.jobs.iter().filter(|job| !job.up_to_date)
    }

    /// How many jobs still have to run.
    pub fn stale_count(&self) -> usize {
        self.stale().count()
    }
}

/// One `unzbd` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractJob {
    /// What kind of archive this is.
    pub kind: ExtractKind,
    /// The source file, absolute.
    pub source: PathBuf,
    /// The output, relative to the cache directory.
    pub output: PathBuf,
    /// Whether the manifest and the filesystem agree this is already done.
    pub up_to_date: bool,
}

impl ExtractJob {
    /// The command line that performs this job, for `--dry-run` and for the
    /// "run these yourself" message when `unzbd` is missing.
    pub fn command_line(&self, unzbd: &Path, cache: &CacheDir) -> String {
        format!(
            "{} rc {} {} {}",
            quote(&unzbd.to_string_lossy()),
            self.kind.subcommand(),
            quote(&self.source.to_string_lossy()),
            quote(&cache.path.join(&self.output).to_string_lossy()),
        )
    }
}

/// A command as a copy-pasteable shell line, for telling a user what to run
/// by hand.
pub fn display_command(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| quote(&part.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote a path for a copy-pasteable shell command line.
fn quote(s: &str) -> String {
    if s.is_empty() || s.contains([' ', '\'', '"', '$', '\\', '(', ')', '&', ';']) {
        format!("'{}'", s.replace('\'', r"'\''"))
    } else {
        s.to_string()
    }
}

/// Why planning or extraction failed.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// The install's directory structure could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: std::io::Error,
    },
    /// The `unzbd` binary could not be started at all.
    #[error("cannot run {unzbd}: {source}")]
    Unzbd {
        /// The binary that was tried.
        unzbd: PathBuf,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },
    /// Some jobs failed. The manifest still holds everything that worked.
    #[error("{} of {total} extraction jobs failed:\n{}", failures.len(), format_failures(failures))]
    Jobs {
        /// One entry per failed job.
        failures: Vec<JobFailure>,
        /// How many jobs the run attempted.
        total: usize,
    },
    /// The cache could not be written.
    #[error(transparent)]
    Cache(#[from] crate::cache::CacheError),
}

/// One job that `unzbd` did not complete.
#[derive(Debug)]
pub struct JobFailure {
    /// What was being extracted.
    pub kind: ExtractKind,
    /// The source that failed.
    pub source: PathBuf,
    /// What went wrong: a non-zero exit with its stderr, or a spawn failure.
    pub reason: String,
    /// The log file holding the command's full stderr, when one was written.
    pub log: Option<PathBuf>,
}

fn format_failures(failures: &[JobFailure]) -> String {
    failures
        .iter()
        .map(|f| {
            let log = match &f.log {
                Some(log) => format!(" (log: {})", log.display()),
                None => String::new(),
            };
            format!("  {} {}: {}{log}", f.kind, f.source.display(), f.reason)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How far [`run`] has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractProgress {
    /// A job is about to start. `job` is a zero-based index into the plan.
    Started {
        /// Index of the job in the plan.
        job: usize,
        /// How many jobs the plan holds.
        total: usize,
        /// What kind of archive it is.
        kind: ExtractKind,
        /// The source being extracted.
        source: PathBuf,
    },
    /// A job finished successfully.
    Finished {
        /// Index of the job in the plan.
        job: usize,
    },
    /// A job was already up to date and did not run.
    Skipped {
        /// Index of the job in the plan.
        job: usize,
    },
    /// A job failed. The run continues with the next one.
    Failed {
        /// Index of the job in the plan.
        job: usize,
        /// What went wrong.
        reason: String,
    },
}

/// The texture archives worth extracting, in preference order.
///
/// docs/data/materials.md: `rtexture4` is the full-size hardware set and `rtexture2`
/// the same 576 names at half size, so both are extracted (03 picks a tier).
/// The `texture*` family is the software renderer's palettised equivalent and
/// is skipped.
const TEXTURE_ARCHIVES: [&str; 2] = ["rtexture4", "rtexture2"];

/// The sound archive to extract.
///
/// docs/data/sounds.md: the three fidelity tiers hold exactly the same 280 names,
/// so only the best one is worth 35 MB of cache.
const SOUND_ARCHIVE: &str = "soundsh";

/// Enumerate what an install offers and where each piece goes in the cache.
///
/// Sources follow docs/data/install.md's command table. A job is `up_to_date` when
/// the manifest records the same source length and mtime *and* the output
/// file is still on disk, so deleting a cache file forces its job to re-run.
pub fn plan(
    root: &DataRoot,
    cache: &CacheDir,
    manifest: Option<&CacheManifest>,
) -> Result<ExtractPlan, ExtractError> {
    let zbd = root.path.join("zbd");
    let mut jobs = Vec::new();

    // --- global archives ---
    push(
        &mut jobs,
        ExtractKind::Interp,
        zbd.join("interp.zbd"),
        PathBuf::from("interp.json"),
    );
    push(
        &mut jobs,
        ExtractKind::Messages,
        root.path.join("messages.dll"),
        PathBuf::from("messages.json"),
    );
    // FR-D2 allows either name; whichever exists is the message table.
    if !root.path.join("messages.dll").is_file() {
        push(
            &mut jobs,
            ExtractKind::Messages,
            root.path.join("strings.dll"),
            PathBuf::from("messages.json"),
        );
    }
    push(
        &mut jobs,
        ExtractKind::Reader,
        zbd.join("zrdr.zbd"),
        PathBuf::from("reader/zrdr.rdr.zip"),
    );
    push(
        &mut jobs,
        ExtractKind::Textures,
        zbd.join("image.zbd"),
        PathBuf::from("textures/image.tex.zip"),
    );
    push(
        &mut jobs,
        ExtractKind::Sounds,
        zbd.join(format!("{SOUND_ARCHIVE}.zbd")),
        PathBuf::from(format!("sounds/{SOUND_ARCHIVE}.snd.zip")),
    );

    // --- automaps ---
    for n in 1..=MAX_MISSION {
        push(
            &mut jobs,
            ExtractKind::ZMap,
            root.path.join("maps").join(format!("m{n}.zmap")),
            PathBuf::from(format!("zmap/m{n}.zmap.json")),
        );
    }

    // --- per mission ---
    for n in 1..=MAX_MISSION {
        let mission = format!("m{n}");
        let dir = zbd.join(&mission);
        if !dir.is_dir() {
            continue;
        }

        push(
            &mut jobs,
            ExtractKind::Gamez,
            dir.join("gamez.zbd"),
            PathBuf::from(format!("gamez/{mission}-gamez.gamez.zip")),
        );
        push(
            &mut jobs,
            ExtractKind::Anim,
            dir.join("anim.zbd"),
            PathBuf::from(format!("anim/{mission}-anim.anim.zip")),
        );
        push(
            &mut jobs,
            ExtractKind::Reader,
            dir.join("zrdr.zbd"),
            PathBuf::from(format!("reader/{mission}-zrdr.rdr.zip")),
        );
        for archive in TEXTURE_ARCHIVES {
            push(
                &mut jobs,
                ExtractKind::Textures,
                dir.join(format!("{archive}.zbd")),
                PathBuf::from(format!("textures/{mission}-{archive}.tex.zip")),
            );
        }
    }

    for job in &mut jobs {
        job.up_to_date = is_up_to_date(job, cache, manifest);
    }

    Ok(ExtractPlan { jobs })
}

/// Add a job when its source exists. A missing source is normal — `m13` has
/// no zmap, `texture8.zbd` only exists in m6 — so it is silently skipped.
fn push(jobs: &mut Vec<ExtractJob>, kind: ExtractKind, source: PathBuf, output: PathBuf) {
    if source.is_file() {
        jobs.push(ExtractJob {
            kind,
            source,
            output,
            up_to_date: false,
        });
    }
}

/// Whether a previous run already did this job with this same input.
fn is_up_to_date(job: &ExtractJob, cache: &CacheDir, manifest: Option<&CacheManifest>) -> bool {
    let Some(manifest) = manifest else {
        return false;
    };
    let Some(entry) = manifest
        .entries
        .iter()
        .find(|e| e.output == job.output && e.source == job.source)
    else {
        return false;
    };
    let Some((len, mtime)) = source_stamp(&job.source) else {
        return false;
    };

    entry.source_len == len && entry.source_mtime == mtime && cache.path.join(&job.output).exists()
}

/// A source's length and mtime, the pair that decides staleness.
fn source_stamp(source: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(source).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();

    Some((meta.len(), mtime))
}

/// Run a plan's stale jobs, writing the manifest at the end.
///
/// Jobs run sequentially: `unzbd` is I/O-bound on a 450 MB install and the
/// progress callback is easier to reason about in order. A failing job does
/// not stop the run (FR-D3's "one broken level does not block"); its stderr
/// goes to `<output>.log`, the others still land in the cache, and the
/// manifest records everything that worked before [`ExtractError::Jobs`] is
/// returned.
pub fn run(
    plan: &ExtractPlan,
    unzbd: &Path,
    cache: &CacheDir,
    mut progress: impl FnMut(ExtractProgress),
) -> Result<CacheManifest, ExtractError> {
    let version = unzbd_version(unzbd)?;

    std::fs::create_dir_all(&cache.path).map_err(|source| ExtractError::Io {
        path: cache.path.clone(),
        source,
    })?;

    let total = plan.jobs.len();
    let mut entries = Vec::new();
    let mut failures = Vec::new();

    for (index, job) in plan.jobs.iter().enumerate() {
        // An up-to-date job keeps its manifest entry: the output is still
        // there and still current, so dropping it would make the next run
        // redo the work.
        if job.up_to_date {
            progress(ExtractProgress::Skipped { job: index });
            if let Some((len, mtime)) = source_stamp(&job.source) {
                entries.push(entry_for(job, len, mtime));
            }
            continue;
        }

        progress(ExtractProgress::Started {
            job: index,
            total,
            kind: job.kind,
            source: job.source.clone(),
        });

        match run_job(job, unzbd, cache) {
            Ok(()) => {
                progress(ExtractProgress::Finished { job: index });
                if let Some((len, mtime)) = source_stamp(&job.source) {
                    entries.push(entry_for(job, len, mtime));
                }
            }
            Err(failure) => {
                progress(ExtractProgress::Failed {
                    job: index,
                    reason: failure.reason.clone(),
                });
                failures.push(failure);
            }
        }
    }

    let manifest = CacheManifest {
        mech3ax_version: version,
        mech3ax_commit: std::env::var("BFT_MECH3AX_COMMIT").ok(),
        created: now_rfc3339(),
        entries,
    };
    crate::cache::save_manifest(cache, &manifest)?;

    if failures.is_empty() {
        Ok(manifest)
    } else {
        Err(ExtractError::Jobs { failures, total })
    }
}

fn entry_for(job: &ExtractJob, source_len: u64, source_mtime: u64) -> CacheEntry {
    CacheEntry {
        kind: job.kind,
        source: job.source.clone(),
        output: job.output.clone(),
        source_len,
        source_mtime,
    }
}

/// Run one job, capturing stderr to `<output>.log`.
fn run_job(job: &ExtractJob, unzbd: &Path, cache: &CacheDir) -> Result<(), JobFailure> {
    let output_path = cache.path.join(&job.output);
    let fail = |reason: String, log: Option<PathBuf>| JobFailure {
        kind: job.kind,
        source: job.source.clone(),
        reason,
        log,
    };

    if let Some(parent) = output_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Err(fail(
            format!("cannot create {}: {e}", parent.display()),
            None,
        ));
    }

    let result = Command::new(unzbd)
        .arg("rc")
        .arg(job.kind.subcommand())
        .arg(&job.source)
        .arg(&output_path)
        // mech3ax logs its bounding-box diagnostics through `tracing`, which
        // is quiet unless RUST_LOG asks for it. docs/data/quirks.md catalogues
        // them; keeping them in the log makes a later mismatch traceable.
        .env("RUST_LOG", "info")
        .output();

    let out = match result {
        Ok(out) => out,
        Err(e) => return Err(fail(format!("cannot run unzbd: {e}"), None)),
    };

    let log = write_log(&output_path, &out.stderr);

    if out.status.success() {
        Ok(())
    } else {
        // The last stderr line is usually mech3ax's actual complaint; the
        // rest is `info` chatter that the log file keeps.
        let tail = String::from_utf8_lossy(&out.stderr)
            .lines()
            .next_back()
            .unwrap_or("no error output")
            .to_string();

        Err(fail(format!("{} — {tail}", exit_status(&out.status)), log))
    }
}

/// Describe an exit status without assuming a code exists (signals).
fn exit_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".to_string(),
    }
}

/// Write a job's stderr beside its output, returning the log's path.
///
/// Only diagnostics are kept. At `RUST_LOG=info` mech3ax narrates every job
/// with a `Reading`/`Wrote` pair, and writing 82 files to say "it worked"
/// would bury the ones that matter — docs/data/quirks.md's bounding-box
/// mismatches and duplicate anim refs.
///
/// Best-effort: a log that cannot be written must not turn a successful
/// extraction into a failure.
fn write_log(output_path: &Path, stderr: &[u8]) -> Option<PathBuf> {
    if stderr.is_empty() || !is_noteworthy(stderr) {
        return None;
    }

    let log = output_path.with_extension(format!(
        "{}.log",
        output_path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
    ));
    let mut file = std::fs::File::create(&log).ok()?;
    file.write_all(stderr).ok()?;

    Some(log)
}

/// Whether a job's stderr says anything beyond "I read it, I wrote it".
///
/// mech3ax's routine lines are `<KIND>: Reading \`…\`` and
/// `<KIND>: Wrote \`…\``; anything else — a bounding-box mismatch, a
/// duplicate anim ref, an error — is worth keeping.
fn is_noteworthy(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.contains(": Reading `") && !line.contains(": Wrote `")
    })
}

/// Ask `unzbd` what it is, for the manifest (FR-D4).
pub fn unzbd_version(unzbd: &Path) -> Result<String, ExtractError> {
    let out = Command::new(unzbd)
        .arg("--version")
        .output()
        .map_err(|source| ExtractError::Unzbd {
            unzbd: unzbd.to_path_buf(),
            source,
        })?;

    // `unzbd --version` prints to stdout; fall back to stderr in case a
    // future build changes that rather than recording an empty string.
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr)
    } else {
        String::from_utf8_lossy(&out.stdout)
    };

    Ok(text.trim().to_string())
}

/// Find `unzbd`: CLI, `BFT_UNZBD`, config, `PATH`, then the copy
/// `open-bft setup` builds ([`managed_unzbd`]).
pub fn find_unzbd(cli: Option<&Path>, config: Option<&Path>) -> Option<PathBuf> {
    let candidates = cli
        .map(Path::to_path_buf)
        .into_iter()
        .chain(std::env::var_os("BFT_UNZBD").map(PathBuf::from))
        .chain(config.map(Path::to_path_buf));

    for candidate in candidates {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    which_unzbd().or_else(|| managed_unzbd().filter(|path| path.is_file()))
}

/// The binary's file name on this platform.
const UNZBD_NAME: &str = if cfg!(windows) { "unzbd.exe" } else { "unzbd" };

/// The first `unzbd` on `PATH`.
fn which_unzbd() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(UNZBD_NAME))
        .find(|path| path.is_file())
}

/// The mech3ax repository `unzbd` is built from.
pub const MECH3AX_REPO: &str = "https://github.com/TerranMechworks/mech3ax";

/// The mech3ax commit the cache layout and the loaders are written against.
///
/// The same pin as docs/data/install.md; change both together.
pub const MECH3AX_COMMIT: &str = "cbb838fd8b628417abe61dee6132c350a2375f33";

/// Where `open-bft setup` installs the `unzbd` it builds: `cargo install`'s
/// `--root`, under the platform data directory.
pub fn managed_unzbd_root() -> Option<PathBuf> {
    crate::cache::project_dirs().map(|dirs| dirs.data_dir().join("tools"))
}

/// The binary `cargo install --root` puts under [`managed_unzbd_root`].
pub fn managed_unzbd() -> Option<PathBuf> {
    managed_unzbd_root().map(|root| root.join("bin").join(UNZBD_NAME))
}

/// The command that builds `unzbd` at [`MECH3AX_COMMIT`] into `root`.
///
/// `--locked` builds against mech3ax's own lockfile, so the result does not
/// drift as its dependencies release. Cargo builds in a temporary directory
/// and removes it afterwards, so only the binary stays.
pub fn unzbd_install_command(cargo: &Path, root: &Path) -> Command {
    let mut command = Command::new(cargo);
    command
        .args(["install", "--locked", "--git", MECH3AX_REPO, "--rev"])
        .arg(MECH3AX_COMMIT)
        .arg("--root")
        .arg(root)
        .arg("unzbd");

    command
}

/// The current time as RFC 3339 in UTC.
///
/// Hand-rolled from the Unix epoch because the workspace has no date crate
/// and the manifest only needs a human-readable stamp, not date arithmetic.
fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (days, time) = (secs / 86_400, secs % 86_400);
    let (hour, minute, second) = (time / 3600, (time % 3600) / 60, time % 60);
    let (year, month, day) = civil_from_days(days as i64);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's `civil_from_days`: a day count since 1970-01-01 to a
/// proleptic Gregorian date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };

    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheDir;
    use crate::test_support::{FakeRoot, TempDir};

    fn plan_for(fake: &FakeRoot, cache: &CacheDir) -> ExtractPlan {
        let root = fake.data_root();
        plan(&root, cache, None).unwrap()
    }

    fn outputs(plan: &ExtractPlan) -> Vec<String> {
        plan.jobs
            .iter()
            .map(|j| j.output.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn a_retail_tree_plans_every_documented_source() {
        let fake = FakeRoot::retail();
        let cache = TempDir::new();

        let plan = plan_for(&fake, &CacheDir::new(cache.path()));
        let outputs = outputs(&plan);

        // Globals.
        assert!(outputs.contains(&"interp.json".to_string()));
        assert!(outputs.contains(&"messages.json".to_string()));
        assert!(outputs.contains(&"reader/zrdr.rdr.zip".to_string()));
        assert!(outputs.contains(&"textures/image.tex.zip".to_string()));
        assert!(outputs.contains(&"sounds/soundsh.snd.zip".to_string()));
        // Per mission.
        assert!(outputs.contains(&"gamez/m1-gamez.gamez.zip".to_string()));
        assert!(outputs.contains(&"anim/m13-anim.anim.zip".to_string()));
        assert!(outputs.contains(&"reader/m7-zrdr.rdr.zip".to_string()));
        assert!(outputs.contains(&"textures/m1-rtexture4.tex.zip".to_string()));
        assert!(outputs.contains(&"textures/m1-rtexture2.tex.zip".to_string()));
        // Automaps: m1-m12 only.
        assert!(outputs.contains(&"zmap/m12.zmap.json".to_string()));
        assert!(!outputs.contains(&"zmap/m13.zmap.json".to_string()));
    }

    #[test]
    fn software_texture_tiers_are_not_extracted() {
        let fake = FakeRoot::retail();
        let cache = TempDir::new();

        let plan = plan_for(&fake, &CacheDir::new(cache.path()));

        // docs/data/materials.md: the `texture*` family is the software renderer's.
        assert!(
            !outputs(&plan).iter().any(|o| o.contains("-texture")),
            "software texture tiers should be skipped"
        );
    }

    #[test]
    fn only_the_high_fidelity_sound_archive_is_extracted() {
        let fake = FakeRoot::retail();
        let cache = TempDir::new();

        let plan = plan_for(&fake, &CacheDir::new(cache.path()));
        let sounds: Vec<_> = plan
            .jobs
            .iter()
            .filter(|j| j.kind == ExtractKind::Sounds)
            .collect();

        // docs/data/sounds.md: all three tiers hold the same 280 names.
        assert_eq!(sounds.len(), 1);
        assert!(sounds[0].source.ends_with("soundsh.zbd"));
    }

    #[test]
    fn every_output_carries_the_suffix_its_loader_will_key_on() {
        let fake = FakeRoot::retail();
        let cache = TempDir::new();

        for job in plan_for(&fake, &CacheDir::new(cache.path())).jobs {
            let output = job.output.to_string_lossy().to_string();
            let expected = match job.kind {
                ExtractKind::Gamez => ".gamez.zip",
                ExtractKind::Anim => ".anim.zip",
                ExtractKind::Textures => ".tex.zip",
                ExtractKind::Reader => ".rdr.zip",
                ExtractKind::Sounds => ".snd.zip",
                ExtractKind::ZMap => ".zmap.json",
                ExtractKind::Interp | ExtractKind::Messages => ".json",
            };
            assert!(output.ends_with(expected), "{output} should end {expected}");
        }
    }

    #[test]
    fn a_partial_install_plans_only_what_it_has() {
        let fake = FakeRoot::with_missions(1..=2);
        let cache = TempDir::new();

        let outputs = outputs(&plan_for(&fake, &CacheDir::new(cache.path())));

        assert!(outputs.contains(&"gamez/m2-gamez.gamez.zip".to_string()));
        assert!(!outputs.iter().any(|o| o.contains("m3")));
    }

    #[test]
    fn strings_dll_is_planned_when_messages_dll_is_absent() {
        let fake = FakeRoot::retail();
        std::fs::remove_file(fake.path().join("messages.dll")).unwrap();
        std::fs::write(fake.path().join("strings.dll"), []).unwrap();
        let cache = TempDir::new();

        let plan = plan_for(&fake, &CacheDir::new(cache.path()));
        let messages: Vec<_> = plan
            .jobs
            .iter()
            .filter(|j| j.kind == ExtractKind::Messages)
            .collect();

        assert_eq!(messages.len(), 1);
        assert!(messages[0].source.ends_with("strings.dll"));
        assert_eq!(messages[0].output, Path::new("messages.json"));
    }

    #[test]
    fn a_matching_manifest_and_output_make_a_job_up_to_date() {
        let fake = FakeRoot::retail();
        let cache_dir = TempDir::new();
        let cache = CacheDir::new(cache_dir.path());

        // Pretend a previous run extracted everything.
        let first = plan_for(&fake, &cache);
        let mut entries = Vec::new();
        for job in &first.jobs {
            let output = cache.path.join(&job.output);
            std::fs::create_dir_all(output.parent().unwrap()).unwrap();
            std::fs::write(&output, []).unwrap();
            let (len, mtime) = source_stamp(&job.source).unwrap();
            entries.push(entry_for(job, len, mtime));
        }
        let manifest = CacheManifest {
            mech3ax_version: "unzbd v0.0.0-test".to_string(),
            mech3ax_commit: None,
            created: "2000-01-01T00:00:00Z".to_string(),
            entries,
        };

        let second = plan(&fake.data_root(), &cache, Some(&manifest)).unwrap();

        assert_eq!(
            second.stale_count(),
            0,
            "a second run should skip everything"
        );
    }

    #[test]
    fn a_deleted_output_makes_its_job_stale_again() {
        let fake = FakeRoot::retail();
        let cache_dir = TempDir::new();
        let cache = CacheDir::new(cache_dir.path());

        let first = plan_for(&fake, &cache);
        let job = first
            .jobs
            .iter()
            .find(|j| j.kind == ExtractKind::Gamez)
            .unwrap();
        let (len, mtime) = source_stamp(&job.source).unwrap();
        let manifest = CacheManifest {
            mech3ax_version: "unzbd v0.0.0-test".to_string(),
            mech3ax_commit: None,
            created: "2000-01-01T00:00:00Z".to_string(),
            entries: vec![entry_for(job, len, mtime)],
        };

        // The manifest says it is done, but the file is not in the cache.
        let second = plan(&fake.data_root(), &cache, Some(&manifest)).unwrap();
        let same = second.jobs.iter().find(|j| j.output == job.output).unwrap();

        assert!(!same.up_to_date);
    }

    #[test]
    fn a_changed_source_makes_its_job_stale_again() {
        let fake = FakeRoot::retail();
        let cache_dir = TempDir::new();
        let cache = CacheDir::new(cache_dir.path());

        let first = plan_for(&fake, &cache);
        let job = first
            .jobs
            .iter()
            .find(|j| j.kind == ExtractKind::Gamez)
            .unwrap();
        let output = cache.path.join(&job.output);
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, []).unwrap();
        let (_, mtime) = source_stamp(&job.source).unwrap();
        let manifest = CacheManifest {
            mech3ax_version: "unzbd v0.0.0-test".to_string(),
            mech3ax_commit: None,
            created: "2000-01-01T00:00:00Z".to_string(),
            // A different length than the source actually has.
            entries: vec![entry_for(job, 99_999, mtime)],
        };

        let second = plan(&fake.data_root(), &cache, Some(&manifest)).unwrap();
        let same = second.jobs.iter().find(|j| j.output == job.output).unwrap();

        assert!(!same.up_to_date);
    }

    #[test]
    fn a_command_line_is_copy_pasteable() {
        let job = ExtractJob {
            kind: ExtractKind::Gamez,
            source: PathBuf::from("/games/my recoil/zbd/m1/gamez.zbd"),
            output: PathBuf::from("gamez/m1-gamez.gamez.zip"),
            up_to_date: false,
        };
        let cache = CacheDir::new("/cache");

        let line = job.command_line(Path::new("/usr/bin/unzbd"), &cache);

        assert_eq!(
            line,
            "/usr/bin/unzbd rc gamez '/games/my recoil/zbd/m1/gamez.zbd' /cache/gamez/m1-gamez.gamez.zip"
        );
    }

    #[test]
    fn only_diagnostics_are_worth_a_log_file() {
        // The routine pair every successful job emits at RUST_LOG=info.
        let routine = b"GAMEZ: Reading `/games/m1/gamez.zbd` (RC)\n\
                        GAMEZ: Wrote `/cache/gamez/m1-gamez.gamez.zip`\n";
        assert!(!is_noteworthy(routine));

        // docs/data/quirks.md's diagnostics, which must survive.
        let bbox = b"GAMEZ: Reading `/games/m2/gamez.zbd` (RC)\n\
                     LOD BBOX_NODE FAIL\n\
                     GAMEZ: Wrote `/cache/gamez/m2-gamez.gamez.zip`\n";
        assert!(is_noteworthy(bbox));

        let anim = b"anim def duplicate anim ref `boat_wake`.`left_wake`\n";
        assert!(is_noteworthy(anim));
    }

    #[test]
    fn an_unrunnable_unzbd_is_an_error_not_a_panic() {
        let err = unzbd_version(Path::new("/nonexistent/unzbd")).unwrap_err();

        assert!(matches!(err, ExtractError::Unzbd { .. }), "{err}");
    }

    #[test]
    fn the_install_command_builds_the_pinned_unzbd_into_the_root() {
        let command = unzbd_install_command(Path::new("cargo"), Path::new("/data/tools"));

        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert_eq!(command.get_program(), "cargo");
        assert_eq!(
            args,
            [
                "install",
                "--locked",
                "--git",
                MECH3AX_REPO,
                "--rev",
                MECH3AX_COMMIT,
                "--root",
                "/data/tools",
                "unzbd",
            ]
        );
    }

    #[test]
    fn the_pin_matches_the_install_doc() {
        let doc = include_str!("../../../docs/data/install.md");

        assert!(
            doc.contains(&format!("commit {MECH3AX_COMMIT}")),
            "docs/data/install.md pins a different mech3ax commit"
        );
    }

    #[test]
    fn the_managed_unzbd_lives_under_the_install_root() {
        let (Some(root), Some(binary)) = (managed_unzbd_root(), managed_unzbd()) else {
            return;
        };

        assert_eq!(binary.parent(), Some(root.join("bin").as_path()));
    }

    #[test]
    fn the_timestamp_is_rfc_3339() {
        let now = now_rfc3339();

        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z'));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
    }

    #[test]
    fn the_real_install_plans_the_documented_job_count() {
        let Some(root) = crate::test_support::data_root() else {
            return;
        };
        let root = crate::root::DataRoot {
            path: root,
            version: crate::root::DataVersion::Retail,
        };
        let cache = TempDir::new();

        let plan = plan(&root, &CacheDir::new(cache.path()), None).unwrap();

        // docs/data/install.md: 5 globals + 12 zmaps + 13 missions x (gamez, anim,
        // reader, rtexture4, rtexture2).
        assert_eq!(plan.jobs.len(), 5 + 12 + 13 * 5);
        assert_eq!(plan.stale_count(), plan.jobs.len());
    }
}
