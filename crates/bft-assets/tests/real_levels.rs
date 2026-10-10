//! The level loader against a real Recoil install: all 13 levels.
//!
//! These are the level loader's checks that need game data. They skip
//! (not fail) when `BFT_DATA_DIR` is unset or the cache has not been built, so
//! a contributor without the game still runs `cargo test --workspace` green.
//! No game data enters the repository: only counts and invariants are
//! asserted.
//!
//! Build the cache first with `open-bft extract`.
//!
//! Every number asserted below is a measurement recorded in docs/data/nodes.md,
//! run as a test so a future change to the loader — or a different
//! mech3ax build emitting a different shape — fails here rather than in the
//! viewer.

use std::path::PathBuf;

use bevy::math::{Mat3, Vec3};
use bft_assets::DataPaths;
use bft_assets::level::{
    LevelData, NodeDesc, PartitionKey, TransformDesc, parse_gamez, parse_zmap,
};

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

/// One level's cached `gamez` archive.
fn gamez_path(paths: &DataPaths, level: u16) -> PathBuf {
    paths
        .cache
        .path
        .join(format!("gamez/m{level}-gamez.gamez.zip"))
}

/// Load one level through the real loader, or `None` when it is not cached.
fn load(paths: &DataPaths, level: u16) -> Option<LevelData> {
    let path = gamez_path(paths, level);

    if !path.is_file() {
        eprintln!("{} is not in the cache; skipping m{level}", path.display());
        return None;
    }

    let bytes = std::fs::read(&path).expect("the cache is readable");

    match parse_gamez(&bytes, &format!("m{level}")) {
        Ok(level) => Some(level),
        Err(e) => panic!("m{level} failed to load: {e}"),
    }
}

/// Every level, with the ones not in the cache skipped.
fn every_level(paths: &DataPaths) -> Vec<(u16, LevelData)> {
    (1..=13)
        .filter_map(|n| load(paths, n).map(|level| (n, level)))
        .collect()
}

/// Node counts per level, from docs/data/nodes.md's table.
const NODE_COUNTS: [(u16, usize, usize, usize, usize); 13] = [
    // level, nodes, Object3d, Lod, Empty
    (1, 4199, 4041, 153, 0),
    (2, 3956, 3638, 313, 0),
    (3, 2463, 2334, 124, 0),
    (4, 3862, 3670, 187, 0),
    (5, 2942, 2814, 123, 0),
    (6, 4955, 4549, 114, 287),
    (7, 1070, 1030, 35, 0),
    (8, 964, 928, 31, 0),
    (9, 1598, 1562, 31, 0),
    (10, 1275, 1239, 31, 0),
    (11, 1052, 1016, 31, 0),
    (12, 1255, 1219, 31, 0),
    (13, 2628, 2582, 41, 0),
];

#[test]
fn every_level_loads_with_the_node_counts_the_notes_record() {
    let Some(paths) = paths() else {
        return;
    };

    let mut checked = 0usize;

    for (n, nodes, object3d, lod, empty) in NODE_COUNTS {
        let Some(level) = load(&paths, n) else {
            continue;
        };
        checked += 1;

        let census = level.census();

        assert_eq!(level.nodes.len(), nodes, "m{n} node count");
        assert_eq!(census.object3d, object3d, "m{n} Object3d count");
        assert_eq!(census.lod, lod, "m{n} Lod count");
        assert_eq!(census.empty, empty, "m{n} Empty count");

        // The scene-setup singletons: exactly one of each, in every level.
        assert_eq!(census.world, 1, "m{n} World");
        assert_eq!(census.camera, 1, "m{n} Camera");
        assert_eq!(census.display, 1, "m{n} Display");
        assert_eq!(census.window, 1, "m{n} Window");
        assert_eq!(census.light, 1, "m{n} Light");
    }

    assert!(
        checked > 0,
        "no level was in the cache; nothing was checked"
    );
}

