//! `open-bft inspect`: what the loaders make of the data, as text, with no
//! window (FR-T2).
//!
//! Each target is loaded through the same `AssetServer` and loaders the game
//! uses, in a `MinimalPlugins` app polled until the asset is in, so what this
//! prints is what the game gets. The printers take any [`Write`], and a
//! closed pipe (`open-bft inspect level 1 | head`) ends the output quietly
//! instead of failing.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use bevy::asset::{AssetPath, LoadState};
use bevy::prelude::*;
use bft_assets::anim::AnimSet;
use bft_assets::axes::AxisConvention;
use bft_assets::catalog::{LevelCatalog, LevelEntry, LevelId};
use bft_assets::interp::InterpScripts;
use bft_assets::level::{LevelData, NodeFlagsDesc};
use bft_assets::messages::Messages;
use bft_assets::reader::ReaderData;
use bft_assets::textures::TextureSet;
use bft_assets::{AssetsPlugin, DataPaths};

use crate::level_view::level_draw_census;

/// How long one asset may take to load before `inspect` gives up. A level's
/// texture archive decodes several hundred PNGs; a minute is generous.
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// What to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectTarget {
    /// A level's node tree, and its model and material statistics.
    Level(LevelId),
    /// One texture archive: a level (`m1`) for its own, or an archive's file
    /// name or the start of it (`rtexture2`, `m1-rtexture4`).
    Textures(String),
    /// A reader archive's entries, or one entry as JSON. `None` is the
    /// global archive.
    Reader {
        level: Option<LevelId>,
        entry: Option<String>,
    },
    /// The interp scripts, or one mission's runtime bindings.
    Interp { level: Option<LevelId> },
    /// The message table, optionally only the keys or texts matching.
    Messages { filter: Option<String> },
    /// A level's anim definitions, or every definition with one name.
    Anim {
        level: LevelId,
        name: Option<String>,
    },
}

