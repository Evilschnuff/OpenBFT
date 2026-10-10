//! The model mirror and the triangulator against a real Recoil install.
//!
//! These are the model and mesh checks that need game data. They skip
//! (not fail) when `BFT_DATA_DIR` is unset or the cache has not been built,
//! so a contributor without the game still runs `cargo test --workspace`
//! green. No game data enters the repository: everything is read from the
//! cache and only counts and invariants are asserted.
//!
//! Build the cache first with `open-bft extract`.
//!
//! The winding check below is the one that matters: it is the measurement
//! behind docs/data/models.md's winding finding, run as a test so a future
//! change to `triangulate` or to `AxisConvention::GAMEZ_DEFAULT` that turned
//! the world inside out would fail here rather than in the viewer.

use std::path::PathBuf;

use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::geometry::{collider_indices, triangulate};
use bft_assets::mech3ax;
use bft_assets::models::{ModelDesc, ModelType, model_desc};
use bft_assets::{DataPaths, materials::MaterialDesc, materials::material_descs};

/// The resolved paths for a real install, or `None` when there is no data.
fn paths() -> Option<DataPaths> {
    let root = bft_assets::test_support::data_root()?;

    match DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.is_dir() => Some(paths),
        Ok(paths) => {
            eprintln!(
                "no extraction cache at {}; run `open-bft extract`. Skipping.",
                paths.cache.path.display()
            );
            None
        }
        Err(e) => {
            eprintln!("cannot resolve the data paths ({e}); skipping");
            None
        }
    }
}

/// Read one JSON member of a cached ZIP.
fn zip_json<T: serde::de::DeserializeOwned>(path: &PathBuf, entry: &str) -> T {
    use std::io::Read;

    let file = std::fs::File::open(path).expect("the cache holds this archive");
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).expect("a readable ZIP");
    let mut json = String::new();
    archive
        .by_name(entry)
        .expect("the archive holds this entry")
        .read_to_string(&mut json)
        .expect("readable");

    serde_json::from_str(&json).expect("parses into the mirrored types")
}

/// Every level's gamez archive that is actually in the cache.
fn level_archives(paths: &DataPaths) -> Vec<(String, PathBuf)> {
    (1..=13)
        .map(|mission| {
            let name = format!("m{mission}");
            let path = paths
                .cache
                .path
                .join(format!("gamez/{name}-gamez.gamez.zip"));
            (name, path)
        })
        .filter(|(_, path)| path.is_file())
        .collect()
}

/// One level's models, through the mirror and the converter.
fn models_of(archive: &PathBuf) -> Vec<ModelDesc> {
    let models: Vec<mech3ax::Model> = zip_json(archive, "models.json");

    models.iter().map(model_desc).collect()
}

#[test]
fn every_level_parses_every_model() {
    let Some(paths) = paths() else { return };
    let archives = level_archives(&paths);

    if archives.is_empty() {
        eprintln!("no gamez archives in the cache; skipping");
        return;
    }

    let mut total = 0usize;
    let mut polygons = 0usize;

    for (level, archive) in &archives {
        let models = models_of(archive);

        assert!(!models.is_empty(), "{level} has models");

        for (index, model) in models.iter().enumerate() {
            // Every polygon index must be in range, or `triangulate` would
            // be dropping real geometry.
            for polygon in &model.polygons {
                for vertex in &polygon.vertex_indices {
                    assert!(
                        (*vertex as usize) < model.vertices.len(),
                        "{level} model {index}: vertex index {vertex} of {}",
                        model.vertices.len()
                    );
                }
            }

            polygons += model.polygons.len();
        }

        total += models.len();
    }

    // The counts docs/data/models.md records, so a mech3ax bump that changed the
    // extraction shows up here.
    if archives.len() == 13 {
        assert_eq!(total, 13_036, "models across all 13 levels");
        assert_eq!(polygons, 103_158, "polygons across all 13 levels");
    }
}