#[test]
fn the_hierarchy_partitions_every_node_into_exactly_one_root_tree() {
    // The load-bearing claim of docs/data/nodes.md: building from
    // `parent_indices` gives a clean forest, which is what makes the spawner's
    // one-entity-per-node hierarchy sound.
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        let mut children: Vec<Vec<u32>> = vec![Vec::new(); level.nodes.len()];
        for (id, node) in level.nodes.iter().enumerate() {
            if let Some(parent) = node.common().parent {
                children[parent as usize].push(id as u32);
            }
        }

        let mut seen = vec![false; level.nodes.len()];
        for root in &level.root_nodes {
            let mut stack = vec![*root];
            while let Some(id) = stack.pop() {
                assert!(
                    !seen[id as usize],
                    "m{n}: node {id} is in two root trees, so the hierarchy is not a forest"
                );
                seen[id as usize] = true;
                stack.extend_from_slice(&children[id as usize]);
            }
        }

        assert!(
            seen.iter().all(|&s| s),
            "m{n}: {} node(s) are in no root tree",
            seen.iter().filter(|s| !**s).count()
        );

        // 174-220 roots per level, never one (docs/data/nodes.md).
        assert!(
            (150..=250).contains(&level.root_nodes.len()),
            "m{n} has {} roots, outside the 150-250 the data shows",
            level.root_nodes.len()
        );
    }
}

#[test]
fn child_indices_never_contradicts_parent_indices_but_does_omit_edges() {
    // The measurement behind "parents are the authority": zero contradictions,
    // 83-874 omissions per level.
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        let mut contradictions = 0usize;
        let mut omissions = 0usize;

        // Nodes claimed as a child by more than one parent. The loader keeps
        // one parent per node, so these are the only places the two directions
        // can legitimately disagree (docs/data/nodes.md).
        let mut claims: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for node in &level.nodes {
            for child in &node.common().children {
                *claims.entry(*child).or_default() += 1;
            }
        }
        let multi_parent: std::collections::BTreeSet<u32> = claims
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(id, _)| *id)
            .collect();

        for (id, node) in level.nodes.iter().enumerate() {
            let id = id as u32;

            for child in &node.common().children {
                let child_parent = level.nodes[*child as usize].common().parent;

                // A child whose kept parent is someone else is only a
                // contradiction if it is not one of the multi-parent nodes the
                // loader reduced: m9's node 37 hangs off all eight `sgate`
                // nodes, seven of which it therefore no longer names.
                if child_parent != Some(id) && !multi_parent.contains(child) {
                    contradictions += 1;
                }
            }

            if let Some(parent) = node.common().parent
                && !level.nodes[parent as usize].common().children.contains(&id)
            {
                omissions += 1;
            }
        }

        assert_eq!(
            contradictions,
            0,
            "m{n}: `child_indices` contradicts `parent_indices` {contradictions} time(s) \
             outside the {} multi-parent node(s); if this ever fires, the two are no \
             longer consistent and the spawner's choice of authority needs revisiting",
            multi_parent.len()
        );
        // Only m9 has any, and only one (docs/data/nodes.md).
        assert_eq!(
            multi_parent.len(),
            usize::from(n == 9),
            "m{n}: {} node(s) claimed by several parents",
            multi_parent.len()
        );
        assert!(
            omissions > 0,
            "m{n}: `child_indices` omits nothing, so docs/data/nodes.md is wrong \
             and trusting children would be safe after all"
        );
    }
}

#[test]
fn the_nodes_omitted_from_the_worlds_children_are_the_partitioned_ones() {
    // m1: 468 = 468, m9: 441 = 441, set-equal. m6 additionally has its 287
    // `Empty` nodes omitted, which are parented elsewhere.
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        let world = level.world_node;
        let world_children = &level.nodes[world as usize].common().children;

        let omitted: std::collections::BTreeSet<u32> = level
            .nodes
            .iter()
            .enumerate()
            .filter(|(id, node)| {
                node.common().parent == Some(world) && !world_children.contains(&(*id as u32))
            })
            .map(|(id, _)| id as u32)
            .collect();

        let partitioned: std::collections::BTreeSet<u32> = level
            .world
            .partitions
            .iter()
            .flat_map(|cell| cell.nodes.iter().copied())
            .collect();

        // Every partitioned node is one the children list omitted. (The
        // converse fails on m6, whose Empty nodes are omitted too.)
        assert!(
            partitioned.is_subset(&omitted),
            "m{n}: {} partitioned node(s) are not among the {} the World's children omit",
            partitioned.difference(&omitted).count(),
            omitted.len()
        );
        assert!(!partitioned.is_empty(), "m{n} has no partitioned nodes");
    }
}

