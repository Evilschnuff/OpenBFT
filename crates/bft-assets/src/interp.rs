//! The runtime subset of the interp scripts, extracted as data.
//!
//! `interp.zbd` holds 122 build-time scene-compiler scripts whose output is
//! the shipped `gamez.zbd`; almost all of it is already baked and must be
//! ignored. Only the `mN_zbd.gs` path runs at level load, and all it does is
//! bind texture animations by node name, plus a handful of constants
//! (docs/data/interp.md).
//!
//! There is **no interpreter**. [`runtime_bindings`] follows `source`,
//! expands `%VAR%` from `set`, tracks the current `FindNode`/`FindSubNode`
//! target and emits [`TextureAnimBinding`]s. Every other verb is counted and
//! dropped.
//!
//! This lives here, beside the texture loader, because cycling and scrolling
//! have no other source: no material in any level has `CycleData` and every model's
//! `texture_scroll` is zero, yet the runtime script binds 215 cycle sets and
//! 157 scrolls across the 13 missions.

use std::collections::HashMap;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::*;

use crate::mech3ax::Script;
use crate::textures::texture_key;

/// Every script in `interp.json`, as the asset server hands it over.
///
/// A whole-file asset rather than one per script: the file is 119 KB and
/// [`runtime_bindings`] needs the lot, because `source` follows references
/// between scripts.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct InterpScripts {
    /// The scripts, in file order.
    pub scripts: Vec<Script>,
}

impl InterpScripts {
    /// The runtime bindings for one mission, 1-based.
    pub fn bindings(&self, mission: u16) -> RuntimeBindings {
        runtime_bindings(&self.scripts, mission)
    }
}

/// What can go wrong reading `interp.json`.
#[derive(Debug, thiserror::Error)]
pub enum InterpError {
    #[error("cannot read interp.json: {0}")]
    Io(#[from] std::io::Error),
    #[error("interp.json is not the array of scripts this loader expects: {0}")]
    Json(#[from] serde_json::Error),
}

/// Loads `cache://interp.json`.
#[derive(TypePath, Debug, Default)]
pub struct InterpScriptsLoader;

impl AssetLoader for InterpScriptsLoader {
    type Asset = InterpScripts;
    type Settings = ();
    type Error = InterpError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _context: &mut LoadContext<'_>,
    ) -> Result<InterpScripts, InterpError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        Ok(InterpScripts {
            scripts: serde_json::from_slice(&bytes)?,
        })
    }

    fn extensions(&self) -> &[&str] {
        // `interp.json` is the only `.json` in the cache root; the zmaps live
        // under `zmap/` and carry `.zmap.json`.
        &["json"]
    }
}

/// A texture animation bound to a node by name.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureAnimBinding {
    /// The `FindNode` target, already `%VAR%`-expanded.
    pub node: String,
    /// The `FindSubNode` target, when the binding was narrowed to one.
    pub sub_node: Option<String>,
    pub anim: TextureAnim,
}

/// What the script asks a node's material to do.
#[derive(Debug, Clone, PartialEq)]
pub enum TextureAnim {
    /// `Object3DSetScroll on u v`: UV units per second.
    Scroll { u: f32, v: f32 },
    /// `CycleTextureSetOn`/`Speed`/`Looping`/`Map`: a frame list.
    Cycle {
        /// Frame names, normalised by [`texture_key`].
        frames: Vec<String>,
        /// Frames per second. `0.0` means the game advances the frame, not
        /// time: the player's tank tracks are a 4-frame set at speed 0.
        fps: f32,
        looping: bool,
    },
}

/// Everything the runtime script path yields for one mission.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuntimeBindings {
    /// Texture scroll and cycle bindings, in script order.
    pub texture_anims: Vec<TextureAnimBinding>,
    /// `WeaponSetMaxTetherAltitude`, in metres. 30.0 in all 13 missions;
    /// the tether-guided missiles' ceiling.
    pub max_tether_altitude: Option<f32>,
    /// The well-known node names (`playerName` → `bft_00`, `worldName` →
    /// `world1`, …) as this mission's scripts set them. Identical across all
    /// 13 missions, but read rather than assumed.
    pub names: HashMap<String, String>,
    /// Verbs seen and ignored, with counts. Kept so a mission that suddenly
    /// uses something new is visible rather than silently dropped.
    pub ignored: HashMap<String, usize>,
}