#[test]
fn the_data_notes_claims_about_models_still_hold() {
    let Some(paths) = paths() else { return };
    let archives = level_archives(&paths);

    if archives.len() < 13 {
        eprintln!("not every level is in the cache; skipping the census");
        return;
    }

    let mut tri_strip = 0usize;
    let mut points = 0usize;
    let mut vertex_colors = 0usize;
    let mut multi_material = 0usize;
    let mut non_cylindrical_facades = 0usize;
    let mut scrolled = 0usize;
    let mut normal_indices = 0usize;
    let mut uvless = 0usize;

    for (_, archive) in &archives {
        for model in models_of(archive) {
            if matches!(model.model_type, ModelType::Points) {
                points += 1;
            }
            if matches!(model.model_type, ModelType::Facade)
                && !matches!(
                    model.facade_mode,
                    bft_assets::models::FacadeMode::CylindricalY
                )
            {
                non_cylindrical_facades += 1;
            }
            if model.texture_scroll != Vec2::ZERO {
                scrolled += 1;
            }

            for polygon in &model.polygons {
                if polygon.tri_strip {
                    tri_strip += 1;
                }
                if !polygon.vertex_colors.is_empty() {
                    vertex_colors += 1;
                }
                if polygon.materials.len() > 1 {
                    multi_material += 1;
                }
                if polygon.normal_indices.is_some() {
                    normal_indices += 1;
                }
                for material in &polygon.materials {
                    if material.uv_coords.is_none() {
                        uvless += 1;
                    }
                }
            }
        }
    }

    // docs/data/models.md. Each of these lets `triangulate` skip work; if one ever
    // becomes non-zero the corresponding path needs real testing.
    assert_eq!(tri_strip, 0, "TRI_STRIP is never used");
    assert_eq!(points, 0, "no Points models");
    assert_eq!(vertex_colors, 0, "no vertex colours");
    assert_eq!(multi_material, 0, "no multi-material polygons");
    assert_eq!(non_cylindrical_facades, 0, "every facade is CylindricalY");
    assert_eq!(scrolled, 0, "texture_scroll is baked nowhere");

    // docs/data/models.md: commoner than the first count suggested.
    assert_eq!(normal_indices, 2120, "polygons with authored normals");
    // Item 3: the Colored-material polygons.
    assert_eq!(uvless, 2610, "polygon materials without UVs");
}

