//! Helpers for tests that need a real Recoil install.
//!
//! No game data is committed to this repo, ever, so data-backed tests cannot
//! assume their input exists. They ask for [`data_root`] and return early when
//! it is `None` — skipping, not failing, because a contributor without the
//! game must still be able to run `cargo test --workspace` green.

use std::path::PathBuf;

/// The Recoil install directory from `BFT_DATA_DIR`, or `None` when unset.
///
/// ```no_run
/// # fn parses_a_level() {
/// let Some(root) = bft_assets::test_support::data_root() else {
///     return;
/// };
/// # let _ = root;
/// # }
/// ```
pub fn data_root() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("BFT_DATA_DIR")?);

    // An unset variable means "no game data here" and is the normal case. A
    // set-but-wrong one is a mistake the contributor wants to hear about.
    if !path.is_dir() {
        eprintln!(
            "BFT_DATA_DIR is set to {}, which is not a directory; skipping data-backed tests",
            path.display()
        );
        return None;
    }

    Some(path)
}

/// A `World` node as the real data writes it, for a fixture's `nodes.json`.
///
/// Node 0 with one child (node 1), one partition row of one cell, and the
/// inverted `fog_altitude` the shipped levels have. Public so `bft-sim` and
/// `bft-game` can build the same fixture without a Recoil install.
pub const WORLD_NODE: &str = r#"{
  "name": "world1",
  "flags": { "active": true, "altitude_surface": true, "intersect_surface": true,
             "tree_valid": true, "id_zone_check": true },
  "zone_id": -1, "model_index": -1,
  "area_partition": null, "parent_indices": [], "child_indices": [1],
  "node_bbox": { "a": { "x": 0.0, "y": 0.0, "z": 0.0 }, "b": { "x": 0.0, "y": 0.0, "z": 0.0 } },
  "data": { "World": {
    "fog": {
      "fog_type": "Linear",
      "fog_color": { "r": 0.5, "g": 0.6, "b": 0.7 },
      "fog_range": { "min": 100.0, "max": 200.0 },
      "fog_altitude": { "min": 48.0, "max": 24.0 },
      "fog_density": 0.0
    },
    "area": { "left": 0, "top": 0, "right": 512, "bottom": 512 },
    "light_indices": [], "sound_indices": [],
    "partitions": [
      [ { "x": 0, "z": 0,
          "min": { "x": 0.0, "y": 0.0, "z": 0.0 },
          "max": { "x": 256.0, "y": 0.0, "z": 256.0 },
          "node_indices": [], "values": [] } ]
    ]
  } }
}"#;

/// The smallest `gamez` archive that exercises the whole loader: one world,
/// one group node, one object node with a model, one model, one material, one
/// texture name.
///
/// The object sits at (10, 0, 20) under a group at the origin, so a spawn test
/// has a two-level hierarchy and a non-identity transform to check.
pub fn minimal_gamez() -> Vec<u8> {
    let nodes = format!(
        "[{WORLD_NODE},{},{}]",
        // A group with no model, the way a third to a half of real nodes are.
        r#"{ "name": "group", "zone_id": 3, "model_index": -1,
             "parent_indices": [0], "child_indices": [2],
             "flags": { "active": true, "tree_valid": true },
             "data": { "Object3d": { "transform": "Initial", "signs": 128 } } }"#,
        // A quad-bearing child, offset and flagged as a riding surface.
        r#"{ "name": "floor", "zone_id": 3, "model_index": 0,
             "parent_indices": [1], "child_indices": [],
             "area_partition": { "x": 0, "z": 0 },
             "flags": { "active": true, "tree_valid": true,
                        "altitude_surface": true, "intersect_surface": true },
             "node_bbox": { "a": { "x": -1.0, "y": 0.0, "z": -1.0 },
                            "b": { "x": 1.0, "y": 0.0, "z": 1.0 } },
             "data": { "Object3d": { "transform": { "Matrix": {
                 "r00": 1.0, "r01": 0.0, "r02": 0.0,
                 "r10": 0.0, "r11": 1.0, "r12": 0.0,
                 "r20": 0.0, "r21": 0.0, "r22": 1.0,
                 "r30": 10.0, "r31": 0.0, "r32": 20.0 } }, "signs": 128 } } }"#
    );

    gamez_from(
        r#"[{ "Textured": { "texture_index": 0, "soil": "Default", "cycle": null, "flag": false } }]"#,
        r#"[{ "name": "wall.tif" }]"#,
        MINIMAL_MODEL,
        &nodes,
    )
}