impl RuntimeBindings {
    /// The value of a well-known name, e.g. `"playerName"`.
    pub fn name(&self, key: &str) -> Option<&str> {
        self.names.get(key).map(String::as_str)
    }
}

/// The verbs [`runtime_bindings`] acts on. Everything else is counted.
const KNOWN_VERBS: [&str; 10] = [
    "source",
    "set",
    "FindNode",
    "FindSubNode",
    "Object3DSetScroll",
    "CycleTextureSetOn",
    "CycleTextureSetSpeed",
    "CycleTextureSetLooping",
    "CycleTextureSetMap",
    "WeaponSetMaxTetherAltitude",
];

/// Extract the runtime bindings for one mission.
///
/// `scripts` is the whole of `interp.json`; `mission` is 1-based, selecting
/// the `mN_zbd.gs` entry point. Returns an empty set (and warns) when the
/// mission has no runtime script.
pub fn runtime_bindings(scripts: &[Script], mission: u16) -> RuntimeBindings {
    let by_name: HashMap<String, &Script> = scripts
        .iter()
        .map(|script| (script_key(&script.name), script))
        .collect();

    let entry = format!("m{mission}_zbd.gs");
    if !by_name.contains_key(&script_key(&entry)) {
        warn!("interp has no {entry}; mission {mission} gets no texture animations");
        return RuntimeBindings::default();
    }

    let mut parser = Parser::default();
    parser.run(&by_name, &entry);
    parser.finish()
}