#[test]
fn a_polygon_has_uvs_exactly_when_its_material_is_textured() {
    let Some(paths) = paths() else { return };
    let archives = level_archives(&paths);

    if archives.is_empty() {
        eprintln!("no gamez archives in the cache; skipping");
        return;
    }

    // The invariant `triangulate`'s "no UVs means zeroes" rests on: a missing
    // UV set is a `Colored` material, never a textured one missing its data.
    for (level, archive) in &archives {
        let materials: Vec<mech3ax::Material> = zip_json(archive, "materials.json");
        let textures: Vec<mech3ax::TextureRef> = zip_json(archive, "textures.json");
        let materials = material_descs(&materials, &textures).expect("the materials resolve");

        for (index, model) in models_of(archive).iter().enumerate() {
            for polygon in &model.polygons {
                for material in &polygon.materials {
                    let desc = materials
                        .get(material.material_index as usize)
                        .expect("the material index is in range");

                    match (desc, &material.uv_coords) {
                        (MaterialDesc::Textured { .. }, Some(uvs)) => assert_eq!(
                            uvs.len(),
                            polygon.vertex_indices.len(),
                            "{level} model {index}: one UV per corner"
                        ),
                        (MaterialDesc::Colored { .. }, None) => {}
                        (MaterialDesc::Textured { .. }, None) => {
                            panic!("{level} model {index}: a textured polygon with no UVs")
                        }
                        (MaterialDesc::Colored { .. }, Some(_)) => {
                            panic!("{level} model {index}: a coloured polygon with UVs")
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn triangulating_a_whole_level_keeps_every_polygon_and_agrees_with_the_collider() {
    let Some(paths) = paths() else { return };
    let archives = level_archives(&paths);

    if archives.is_empty() {
        eprintln!("no gamez archives in the cache; skipping");
        return;
    }

    let axes = AxisConvention::GAMEZ_DEFAULT;

    for (level, archive) in &archives {
        let mut expected_triangles = 0usize;
        let mut actual_triangles = 0usize;

        for (index, model) in models_of(archive).iter().enumerate() {
            let triangulated = triangulate(model, &axes);

            assert_eq!(
                triangulated.skipped_polygons, 0,
                "{level} model {index}: no polygon should be dropped"
            );

            // An n-gon fans into n-2 triangles, and Recoil has exactly one
            // material per polygon, so the totals are arithmetic.
            for polygon in &model.polygons {
                if polygon.vertex_indices.len() >= 3 {
                    expected_triangles += polygon.vertex_indices.len() - 2;
                }
            }
            actual_triangles += triangulated.triangle_count();

            // Every attribute array is parallel.
            let corners = triangulated.positions.len();
            assert_eq!(triangulated.normals.len(), corners);
            assert_eq!(triangulated.uvs.len(), corners);
            assert_eq!(triangulated.colors.len(), corners);

            // Every index is in range.
            for group in &triangulated.groups {
                for corner in &group.indices {
                    assert!(
                        (*corner as usize) < corners,
                        "{level} model {index}: index out of range"
                    );
                }
            }

            // And the collider sees exactly the same triangles, which is the
            // whole "what you see is what you hit" claim (DESIGN §7.1).
            let (vertices, triangles) = collider_indices(model.model_type, &triangulated);
            match model.model_type {
                ModelType::Default => {
                    assert_eq!(triangles.len(), triangulated.triangle_count());
                    assert_eq!(vertices.len(), corners);
                }
                ModelType::Facade | ModelType::Points => {
                    assert!(triangles.is_empty(), "{level} model {index} is a billboard");
                }
            }
        }

        assert_eq!(
            actual_triangles, expected_triangles,
            "{level}: every polygon fans into n-2 triangles"
        );
        assert!(actual_triangles > 0, "{level} produced geometry");
    }
}

#[test]
fn closed_models_are_wound_outward_under_the_shipped_convention() {
    let Some(paths) = paths() else { return };
    let archives = level_archives(&paths);

    if archives.is_empty() {
        eprintln!("no gamez archives in the cache; skipping");
        return;
    }

    // "No inside-out surfaces", made mechanical. For a closed mesh whose triangles are counter-clockwise
    // seen from outside — Bevy's front-face convention — the divergence
    // theorem gives a positive signed volume. A convention or a winding rule
    // that mirrored the world would make these negative.
    let axes = AxisConvention::GAMEZ_DEFAULT;
    let mut outward = 0usize;
    let mut inward = 0usize;

    for (_, archive) in &archives {
        for model in models_of(archive) {
            if !matches!(model.model_type, ModelType::Default) {
                continue;
            }

            let triangulated = triangulate(&model, &axes);
            let (vertices, triangles) = collider_indices(model.model_type, &triangulated);

            if !is_closed(&vertices, &triangles) {
                continue;
            }

            let volume: f64 = triangles
                .iter()
                .map(|triangle| {
                    let [a, b, c] = triangle.map(|index| vertices[index as usize].as_dvec3());
                    a.dot(b.cross(c)) / 6.0
                })
                .sum();

            if volume > 1e-6 {
                outward += 1;
            } else if volume < -1e-6 {
                inward += 1;
            }
        }
    }

    assert!(
        outward > 100,
        "the levels should hold hundreds of closed models, found {outward}"
    );
    // 373 closed models in the 13 levels, 2 of which are wound inward — an
    // authoring slip in the original data, not a bug here. Anything more than
    // a handful means the winding rule has been broken.
    assert!(
        inward * 50 < outward,
        "{inward} of {} closed models are inside out; the winding rule is wrong",
        outward + inward
    );
}

/// Whether every edge of a triangle soup is shared by exactly two triangles.
///
/// `triangulate` duplicates every polygon corner, so two triangles that share
/// an edge geometrically do **not** share an index. The positions are welded
/// by exact bits first — the duplicates come from copying one `Vec3`, so they
/// are bit-identical and no epsilon is needed.
fn is_closed(vertices: &[Vec3], triangles: &[[u32; 3]]) -> bool {
    use std::collections::HashMap;

    if triangles.len() < 4 {
        return false;
    }

    let mut welded: HashMap<[u32; 3], u32> = HashMap::new();
    let mut id_of = |index: u32| -> u32 {
        let position = vertices[index as usize];
        let key = [
            position.x.to_bits(),
            position.y.to_bits(),
            position.z.to_bits(),
        ];
        let next = welded.len() as u32;

        *welded.entry(key).or_insert(next)
    };

    let mut edges: HashMap<(u32, u32), usize> = HashMap::new();

    for triangle in triangles {
        let [a, b, c] = triangle.map(&mut id_of);

        if a == b || b == c || a == c {
            // A degenerate triangle has no well-defined edges to match.
            return false;
        }

        for (from, to) in [(a, b), (b, c), (c, a)] {
            *edges.entry((from.min(to), from.max(to))).or_default() += 1;
        }
    }

    edges.values().all(|count| *count == 2)
}