/// One unit quad on the ground plane, wound counter-clockwise from above as
/// the real data is.
pub const MINIMAL_MODEL: &str = r#"[{
  "model_type": "Default", "facade_mode": "CylindricalY",
  "flags": { "lighting": true, "fog": true, "clouds": false, "facade_centroid": false },
  "vertices": [
    { "x": -1.0, "y": 0.0, "z": -1.0 },
    { "x": -1.0, "y": 0.0, "z":  1.0 },
    { "x":  1.0, "y": 0.0, "z":  1.0 },
    { "x":  1.0, "y": 0.0, "z": -1.0 }
  ],
  "normals": [], "morphs": [], "lights": [],
  "polygons": [ {
    "flags": { "show_backface": false, "tri_strip": false },
    "priority": 0, "zone_set": [],
    "vertex_indices": [0, 1, 2, 3],
    "normal_indices": null, "vertex_colors": [],
    "materials": [ { "material_index": 0, "uv_coords": [
      { "u": 0.0, "v": 0.0 }, { "u": 0.0, "v": 1.0 },
      { "u": 1.0, "v": 1.0 }, { "u": 1.0, "v": 0.0 } ] } ]
  } ],
  "texture_scroll": { "u": 0.0, "v": 0.0 },
  "bbox_mid": { "x": 0.0, "y": 0.0, "z": 0.0 }, "bbox_diag": 2.83
}]"#;

/// Build a `gamez` ZIP from four JSON documents, writing `metadata.json` too.
///
/// The bodies are passed as text rather than built from structs on purpose:
/// the loader's job is to read *mech3ax's* JSON, so a fixture that went
/// through our own serialisers would not test the shape (DESIGN §3.1).
pub fn gamez_from(materials: &str, textures: &str, models: &str, nodes: &str) -> Vec<u8> {
    let metadata = r#"{
      "datetime": "1970-01-01T00:00:00.0Z",
      "material_array_size": 5000, "model_array_size": 6000,
      "node_array_size": 16000, "node_last_free": 3
    }"#;

    zip_of(&[
        ("metadata.json", metadata),
        ("textures.json", textures),
        ("materials.json", materials),
        ("models.json", models),
        ("nodes.json", nodes),
    ])
}

/// Write named documents into an in-memory ZIP.
pub fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;

    let mut buffer = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();

        for (name, body) in files {
            zip.start_file(*name, options).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }

        zip.finish().expect("finish");
    }

    buffer
}