/// Normalise a script name for lookup: lowercase, forward slashes.
///
/// `source` writes DOS paths (`support\tex_fx.gw`) that must match the
/// `name` field of the script they refer to.
fn script_key(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

/// The parser's state while walking one mission's script tree.
#[derive(Default)]
struct Parser {
    bindings: RuntimeBindings,
    /// `FindNode` target.
    node: Option<String>,
    /// `FindSubNode` target, cleared by the next `FindNode`.
    sub_node: Option<String>,
    /// The cycle being accumulated, if `CycleTextureSetOn` opened one.
    cycle: Option<PendingCycle>,
    /// Scripts already entered, so a `source` cycle cannot loop forever.
    visited: Vec<String>,
}

/// A cycle between its `CycleTextureSetOn` and the next node change.
struct PendingCycle {
    node: String,
    sub_node: Option<String>,
    /// The count `CycleTextureSetOn` declared, checked against the frames.
    declared: usize,
    frames: Vec<String>,
    fps: f32,
    /// `None` until `CycleTextureSetLooping` says; 13 cycles never do.
    looping: Option<bool>,
}

impl Parser {
    fn run(&mut self, scripts: &HashMap<String, &Script>, name: &str) {
        let key = script_key(name);

        // `source` forms a tree in this data, but a cycle would hang us.
        if self.visited.contains(&key) {
            return;
        }
        let Some(script) = scripts.get(&key) else {
            warn!("interp script {name} is sourced but not present; skipping it");
            return;
        };
        self.visited.push(key);

        for line in &script.lines {
            let expanded = self.expand(line);
            let mut words = expanded.split_whitespace();
            let Some(verb) = words.next() else {
                continue;
            };
            let args: Vec<&str> = words.collect();

            match verb {
                "source" => {
                    if let Some(path) = args.first() {
                        // The sourced file's own lines finish before ours
                        // resume, and a `FindNode` never spans the boundary
                        // in this data — but flush anyway so it cannot.
                        self.flush_cycle();
                        self.run(scripts, path);
                    }
                }
                "set" => {
                    if let (Some(key), Some(value)) = (args.first(), args.get(1)) {
                        self.bindings
                            .names
                            .insert((*key).to_string(), (*value).to_string());
                    }
                }
                "FindNode" => {
                    self.flush_cycle();
                    self.node = args.first().map(|name| (*name).to_string());
                    self.sub_node = None;
                }
                "FindSubNode" => {
                    self.flush_cycle();
                    self.sub_node = args.first().map(|name| (*name).to_string());
                }
                "Object3DSetScroll" => self.scroll(&args),
                "CycleTextureSetOn" => self.cycle_on(&args),
                "CycleTextureSetSpeed" => self.cycle_speed(&args),
                "CycleTextureSetLooping" => self.cycle_looping(&args),
                "CycleTextureSetMap" => self.cycle_map(&args),
                "WeaponSetMaxTetherAltitude" => {
                    if let Some(value) = args.first().and_then(|a| a.parse().ok()) {
                        self.bindings.max_tether_altitude = Some(value);
                    }
                }
                _ => {
                    debug_assert!(!KNOWN_VERBS.contains(&verb));
                    *self.bindings.ignored.entry(verb.to_string()).or_default() += 1;
                }
            }
        }

        // A script's last cycle ends with the script (every `tex_fx*.gw`
        // finishes mid-cycle, before `Quit`).
        self.flush_cycle();
    }

    /// `Object3DSetScroll on|off u v`.
    fn scroll(&mut self, args: &[&str]) {
        let Some(node) = self.node.clone() else {
            warn!("Object3DSetScroll with no FindNode before it; ignoring");
            return;
        };
        // `off` never appears in the data, but honouring it is one branch.
        if args.first() != Some(&"on") {
            return;
        }
        let (Some(u), Some(v)) = (
            args.get(1).and_then(|a| a.parse().ok()),
            args.get(2).and_then(|a| a.parse().ok()),
        ) else {
            warn!("Object3DSetScroll on {node} without two rates; ignoring");
            return;
        };

        self.bindings.texture_anims.push(TextureAnimBinding {
            node,
            sub_node: self.sub_node.clone(),
            anim: TextureAnim::Scroll { u, v },
        });
    }

    /// `CycleTextureSetOn <frame count>` opens a cycle.
    fn cycle_on(&mut self, args: &[&str]) {
        self.flush_cycle();

        let Some(node) = self.node.clone() else {
            warn!("CycleTextureSetOn with no FindNode before it; ignoring");
            return;
        };

        self.cycle = Some(PendingCycle {
            node,
            sub_node: self.sub_node.clone(),
            declared: args.first().and_then(|a| a.parse().ok()).unwrap_or(0),
            frames: Vec::new(),
            fps: 0.0,
            looping: None,
        });
    }

    fn cycle_speed(&mut self, args: &[&str]) {
        let Some(cycle) = self.cycle.as_mut() else {
            warn!("CycleTextureSetSpeed outside a cycle; ignoring");
            return;
        };
        if let Some(fps) = args.first().and_then(|a| a.parse().ok()) {
            cycle.fps = fps;
        }
    }

    fn cycle_looping(&mut self, args: &[&str]) {
        let Some(cycle) = self.cycle.as_mut() else {
            warn!("CycleTextureSetLooping outside a cycle; ignoring");
            return;
        };
        cycle.looping = Some(args.first() == Some(&"on"));
    }

    fn cycle_map(&mut self, args: &[&str]) {
        let Some(cycle) = self.cycle.as_mut() else {
            warn!("CycleTextureSetMap outside a cycle; ignoring");
            return;
        };
        if let Some(name) = args.first() {
            cycle.frames.push(texture_key(name));
        }
    }

    /// Close the open cycle, if any, and emit it.
    fn flush_cycle(&mut self) {
        let Some(cycle) = self.cycle.take() else {
            return;
        };

        if cycle.frames.is_empty() {
            warn!("CycleTextureSetOn {} with no frames; dropping", cycle.node);
            return;
        }
        // Consistent in all 13 missions, so a mismatch means the grammar is
        // not what we think it is.
        if cycle.declared != cycle.frames.len() {
            warn!(
                "{}: CycleTextureSetOn declared {} frames but {} followed; using the {} listed",
                cycle.node,
                cycle.declared,
                cycle.frames.len(),
                cycle.frames.len()
            );
        }

        self.bindings.texture_anims.push(TextureAnimBinding {
            node: cycle.node,
            sub_node: cycle.sub_node,
            anim: TextureAnim::Cycle {
                frames: cycle.frames,
                fps: cycle.fps,
                // 13 cycles omit `CycleTextureSetLooping`. Not looping is
                // the safe reading: a one-shot that stops is a missing
                // effect, a loop that should have stopped is a visible bug.
                looping: cycle.looping.unwrap_or(false),
            },
        });
    }

    /// Replace every `%VAR%` with what `set` gave it.
    ///
    /// An unset variable is left as written: `%MissionZBDFile%` is set by a
    /// script we do not follow, and leaving it visible beats an empty node
    /// name that silently matches nothing.
    fn expand(&self, line: &str) -> String {
        if !line.contains('%') {
            return line.to_string();
        }

        let mut out = String::with_capacity(line.len());
        let mut rest = line;

        while let Some(start) = rest.find('%') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];

            match after.find('%') {
                Some(end) => {
                    let name = &after[..end];
                    match self.bindings.names.get(name) {
                        Some(value) => out.push_str(value),
                        None => {
                            out.push('%');
                            out.push_str(name);
                            out.push('%');
                        }
                    }
                    rest = &after[end + 1..];
                }
                None => {
                    // An unpaired `%`: emit it and stop looking.
                    out.push('%');
                    rest = after;
                    break;
                }
            }
        }
        out.push_str(rest);

        out
    }

    fn finish(self) -> RuntimeBindings {
        self.bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scripts in the shape `interp.json` has, with invented contents.
    fn script(name: &str, lines: &[&str]) -> Script {
        Script {
            name: name.to_string(),
            lines: lines.iter().map(|line| (*line).to_string()).collect(),
        }
    }

    #[test]
    fn a_missing_mission_script_is_empty_not_a_panic() {
        let bindings = runtime_bindings(&[], 1);

        assert_eq!(bindings, RuntimeBindings::default());
    }

    #[test]
    fn source_is_followed_and_variables_expand() {
        let scripts = vec![
            script(
                "m1_zbd.gs",
                &[
                    "source support\\common.gw",
                    "source support\\tex.gw",
                    "Quit",
                ],
            ),
            script("support\\common.gw", &["set playerName bft_00"]),
            script(
                "support\\tex.gw",
                &[
                    "FindNode %playerName%",
                    "FindSubNode arc_scroll",
                    "Object3DSetScroll on 0.0 12.0",
                ],
            ),
        ];

        let bindings = runtime_bindings(&scripts, 1);

        assert_eq!(bindings.name("playerName"), Some("bft_00"));
        assert_eq!(
            bindings.texture_anims,
            vec![TextureAnimBinding {
                node: "bft_00".to_string(),
                sub_node: Some("arc_scroll".to_string()),
                anim: TextureAnim::Scroll { u: 0.0, v: 12.0 },
            }]
        );
    }

    #[test]
    fn a_cycle_accumulates_until_the_next_node() {
        let scripts = vec![script(
            "m1_zbd.gs",
            &[
                "FindNode surf1",
                "CycleTextureSetOn 3",
                "CycleTextureSetLooping on",
                "CycleTextureSetSpeed 6.0",
                "CycleTextureSetMap surf01.tif",
                "CycleTextureSetMap surf02.tif",
                "CycleTextureSetMap surf03.tif",
                "FindNode gullfly",
                "CycleTextureSetOn 1",
                "CycleTextureSetSpeed 12",
                "CycleTextureSetMap gfly01.tif",
                "Quit",
            ],
        )];

        let bindings = runtime_bindings(&scripts, 1);

        assert_eq!(
            bindings.texture_anims,
            vec![
                TextureAnimBinding {
                    node: "surf1".to_string(),
                    sub_node: None,
                    anim: TextureAnim::Cycle {
                        frames: vec![
                            "surf01".to_string(),
                            "surf02".to_string(),
                            "surf03".to_string(),
                        ],
                        fps: 6.0,
                        looping: true,
                    },
                },
                TextureAnimBinding {
                    node: "gullfly".to_string(),
                    sub_node: None,
                    // No `CycleTextureSetLooping`: not looping.
                    anim: TextureAnim::Cycle {
                        frames: vec!["gfly01".to_string()],
                        fps: 12.0,
                        looping: false,
                    },
                },
            ]
        );
    }

    #[test]
    fn a_sub_node_narrows_only_until_the_next_find_node() {
        let scripts = vec![script(
            "m1_zbd.gs",
            &[
                "FindNode wake_trail",
                "FindSubNode wakepoly",
                "CycleTextureSetOn 1",
                "CycleTextureSetMap turb01.tif",
                "FindNode hscroll1",
                "Object3DSetScroll on 0.0 3.5",
            ],
        )];

        let bindings = runtime_bindings(&scripts, 1);

        assert_eq!(
            bindings.texture_anims[0].sub_node,
            Some("wakepoly".to_string())
        );
        assert_eq!(bindings.texture_anims[1].sub_node, None);
    }

    #[test]
    fn a_zero_speed_cycle_survives_as_zero() {
        // The player's tank tracks: the game advances the frame, not time.
        let scripts = vec![script(
            "m1_zbd.gs",
            &[
                "FindNode bft_00",
                "FindSubNode rtracks",
                "CycleTextureSetOn 2",
                "CycleTextureSetSpeed 0.0",
                "CycleTextureSetLooping off",
                "CycleTextureSetMap tanktrak.tif",
                "CycleTextureSetMap tanktrak1.tif",
            ],
        )];

        let bindings = runtime_bindings(&scripts, 1);

        assert_eq!(
            bindings.texture_anims[0].anim,
            TextureAnim::Cycle {
                frames: vec!["tanktrak".to_string(), "tanktrak1".to_string()],
                fps: 0.0,
                looping: false,
            }
        );
    }

    #[test]
    fn unknown_verbs_are_counted_rather_than_dropped_silently() {
        let scripts = vec![script(
            "m1_zbd.gs",
            &[
                "SetMission 1",
                "GameZReadZBDFile %MissionZBDFile%",
                "VideoSetDither on",
                "SetMission 1",
            ],
        )];

        let bindings = runtime_bindings(&scripts, 1);

        assert_eq!(bindings.ignored.get("SetMission"), Some(&2));
        assert_eq!(bindings.ignored.get("GameZReadZBDFile"), Some(&1));
        assert!(bindings.texture_anims.is_empty());
    }

    #[test]
    fn the_tether_altitude_is_picked_up() {
        let scripts = vec![script("m1_zbd.gs", &["WeaponSetMaxTetherAltitude 30.0"])];

        assert_eq!(
            runtime_bindings(&scripts, 1).max_tether_altitude,
            Some(30.0)
        );
    }

    #[test]
    fn an_unset_variable_stays_visible() {
        let scripts = vec![script(
            "m1_zbd.gs",
            &["FindNode %neverSet%", "Object3DSetScroll on 1.0 0.0"],
        )];

        let bindings = runtime_bindings(&scripts, 1);

        assert_eq!(bindings.texture_anims[0].node, "%neverSet%");
    }

    #[test]
    fn a_source_cycle_terminates() {
        let scripts = vec![
            script("m1_zbd.gs", &["source a.gw"]),
            script(
                "a.gw",
                &["source b.gw", "FindNode n", "Object3DSetScroll on 1.0 2.0"],
            ),
            script("b.gw", &["source a.gw"]),
        ];

        let bindings = runtime_bindings(&scripts, 1);

        assert_eq!(bindings.texture_anims.len(), 1);
    }
}