#[test]
fn area_partition_indexes_the_grid_as_partitions_z_then_x() {
    // 468/468 in m1, 588/588 in m6, 441/441 in m9 with zero misses — the
    // measurement behind docs/data/nodes.md.
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        let mut matched = 0usize;

        for (id, node) in level.nodes.iter().enumerate() {
            let Some(key) = node.common().area_partition else {
                continue;
            };

            let cell = level
                .world
                .partition(key)
                .unwrap_or_else(|| panic!("m{n}: node {id}'s partition {key:?} is out of range"));

            assert!(
                cell.nodes.contains(&(id as u32)),
                "m{n}: node {id} names cell {key:?}, which does not list it — the grid is \
                 not indexed [z][x] after all"
            );
            matched += 1;
        }

        assert!(matched > 0, "m{n} has no partitioned nodes");
    }
}

#[test]
fn every_level_has_a_sun_whose_shares_sum_to_one_and_whose_colour_is_black() {
    // docs/data/nodes.md: the `Light` node is a directional sun, not a point
    // light, and its colour is unusable as given.
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        let sun = level
            .sun
            .as_ref()
            .unwrap_or_else(|| panic!("m{n} has no sun"));

        assert_eq!(sun.common.name, "sunlight", "m{n} sun name");
        assert!(
            (sun.diffuse + sun.ambient - 1.0).abs() < 1e-5,
            "m{n}: diffuse {} + ambient {} is not 1.0",
            sun.diffuse,
            sun.ambient
        );
        assert_eq!(
            sun.color,
            [0.0, 0.0, 0.0],
            "m{n}: the sun's colour is no longer black, so the white substitution \
             in `effective_color` may now be wrong"
        );
        assert_eq!(sun.effective_color(), [1.0, 1.0, 1.0]);
        // The sun is aimed, so its orientation cannot be all zeroes.
        assert_ne!(
            sun.orientation,
            bevy::prelude::Vec3::ZERO,
            "m{n} sun direction"
        );
    }
}

#[test]
fn model_lights_are_effect_templates_not_level_lighting() {
    // The light-carrying
    // nodes are weapon and effect prototypes, most of them outside the World
    // tree entirely (docs/data/nodes.md).
    let Some(paths) = paths() else {
        return;
    };

    let Some(level) = load(&paths, 1) else {
        return;
    };

    let carriers: Vec<&str> = level
        .nodes
        .iter()
        .filter(|node| {
            node.common()
                .model_index
                .and_then(|index| level.models.get(index))
                .is_some_and(|model| !model.lights.is_empty())
        })
        .map(|node| node.name())
        .collect();

    assert_eq!(
        carriers.len(),
        18,
        "m1's light-carrying nodes: {carriers:?}"
    );

    // Every one is an effect or weapon template, by name.
    for name in &carriers {
        assert!(
            [
                "lh_flare",
                "frctwirl",
                "frcleft",
                "frcright",
                "plights",
                "sparky",
                "freon_fly",
                "rmine_blue",
                "mine_red",
                "piece1",
                "piece2",
                "piece3",
                "piece4",
            ]
            .contains(name),
            "m1: `{name}` carries lights but is not one of the known effect templates; \
             if a terrain or building node ever does, level lighting may live there after all"
        );
    }

    // And most hang off `*.flt` template roots rather than the World.
    let mut world_tree = vec![false; level.nodes.len()];
    let mut stack = vec![level.world_node];
    while let Some(id) = stack.pop() {
        if world_tree[id as usize] {
            continue;
        }
        world_tree[id as usize] = true;
        for (child, node) in level.nodes.iter().enumerate() {
            if node.common().parent == Some(id) {
                stack.push(child as u32);
            }
        }
    }

    let outside = level
        .nodes
        .iter()
        .enumerate()
        .filter(|(id, node)| {
            !world_tree[*id]
                && node
                    .common()
                    .model_index
                    .and_then(|index| level.models.get(index))
                    .is_some_and(|model| !model.lights.is_empty())
        })
        .count();

    assert_eq!(
        outside, 14,
        "14 of m1's 18 light carriers sit outside the World tree"
    );
}