/// An ISO 9660 image with `files` in its root directory, each name as the
/// disc records it (`DATA1.CAB;1`). No game data: the bodies are the
/// caller's.
pub fn iso_image(files: &[(&str, &[u8])]) -> Vec<u8> {
    fn record(name: &[u8], extent: u32, size: u32, is_dir: bool) -> Vec<u8> {
        let mut r = vec![0u8; 33];
        r[2..6].copy_from_slice(&extent.to_le_bytes());
        r[6..10].copy_from_slice(&extent.to_be_bytes());
        r[10..14].copy_from_slice(&size.to_le_bytes());
        r[14..18].copy_from_slice(&size.to_be_bytes());
        r[25] = if is_dir { 2 } else { 0 };
        r[32] = name.len() as u8;
        r.extend_from_slice(name);
        if r.len() % 2 == 1 {
            r.push(0);
        }
        r[0] = r.len() as u8;
        r
    }

    // Sector 16 the volume descriptor, 18 the root, files from 19.
    let mut dir = record(&[0], 18, 2048, true);
    dir.extend(record(&[1], 18, 2048, true));
    let mut extent = 19u32;
    let mut bodies = Vec::new();
    for (name, body) in files {
        dir.extend(record(name.as_bytes(), extent, body.len() as u32, false));
        let mut padded = body.to_vec();
        padded.resize(body.len().div_ceil(2048).max(1) * 2048, 0);
        extent += (padded.len() / 2048) as u32;
        bodies.extend(padded);
    }

    let mut img = vec![0u8; 19 * 2048];
    let pvd = 16 * 2048;
    img[pvd] = 1;
    img[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
    img[pvd + 40..pvd + 72].copy_from_slice(&[b' '; 32]);
    img[pvd + 40..pvd + 46].copy_from_slice(b"RECOIL");
    img[pvd + 128..pvd + 130].copy_from_slice(&2048u16.to_le_bytes());
    let root = record(&[0], 18, 2048, true);
    img[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
    img[18 * 2048..18 * 2048 + dir.len()].copy_from_slice(&dir);
    img.extend(bodies);
    img
}

/// A Nero image written at once (`DAOX`, footer version 2): `data` as track
/// 1 after a 150-sector pregap, then each of `audio` as a CD-audio track.
/// The audio must be whole 2352-byte sectors.
pub fn nrg_image(data: &[u8], audio: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = vec![0u8; 150 * 2048];
    let mut table = Vec::new();
    let mut add = |bytes: &mut Vec<u8>, body: &[u8], size: u16, mode: u8| {
        let index1 = bytes.len() as u64;
        bytes.extend_from_slice(body);
        table.extend_from_slice(&[0u8; 12]);
        table.extend_from_slice(&size.to_be_bytes());
        table.push(mode);
        table.extend_from_slice(&[0u8; 3]);
        table.extend_from_slice(&index1.to_be_bytes());
        table.extend_from_slice(&index1.to_be_bytes());
        table.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    };

    add(&mut bytes, data, 2048, 0);
    for track in audio {
        assert_eq!(track.len() % 2352, 0, "audio must be whole sectors");
        add(&mut bytes, track, 2352, 7);
    }

    let mut daox = vec![0u8; 20];
    daox.push(1);
    daox.push(1 + audio.len() as u8);
    daox.extend(table);

    let offset = bytes.len() as u64;
    bytes.extend_from_slice(b"DAOX");
    bytes.extend_from_slice(&(daox.len() as u32).to_be_bytes());
    bytes.extend(daox);
    bytes.extend_from_slice(b"END!\0\0\0\0");
    bytes.extend_from_slice(b"NER5");
    bytes.extend_from_slice(&offset.to_be_bytes());
    bytes
}

/// A temporary directory that deletes itself when dropped.
///
/// The workspace has no `tempfile` dependency and adds no crate nothing has
/// asked for; the tests here need nothing more than
/// a unique directory, so this is it.
#[cfg(test)]
pub(crate) struct TempDir {
    path: PathBuf,
}

#[cfg(test)]
impl TempDir {
    /// Create a fresh directory under the system temp directory.
    pub(crate) fn new() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        // pid + a counter: unique across concurrent test binaries and across
        // the threads inside one of them.
        let unique = format!(
            "bft-assets-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).expect("temp dir");

        TempDir { path }
    }

    /// The directory itself.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A fake Recoil install: the real directory layout with empty files.
///
/// Enough for `root::validate` and `extract::plan`, which only ever look at
/// names, sizes and mtimes. No game data is involved (FR-D7).
#[cfg(test)]
pub(crate) struct FakeRoot {
    dir: TempDir,
}

#[cfg(test)]
impl FakeRoot {
    /// An empty directory: everything is missing.
    pub(crate) fn empty() -> FakeRoot {
        FakeRoot {
            dir: TempDir::new(),
        }
    }

    /// The retail layout of docs/data/install.md: 13 missions, 12 automaps.
    pub(crate) fn retail() -> FakeRoot {
        Self::with_missions(1..=13)
    }

    /// An install with the given mission numbers, mirroring the real tree.
    pub(crate) fn with_missions(missions: std::ops::RangeInclusive<u16>) -> FakeRoot {
        let fake = FakeRoot::empty();
        let root = fake.dir.path();

        let touch = |path: std::path::PathBuf| {
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            // A byte rather than nothing, so a length-based staleness check
            // has something to compare.
            std::fs::write(&path, b"z").expect("write");
        };

        touch(root.join("messages.dll"));
        for name in [
            "image.zbd",
            "interp.zbd",
            "zrdr.zbd",
            "soundsl.zbd",
            "soundsm.zbd",
            "soundsh.zbd",
        ] {
            touch(root.join("zbd").join(name));
        }
        std::fs::create_dir_all(root.join("maps")).expect("mkdir");

        for n in missions {
            let dir = root.join("zbd").join(format!("m{n}"));
            for name in [
                "gamez.zbd",
                "anim.zbd",
                "zrdr.zbd",
                "rtexture2.zbd",
                "rtexture4.zbd",
            ] {
                touch(dir.join(name));
            }
            // docs/data/install.md and docs/data/materials.md: the software
            // tiers vary by mission and must be ignored; m6 is the only one
            // with `texture8.zbd`.
            touch(dir.join("texture2.zbd"));
            touch(dir.join("texture4.zbd"));
            if n == 6 {
                touch(dir.join("texture8.zbd"));
            }
            // m13 has no automap (docs/data/install.md).
            if n != 13 {
                touch(root.join("maps").join(format!("m{n}.zmap")));
            }
        }

        fake
    }

    /// The install directory.
    pub(crate) fn path(&self) -> &std::path::Path {
        self.dir.path()
    }

    /// This install as a validated [`crate::root::DataRoot`].
    pub(crate) fn data_root(&self) -> crate::root::DataRoot {
        let version = crate::root::validate(self.path()).expect("a valid fake root");

        crate::root::DataRoot {
            path: self.path().to_path_buf(),
            version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_data_dir_is_none_not_a_panic() {
        // Safety: the test binary is the only thing reading this variable, and
        // `cargo test` threads do not race on it in this single assertion.
        unsafe { std::env::remove_var("BFT_DATA_DIR") };

        assert!(data_root().is_none());
    }
}