/// Why `inspect` could not print its target.
#[derive(Debug, thiserror::Error)]
pub enum InspectError {
    #[error("{0} is not in the catalog; run `open-bft extract`, or try m1 … m13")]
    UnknownLevel(LevelId),
    #[error("the extraction cache has no {0}; run `open-bft extract`")]
    NotExtracted(&'static str),
    #[error("no texture archive matches `{0}`; archives: {1}")]
    NoArchive(String, String),
    #[error("`{0}` matches several archives: {1}")]
    AmbiguousArchive(String, String),
    #[error("{0} has no entry `{1}`")]
    NoEntry(String, String),
    #[error("no anim definition is named `{0}`")]
    NoAnim(String),
    #[error("cannot load {0}: {1}")]
    Load(String, String),
    #[error("{0} did not finish loading within {1} s")]
    Timeout(String, u64),
    #[error("cannot write the output: {0}")]
    Io(#[from] io::Error),
}

/// Load the target and print it to `out`.
pub fn run(
    paths: DataPaths,
    axes: AxisConvention,
    target: &InspectTarget,
    out: &mut dyn Write,
) -> Result<(), InspectError> {
    let mut app = loader_app(paths);
    let catalog = app.world().resource::<LevelCatalog>().clone();

    let result = match target {
        InspectTarget::Level(id) => {
            let entry = level_entry(&catalog, *id)?;
            let level: LevelData = load(&mut app, entry.gamez.clone())?;
            print_level(out, entry, &level, &axes)
        }
        InspectTarget::Textures(name) => {
            let path = texture_archive(&catalog, name)?;
            let set: TextureSet = load(&mut app, path.clone())?;
            print_textures(out, &path, &set)
        }
        InspectTarget::Reader { level, entry } => {
            let (label, path) = match level {
                Some(id) => (id.to_string(), level_entry(&catalog, *id)?.reader.clone()),
                None => (
                    "the global reader".to_string(),
                    catalog
                        .global_reader
                        .clone()
                        .ok_or(InspectError::NotExtracted("global reader archive"))?,
                ),
            };
            let reader: ReaderData = load(&mut app, path)?;
            print_reader(out, &label, &reader, entry.as_deref())
        }
        InspectTarget::Interp { level } => {
            let path = catalog
                .interp
                .clone()
                .ok_or(InspectError::NotExtracted("interp.json"))?;
            let scripts: InterpScripts = load(&mut app, path)?;
            print_interp(out, &scripts, *level)
        }
        InspectTarget::Messages { filter } => {
            let path = catalog
                .messages
                .clone()
                .ok_or(InspectError::NotExtracted("messages table"))?;
            let messages: Messages = load(&mut app, path)?;
            print_messages(out, &messages, filter.as_deref())
        }
        InspectTarget::Anim { level, name } => {
            let entry = level_entry(&catalog, *level)?;
            let path = entry
                .anim
                .clone()
                .ok_or(InspectError::NotExtracted("anim archive for this level"))?;
            let anims: AnimSet = load(&mut app, path)?;
            print_anim(out, &anims, name.as_deref())
        }
    };

    match result {
        // `| head` closed the pipe: the reader has what it wanted.
        Err(InspectError::Io(e)) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

/// A windowless app with the game's loaders and the `cache://` source.
fn loader_app(paths: DataPaths) -> App {
    let mut app = App::new();
    // `AssetsPlugin` before `AssetPlugin`, which builds the `cache://`
    // source (the same order `bft-bin` uses everywhere).
    app.add_plugins(AssetsPlugin::new(paths))
        .add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
    // `App::run` would do these; a hand-polled app has to. `AssetsPlugin`
    // registers its loaders and the catalog in `finish`.
    app.finish();
    app.cleanup();
    app
}

/// Load one asset and take it out of the app, polling until it is in.
fn load<A: Asset>(app: &mut App, path: AssetPath<'static>) -> Result<A, InspectError> {
    let label = path.to_string();
    let handle: Handle<A> = app.world().resource::<AssetServer>().load(path);
    let started = Instant::now();

    loop {
        app.update();

        match app.world().resource::<AssetServer>().load_state(&handle) {
            LoadState::Loaded => break,
            LoadState::Failed(e) => return Err(InspectError::Load(label, e.to_string())),
            LoadState::NotLoaded | LoadState::Loading => {}
        }
        if started.elapsed() > LOAD_TIMEOUT {
            return Err(InspectError::Timeout(label, LOAD_TIMEOUT.as_secs()));
        }
        // The loaders run on the IO pool; there is nothing to do here but
        // wait for them.
        std::thread::sleep(Duration::from_millis(2));
    }

    app.world_mut()
        .resource_mut::<Assets<A>>()
        .remove(&handle)
        .ok_or_else(|| InspectError::Load(label, "loaded, then gone".into()))
}

fn level_entry(catalog: &LevelCatalog, id: LevelId) -> Result<&LevelEntry, InspectError> {
    catalog.level(id).ok_or(InspectError::UnknownLevel(id))
}

/// The archive a `textures` argument names.
fn texture_archive(catalog: &LevelCatalog, name: &str) -> Result<AssetPath<'static>, InspectError> {
    // A level name means that level's own archive, which the catalog lists
    // first (docs/data/materials.md).
    if let Ok(id) = name.parse::<LevelId>()
        && let Some(own) = catalog.level(id).and_then(|entry| entry.textures.first())
    {
        return Ok(own.clone());
    }

    let mut all: Vec<AssetPath<'static>> = catalog
        .levels
        .iter()
        .flat_map(|entry| entry.textures.iter().cloned())
        .chain(catalog.texture_pool.iter().cloned())
        .collect();
    all.sort_by_key(ToString::to_string);
    all.dedup();

    let stem = |path: &AssetPath| {
        path.path()
            .file_name()
            .map(|name| {
                name.to_string_lossy()
                    .trim_end_matches(".tex.zip")
                    .to_string()
            })
            .unwrap_or_default()
    };
    let names = |paths: &[&AssetPath<'static>]| {
        paths
            .iter()
            .map(|path| stem(path))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let wanted = name.to_lowercase();
    if let Some(exact) = all.iter().find(|path| stem(path) == wanted) {
        return Ok(exact.clone());
    }
    let matches: Vec<&AssetPath<'static>> = all
        .iter()
        .filter(|path| stem(path).contains(&wanted))
        .collect();

    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(InspectError::NoArchive(
            name.to_string(),
            names(&all.iter().collect::<Vec<_>>()),
        )),
        several => Err(InspectError::AmbiguousArchive(
            name.to_string(),
            names(several),
        )),
    }
}

/// The node flags that are set, by short name.
///
/// `active` is left out when set, because every node of every shipped level
/// has it (docs/data/nodes.md); `inactive` is printed instead when it is not.
pub fn flag_names(flags: &NodeFlagsDesc) -> Vec<&'static str> {
    let named = [
        (!flags.active, "inactive"),
        (flags.altitude_surface, "altitude"),
        (flags.intersect_surface, "intersect"),
        (flags.intersect_bbox, "bbox-hit"),
        (flags.proximity, "proximity"),
        (flags.landmark, "landmark"),
        (flags.bbox_node, "bbox-node"),
        (flags.bbox_model, "bbox-model"),
        (flags.bbox_child, "bbox-child"),
        (flags.terrain, "terrain"),
        (flags.can_modify, "deformable"),
        (flags.clip_to, "clip-to"),
        (flags.tree_valid, "tree-valid"),
        (flags.id_zone_check, "zone-check"),
    ];

    named
        .into_iter()
        .filter_map(|(set, name)| set.then_some(name))
        .collect()
}

/// Statistics first, then the node tree.
pub fn print_level(
    out: &mut dyn Write,
    entry: &LevelEntry,
    level: &LevelData,
    axes: &AxisConvention,
) -> Result<(), InspectError> {
    let census = level.census();
    let draw = level_draw_census(level, axes);

    writeln!(out, "{} ({})", entry.name, entry.gamez)?;
    writeln!(
        out,
        "nodes     {} in {} root trees: {} object3d, {} lod, {} empty, {} light, {} world, \
         {} camera, {} display, {} window",
        level.nodes.len(),
        level.root_nodes.len(),
        census.object3d,
        census.lod,
        census.empty,
        census.light,
        census.world,
        census.camera,
        census.display,
        census.window,
    )?;
    writeln!(
        out,
        "models    {} ({} referenced, {} with no geometry), {} triangles drawn by {} references",
        level.models.len(),
        draw.distinct_models,
        draw.empty_models,
        draw.triangles,
        draw.model_refs,
    )?;

    let textured = level
        .materials
        .iter()
        .filter(|material| material.texture().is_some())
        .count();
    let mut soils: BTreeMap<String, usize> = BTreeMap::new();
    for material in &level.materials {
        *soils.entry(format!("{:?}", material.soil())).or_default() += 1;
    }
    writeln!(
        out,
        "materials {} ({textured} textured, {} coloured); soils: {}",
        level.materials.len(),
        level.materials.len() - textured,
        soils
            .iter()
            .map(|(soil, count)| format!("{soil} {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    writeln!(out, "textures  {} names", level.texture_names.len())?;
    writeln!(out)?;

    for root in tree_roots(level) {
        print_subtree(out, level, root, 0, &children_by_parent(level))?;
    }

    Ok(())
}

/// The world's tree first, then the prototypes, in id order.
fn tree_roots(level: &LevelData) -> Vec<u32> {
    let mut roots: Vec<u32> = (0..level.nodes.len() as u32)
        .filter(|id| {
            level
                .node(*id)
                .is_some_and(|node| node.common().parent.is_none())
        })
        .collect();
    roots.sort_by_key(|id| (*id != level.world_node, *id));
    roots
}

/// Each node's children, from the parent links.
///
/// Parents, not `children`, because `child_indices` omits hundreds of edges
/// per level (docs/data/nodes.md); `children` only orders siblings.
fn children_by_parent(level: &LevelData) -> BTreeMap<u32, Vec<u32>> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();

    for (id, node) in level.nodes.iter().enumerate() {
        if let Some(parent) = node.common().parent {
            children.entry(parent).or_default().push(id as u32);
        }
    }

    for (parent, list) in children.iter_mut() {
        let order = level
            .node(*parent)
            .map(|node| node.common().children.clone())
            .unwrap_or_default();
        list.sort_by_key(|child| {
            (
                order.iter().position(|c| c == child).unwrap_or(usize::MAX),
                *child,
            )
        });
    }

    children
}

fn print_subtree(
    out: &mut dyn Write,
    level: &LevelData,
    id: u32,
    depth: usize,
    children: &BTreeMap<u32, Vec<u32>>,
) -> Result<(), InspectError> {
    let Some(node) = level.node(id) else {
        return Ok(());
    };
    let common = node.common();

    let mut line = format!(
        "{:indent$}{}  #{id} {}",
        "",
        common.name,
        node.kind(),
        indent = depth * 2
    );
    if let Some(model) = common.model_index {
        line.push_str(&format!(" model={model}"));
    }
    if let Some(zone) = common.zone_id {
        line.push_str(&format!(" zone={zone}"));
    }
    let flags = flag_names(&common.flags);
    if !flags.is_empty() {
        line.push_str(&format!("  [{}]", flags.join(" ")));
    }
    writeln!(out, "{line}")?;

    for child in children.get(&id).into_iter().flatten() {
        print_subtree(out, level, *child, depth + 1, children)?;
    }

    Ok(())
}

/// Every texture: size, alpha, stretch.
pub fn print_textures(
    out: &mut dyn Write,
    path: &AssetPath,
    set: &TextureSet,
) -> Result<(), InspectError> {
    let mut alphas: BTreeMap<String, usize> = BTreeMap::new();
    for info in set.info.values() {
        *alphas.entry(format!("{:?}", info.alpha)).or_default() += 1;
    }

    writeln!(out, "{path}: {} textures", set.info.len())?;
    writeln!(
        out,
        "alpha: {}",
        alphas
            .iter()
            .map(|(alpha, count)| format!("{alpha} {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    writeln!(out)?;

    let mut names: Vec<&String> = set.info.keys().collect();
    names.sort();
    for name in names {
        let info = &set.info[name];
        writeln!(
            out,
            "{name:24} {:>4}x{:<4} {:?} {:?}",
            info.width, info.height, info.alpha, info.stretch
        )?;
    }

    Ok(())
}

/// The entry list, or one entry as pretty JSON.
pub fn print_reader(
    out: &mut dyn Write,
    label: &str,
    reader: &ReaderData,
    entry: Option<&str>,
) -> Result<(), InspectError> {
    match entry {
        Some(name) => {
            // `.zrd` is optional on the command line.
            let value = reader
                .entry(name)
                .or_else(|| reader.entry(&format!("{name}.zrd")))
                .ok_or_else(|| InspectError::NoEntry(label.to_string(), name.to_string()))?;
            let text = serde_json::to_string_pretty(value)
                .map_err(|e| InspectError::Load(name.to_string(), e.to_string()))?;
            writeln!(out, "{text}")?;
        }
        None => {
            writeln!(out, "{label}: {} entries", reader.len())?;
            for name in reader.names() {
                writeln!(out, "  {name}")?;
            }
        }
    }

    Ok(())
}

/// The script list, or one mission's runtime bindings.
pub fn print_interp(
    out: &mut dyn Write,
    scripts: &InterpScripts,
    level: Option<LevelId>,
) -> Result<(), InspectError> {
    match level {
        Some(id) => writeln!(out, "{:#?}", scripts.bindings(id.0))?,
        None => {
            writeln!(out, "{} scripts", scripts.scripts.len())?;
            for script in &scripts.scripts {
                writeln!(out, "  {:40} {} lines", script.name, script.lines.len())?;
            }
        }
    }

    Ok(())
}

/// `KEY = text`, sorted by key.
pub fn print_messages(
    out: &mut dyn Write,
    messages: &Messages,
    filter: Option<&str>,
) -> Result<(), InspectError> {
    let needle = filter.map(str::to_lowercase);
    let mut entries: Vec<(&String, &String)> = messages
        .entries
        .iter()
        .filter(|(key, text)| {
            needle.as_ref().is_none_or(|needle| {
                key.to_lowercase().contains(needle) || text.to_lowercase().contains(needle)
            })
        })
        .collect();
    entries.sort();

    writeln!(
        out,
        "language {}: {} of {} messages",
        messages.language_id,
        entries.len(),
        messages.entries.len()
    )?;
    for (key, text) in entries {
        writeln!(out, "{key} = {text:?}")?;
    }

    Ok(())
}

/// One line per definition, or every definition of one name in full.
pub fn print_anim(
    out: &mut dyn Write,
    anims: &AnimSet,
    name: Option<&str>,
) -> Result<(), InspectError> {
    match name {
        Some(name) => {
            let defs = anims.lookup(name);
            if defs.is_empty() {
                return Err(InspectError::NoAnim(name.to_string()));
            }
            for index in defs {
                writeln!(out, "#{index} {:#?}", anims.defs[*index])?;
            }
        }
        None => {
            writeln!(
                out,
                "{} definitions, {} .zan scripts, gravity {}",
                anims.defs.len(),
                anims.si_scripts.len(),
                anims.gravity
            )?;
            let mut kinds: Vec<(&&str, &usize)> = anims.event_counts.iter().collect();
            kinds.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            writeln!(
                out,
                "events: {}",
                kinds
                    .iter()
                    .map(|(kind, count)| format!("{kind} {count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
            writeln!(out)?;
            for (index, def) in anims.defs.iter().enumerate() {
                writeln!(
                    out,
                    "#{index:<5} {:32} on {:24} {:?} {:?}  {} seq, {} prereq",
                    def.anim_name,
                    def.name,
                    def.activation,
                    def.execution,
                    def.sequences.len(),
                    def.prereqs.len()
                )?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::level::parse_gamez;
    use bft_assets::test_support::minimal_gamez;

    fn entry() -> LevelEntry {
        LevelEntry {
            id: LevelId(1),
            name: "m1".into(),
            title_key: String::new(),
            gamez: AssetPath::from("cache://gamez/m1-gamez.gamez.zip").into_owned(),
            anim: None,
            reader: AssetPath::from("cache://reader/m1.rdr.zip").into_owned(),
            zmap: None,
            zmap_is_placeholder: false,
            textures: Vec::new(),
            kind: bft_assets::catalog::LevelKind::Unknown,
        }
    }

    /// The fixture's world node heads the tree, with its child indented.
    #[test]
    fn a_level_prints_its_statistics_then_its_tree() {
        let level = parse_gamez(&minimal_gamez(), "test").expect("the fixture loads");
        let mut out = Vec::new();

        print_level(&mut out, &entry(), &level, &AxisConvention::GAMEZ_DEFAULT).expect("prints");
        let text = String::from_utf8(out).expect("utf-8");
        let lines: Vec<&str> = text.lines().collect();

        assert!(lines[0].starts_with("m1 "), "{text}");
        assert!(lines[1].starts_with("nodes     3 "), "{text}");

        let tree: Vec<&str> = lines
            .iter()
            .skip_while(|l| !l.is_empty())
            .skip(1)
            .copied()
            .collect();
        assert_eq!(tree.len(), 3, "one line per node: {text}");
        assert!(tree[0].contains("#0 World"), "{text}");
        assert!(
            tree[1].starts_with("  ") && tree[1].contains("#1"),
            "{text}"
        );
        assert!(
            tree[2].starts_with("    ") && tree[2].contains("model=0"),
            "{text}"
        );
    }

    /// `| head` closing the pipe is success, not an error.
    #[test]
    fn a_closed_pipe_is_not_an_error() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let level = parse_gamez(&minimal_gamez(), "test").expect("the fixture loads");
        let result = print_level(
            &mut Closed,
            &entry(),
            &level,
            &AxisConvention::GAMEZ_DEFAULT,
        );

        assert!(
            matches!(&result, Err(InspectError::Io(e)) if e.kind() == io::ErrorKind::BrokenPipe),
            "the printer reports it, so `run` can swallow it"
        );
    }

    #[test]
    fn only_the_unusual_flags_are_named() {
        let flags = NodeFlagsDesc {
            active: true,
            terrain: true,
            ..default()
        };
        assert_eq!(flag_names(&flags), vec!["terrain"]);

        let hidden = NodeFlagsDesc {
            active: false,
            ..default()
        };
        assert_eq!(flag_names(&hidden), vec!["inactive"]);
    }

    #[test]
    fn messages_filter_on_key_and_text() {
        let messages = Messages {
            language_id: 1033,
            entries: [
                ("MSG_A".to_string(), "Hello".to_string()),
                ("MSG_B".to_string(), "World".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let mut out = Vec::new();

        print_messages(&mut out, &messages, Some("world")).expect("prints");
        let text = String::from_utf8(out).expect("utf-8");

        assert!(text.contains("1 of 2"), "{text}");
        assert!(text.contains("MSG_B = \"World\""), "{text}");
        assert!(!text.contains("MSG_A"), "{text}");
    }
}