#[test]
fn the_three_transform_forms_occur_in_the_proportions_the_notes_record() {
    let Some(paths) = paths() else {
        return;
    };

    let Some(level) = load(&paths, 1) else {
        return;
    };

    let mut initial = 0usize;
    let mut matrix = 0usize;
    let mut rts = 0usize;

    for node in &level.nodes {
        if let NodeDesc::Object3d(object) = node {
            match object.transform {
                TransformDesc::Initial => initial += 1,
                TransformDesc::Matrix { .. } => matrix += 1,
                TransformDesc::RotateTranslateScale { .. } => rts += 1,
            }
        }
    }

    // docs/data/coordinates.md's table, for m1.
    assert_eq!(initial, 2732);
    assert_eq!(matrix, 1307);
    assert_eq!(rts, 2);
}

#[test]
fn every_matrix_transform_is_a_proper_rotation() {
    // What lets the spawner hand these to `Quat::from_mat3` without a general
    // affine decomposition: all 1307 of m1's are orthonormal with determinant
    // +1 (no scale, no shear, no mirror).
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        for (id, node) in level.nodes.iter().enumerate() {
            let NodeDesc::Object3d(object) = node else {
                continue;
            };
            let TransformDesc::Matrix { rotation, .. } = object.transform else {
                continue;
            };

            assert!(
                (rotation.determinant() - 1.0).abs() < 1e-3,
                "m{n} node {id}: determinant {} is not 1, so the matrix is not a pure rotation",
                rotation.determinant()
            );
            for axis in [rotation.x_axis, rotation.y_axis, rotation.z_axis] {
                assert!(
                    (axis.length() - 1.0).abs() < 1e-3,
                    "m{n} node {id}: axis {axis:?} is not unit length"
                );
            }
        }
    }
}

#[test]
fn a_matrix_is_read_so_that_every_parent_bounds_its_rotated_children() {
    // The data's own check on the `Matrix` reading (docs/data/nodes.md). Each
    // node's `child_bbox` is the union of its children's boxes carried into
    // its frame by the children's transforms. Where some child is turned by
    // something other than a symmetric matrix (0° or 180°), only the right
    // reading reproduces it; the transposed one fails most of them.
    let Some(paths) = paths() else {
        return;
    };

    let mut checked = 0;
    for (n, level) in every_level(&paths) {
        for (id, node) in level.nodes.iter().enumerate() {
            let NodeDesc::Object3d(_) = node else {
                continue;
            };
            let common = node.common();
            let (want_min, want_max) = common.child_bbox;
            if want_min == want_max || common.children.is_empty() {
                continue;
            }

            let transforms: Option<Vec<_>> = common
                .children
                .iter()
                .map(|&child| match &level.nodes[child as usize] {
                    NodeDesc::Object3d(object) => match object.transform {
                        TransformDesc::Initial => Some((Mat3::IDENTITY, Vec3::ZERO)),
                        TransformDesc::Matrix {
                            rotation,
                            translation,
                        } => Some((rotation, translation)),
                        TransformDesc::RotateTranslateScale { .. } => None,
                    },
                    _ => None,
                })
                .collect();
            let Some(transforms) = transforms else {
                continue;
            };
            if transforms
                .iter()
                .all(|(rotation, _)| rotation.abs_diff_eq(rotation.transpose(), 1e-5))
            {
                continue;
            }

            let (mut min, mut max) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
            for (&child, (rotation, translation)) in common.children.iter().zip(&transforms) {
                let child = level.nodes[child as usize].common();
                for (lo, hi) in [child.node_bbox, child.child_bbox] {
                    if lo == hi {
                        continue;
                    }
                    for corner in 0..8 {
                        let local = Vec3::new(
                            if corner & 1 == 0 { lo.x } else { hi.x },
                            if corner & 2 == 0 { lo.y } else { hi.y },
                            if corner & 4 == 0 { lo.z } else { hi.z },
                        );
                        let placed = *rotation * local + *translation;
                        min = min.min(placed);
                        max = max.max(placed);
                    }
                }
            }
            if min.x > max.x {
                continue;
            }

            checked += 1;
            assert!(
                min.abs_diff_eq(want_min, 0.05) && max.abs_diff_eq(want_max, 0.05),
                "m{n} node {id} `{}`: children span {min}..{max}, child_bbox says \
                 {want_min}..{want_max}",
                common.name
            );
        }
    }

    if checked > 0 {
        assert_eq!(checked, 65, "the parents this reading was settled on");
    }
}

#[test]
fn the_world_fog_is_linear_with_an_inverted_altitude_range() {
    // docs/data/nodes.md: `fog_altitude` has min > max in every level, so
    // nothing may assume it is ordered.
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        let world = &level.world;

        assert!(
            world.fog_range.0 < world.fog_range.1,
            "m{n}: fog range {:?} is not increasing",
            world.fog_range
        );
        assert!(
            world.fog_altitude.0 > world.fog_altitude.1,
            "m{n}: fog altitude {:?} is no longer inverted, so the (clear, full) \
             reading needs revisiting",
            world.fog_altitude
        );
        // An x/z extent with a positive area, though not necessarily anchored
        // at the origin: m6, m9 and m13 are negative or offset.
        assert!(world.area.2 > world.area.0, "m{n} area width");
        assert!(world.area.3 > world.area.1, "m{n} area height");
        assert!(world.partition_columns > 0, "m{n} has no partition grid");
    }
}

#[test]
fn model_and_material_indices_all_resolve() {
    // Nothing in the shipped data dangles: every node's `model_index` and
    // every polygon's `material_index` is in range, so a warning from the
    // loader's range checks would be new information.
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        for (id, node) in level.nodes.iter().enumerate() {
            if let Some(index) = node.common().model_index {
                assert!(
                    index < level.models.len(),
                    "m{n} node {id} names model {index} of {}",
                    level.models.len()
                );
            }
        }

        for (index, model) in level.models.iter().enumerate() {
            for polygon in &model.polygons {
                for material in &polygon.materials {
                    assert!(
                        (material.material_index as usize) < level.materials.len(),
                        "m{n} model {index} names material {} of {}",
                        material.material_index,
                        level.materials.len()
                    );
                }
            }
        }
    }
}

#[test]
fn the_collision_flags_select_a_real_subset_of_the_model_nodes() {
    // The spawner's rule, rather than "everything with a model is collidable": the
    // flags are the authority, and they do discriminate.
    let Some(paths) = paths() else {
        return;
    };

    for (n, level) in every_level(&paths) {
        let with_model = level
            .nodes
            .iter()
            .filter(|node| node.common().model_index.is_some())
            .count();
        let collide = level
            .nodes
            .iter()
            .filter(|node| node.common().model_index.is_some() && node.common().flags.collides())
            .count();

        assert!(with_model > 0, "m{n} has no model-bearing nodes");
        assert!(
            collide > 0 && collide <= with_model,
            "m{n}: {collide} of {with_model} model nodes collide"
        );
    }
}

#[test]
fn every_cached_zmap_parses_and_projects_onto_the_ground_plane() {
    // docs/data/coordinates.md: the automap's header `y` is exactly 0 in both corners in
    // every mission, which is what makes it an x/z projection. m13 ships none.
    let Some(paths) = paths() else {
        return;
    };

    let mut checked = 0usize;

    for n in 1..=13u16 {
        let path = paths.cache.path.join(format!("zmap/m{n}.zmap.json"));
        if !path.is_file() {
            continue;
        }

        let bytes = std::fs::read(&path).expect("readable");
        let zmap = parse_zmap(&bytes).unwrap_or_else(|e| panic!("m{n}.zmap does not parse: {e}"));
        checked += 1;

        assert_eq!(zmap.min.y, 0.0, "m{n} zmap min y");
        assert_eq!(zmap.max.y, 0.0, "m{n} zmap max y");
        assert!(zmap.max.x > zmap.min.x, "m{n} zmap x extent");
        assert!(!zmap.features.is_empty(), "m{n} zmap has no features");
    }

    if checked == 0 {
        eprintln!("no zmap in the cache; skipping");
    }
}

#[test]
fn a_partition_key_out_of_range_is_none_rather_than_a_wrapped_lookup() {
    let Some(paths) = paths() else {
        return;
    };

    let Some(level) = load(&paths, 1) else {
        return;
    };

    let (columns, rows) = (
        level.world.partition_columns,
        level.world.partitions.len() / level.world.partition_columns.max(1),
    );

    assert!(
        level
            .world
            .partition(PartitionKey {
                x: columns as i32,
                z: 0
            })
            .is_none(),
        "one past the last column must not wrap onto the next row"
    );
    assert!(
        level
            .world
            .partition(PartitionKey {
                x: 0,
                z: rows as i32
            })
            .is_none()
    );
}
