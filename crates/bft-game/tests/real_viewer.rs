//! The viewer's checks that need a real Recoil install.
//!
//! These skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built, so a contributor without the game still runs
//! `cargo test --workspace` green. No game data enters the repository: only
//! counts and invariants are asserted.
//!
//! Build the cache first with `open-bft extract`.
//!
//! Two of these are texture checks that need a viewer: a `Simple` alpha
//! cut-out from `image.zbd`, and a texture cycle that actually animates.

use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use bft_assets::interp::InterpScripts;
use bft_assets::textures::{TextureAlpha, TextureSet};
use bft_assets::{AssetsPlugin, DataPaths, LevelCatalog};
use bft_game::level_view::SKY_NODE;
use bft_game::materials::{CycledTexture, alpha_mode};
use bft_game::viewer::{level_bounds, start_viewpoint, viewpoint};

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

/// A headless app with the cache source and every loader registered.
fn app_for(paths: DataPaths) -> App {
    let mut app = App::new();
    // `AssetsPlugin` before Bevy's own, and `finish()` by hand, for the two
    // reasons: the `cache://` source is built
    // when `AssetPlugin` is, and the loaders register in `Plugin::finish`.
    app.add_plugins(AssetsPlugin::new(paths));
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.finish();

    app
}

/// Drive the asset server until a handle resolves, or give up.
fn load_blocking<A: Asset>(app: &mut App, path: &str) -> Handle<A> {
    let handle: Handle<A> = app.world().resource::<AssetServer>().load(path.to_string());

    for _ in 0..4000 {
        app.update();

        match app.world().resource::<AssetServer>().load_state(&handle) {
            bevy::asset::LoadState::Loaded => return handle,
            bevy::asset::LoadState::Failed(e) => panic!("{path} failed to load: {e}"),
            _ => {}
        }
    }

    panic!("{path} did not finish loading");
}

/// A `Simple` texture is one-bit cut-out alpha,
/// and it occurs only in `image.zbd` — level archives are `None` or `Full`
/// (docs/data/materials.md). So the cut-out the viewer has to show comes from the UI
/// archive, and this checks the whole path: the archive loads, a `Simple`
/// texture is in it, and the material layer turns it into a `Mask`.
#[test]
fn a_simple_texture_from_image_zbd_becomes_a_cut_out_material() {
    let Some(paths) = paths() else { return };

    let archive = paths.cache.path.join("textures/image.tex.zip");
    if !archive.is_file() {
        eprintln!("no image.tex.zip in the cache; skipping");
        return;
    }

    let mut app = app_for(paths);
    let handle = load_blocking::<TextureSet>(&mut app, "cache://textures/image.tex.zip");
    let sets = app.world().resource::<Assets<TextureSet>>();
    let set = sets.get(&handle).expect("loaded");

    let simple: Vec<&String> = set
        .info
        .iter()
        .filter(|(_, info)| info.alpha == TextureAlpha::Simple)
        .map(|(name, _)| name)
        .collect();

    // docs/data/materials.md: 17 of image.zbd's 433 are `Simple`, and none of the
    // level archives has any.
    assert_eq!(
        simple.len(),
        17,
        "image.zbd should hold exactly the 17 one-bit cut-out textures materials.md counts"
    );

    // Each one has to have decoded to an image, or there is nothing to cut out.
    for name in &simple {
        assert!(
            set.image(name).is_some(),
            "{name} is `Simple` but decoded to no image"
        );
    }

    // And the material layer has to read that as a cut-out rather than as
    // blending, which is what makes the alpha crisp on screen.
    assert_eq!(alpha_mode(TextureAlpha::Simple), AlphaMode::Mask(0.5));
    assert_eq!(alpha_mode(TextureAlpha::Full), AlphaMode::Blend);
    assert_eq!(alpha_mode(TextureAlpha::None), AlphaMode::Opaque);
}

/// A cycled material has to animate, and no
/// level bakes a cycle — they all come from the interp runtime script
/// (docs/data/materials.md). This loads the real `interp.json`, builds m1's bindings,
/// finds m1's `surf1` cycle, and steps the component to
/// prove it advances.
#[test]
fn m1s_surf_cycle_is_a_thirteen_frame_loop_that_advances() {
    let Some(paths) = paths() else { return };

    let interp = paths.cache.path.join("interp.json");
    if !interp.is_file() {
        eprintln!("no interp.json in the cache; skipping");
        return;
    }

    let mut app = app_for(paths);
    let handle = load_blocking::<InterpScripts>(&mut app, "cache://interp.json");
    let scripts = app.world().resource::<Assets<InterpScripts>>();
    let bindings = scripts.get(&handle).expect("loaded").bindings(1);

    let cycles: Vec<_> = bindings
        .texture_anims
        .iter()
        .filter_map(|binding| match &binding.anim {
            bft_assets::interp::TextureAnim::Cycle {
                frames,
                fps,
                looping,
            } => Some((binding.node.as_str(), frames.len(), *fps, *looping)),
            _ => None,
        })
        .collect();

    assert!(
        !cycles.is_empty(),
        "m1's runtime script binds texture cycles; none were found"
    );

    let (_, frames, fps, looping) = cycles
        .iter()
        .find(|(node, ..)| *node == "surf1")
        .copied()
        .expect("m1 binds a cycle to `surf1`");

    // m1's `surf1`: a 13-frame looping cycle at 6 fps.
    assert_eq!(frames, 13, "surf1 is a 13-frame cycle");
    assert_eq!(fps, 6.0, "at 6 fps");
    assert!(looping, "and it loops");

    // The component that shows it has to actually advance: one period moves
    // one frame, and it wraps rather than stopping.
    let images: Vec<Handle<Image>> = (0..frames).map(|_| Handle::default()).collect();
    let mut cycle = CycledTexture::new(images, fps, looping);
    let period = 1.0 / fps;

    assert_eq!(cycle.frame, 0);
    assert!(cycle.advance(period), "a full period changes the frame");
    assert_eq!(cycle.frame, 1);

    // Round the whole loop and back to the start.
    for _ in 1..frames {
        cycle.advance(period);
    }
    assert_eq!(cycle.frame, 0, "a looping cycle wraps to its first frame");
}

/// Every level in the catalog has to be openable — `Tab` cycles through
/// every level — minus the window. This checks the part
/// that can fail without a GPU: that each level's partition grid yields a
/// viewpoint inside the level rather than at the origin.
#[test]
fn every_level_frames_a_camera_somewhere_in_its_own_terrain() {
    let Some(paths) = paths() else { return };

    let mut app = app_for(paths);
    let levels: Vec<_> = app
        .world()
        .resource::<LevelCatalog>()
        .levels
        .iter()
        .map(|entry| {
            (
                entry.id,
                entry.name.clone(),
                entry.gamez.clone(),
                entry.reader.clone(),
            )
        })
        .collect();

    if levels.is_empty() {
        eprintln!("the cache holds no levels; skipping");
        return;
    }
    assert_eq!(levels.len(), 13, "the retail release ships 13 levels");

    let axes = bft_assets::AxisConvention::GAMEZ_DEFAULT;

    for (id, name, path, reader) in levels {
        let handle = load_blocking::<bft_assets::LevelData>(&mut app, &path.to_string());
        let data = app.world().resource::<Assets<bft_assets::LevelData>>();
        let level = data.get(&handle).expect("loaded");

        let grid = bft_sim::level::PartitionGrid::from_level(level);
        let bounds = level_bounds(&grid, &axes)
            .unwrap_or_else(|| panic!("{name} has no occupied partition cells"));

        let fog_range = level.world.fog_range;
        // Copied out before the next load, which needs the world mutably.
        let node_position = level
            .nodes
            .iter()
            .find(|node| node.common().name == "bft_00")
            .map(|node| match node {
                bft_assets::NodeDesc::Object3d(object) => match object.transform {
                    bft_assets::TransformDesc::Initial => Vec3::ZERO,
                    bft_assets::TransformDesc::Matrix { translation, .. } => translation,
                    bft_assets::TransformDesc::RotateTranslateScale { translate, .. } => translate,
                },
                _ => Vec3::ZERO,
            })
            .unwrap_or_else(|| panic!("{name} has a bft_00 node"));

        let fog_near = Some(fog_range.0);
        let (eye, target) = viewpoint(Some(bounds), None, fog_near);

        // The target is inside the level's own footprint, which is the whole
        // point: twelve of the thirteen levels have no placed player start
        // and a naive camera ends up at the origin looking at templates.
        assert!(
            target.x >= bounds.0.x && target.x <= bounds.1.x,
            "{name}: camera target x {} outside {:?}..{:?}",
            target.x,
            bounds.0.x,
            bounds.1.x
        );
        assert!(
            target.z >= bounds.0.z && target.z <= bounds.1.z,
            "{name}: camera target z {} outside the level",
            target.z
        );
        assert!(
            eye.y > target.y,
            "{name}: the camera should look down at the level"
        );

        // And it stands where the level's own fog still lets it see.
        let distance = eye.distance(target);
        assert!(
            distance < fog_range.0,
            "{name}: camera {distance:.0} away but fog starts at {:.0}",
            fog_range.0
        );

        // Every level states a real player start in its reader archive, and
        // that — not the `bft_00` node — is where the viewer opens.
        let reader_handle = load_blocking::<bft_assets::ReaderData>(&mut app, &reader.to_string());
        let readers = app.world().resource::<Assets<bft_assets::ReaderData>>();
        let start = readers
            .get(&reader_handle)
            .expect("loaded")
            .player_start()
            .expect("every level's aiv.zrd states a bft_00 start");

        assert!(
            start.position.x >= bounds.0.x
                && start.position.x <= bounds.1.x
                && start.position.z >= bounds.0.z
                && start.position.z <= bounds.1.z,
            "{name}: player start {:?} is outside the level's own terrain",
            start.position
        );

        // The node of the same name is a build-script placeholder. m1's is
        // ~240 units from the truth (over water rather than by the hangar);
        // the other twelve are an identity at the origin. Either way it
        // disagrees with the reader, which is the whole reason this test
        // exists.
        assert!(
            node_position.distance(start.position) > 100.0,
            "{name}: the bft_00 node {node_position:?} agrees with the reader \
             start {:?}; if that is now true for real data, this package's \
             note and the fallback both need revisiting",
            start.position
        );

        // Only m1's node is placed at all; the rest sit at the origin.
        assert_eq!(
            node_position == Vec3::ZERO,
            id != bft_assets::LevelId(1),
            "{name}: only m1's bft_00 node carries a transform"
        );

        // And the opening view is over the player's shoulder.
        let (start_eye, start_target) = start_viewpoint(&start);
        assert!(
            start_eye.y > start.position.y,
            "{name}: the camera should be above the player"
        );
        assert!(
            (start_target - start_eye)
                .with_y(0.0)
                .normalize()
                .dot(start.facing())
                > 0.9,
            "{name}: the view should run the way the player faces"
        );
    }
}

/// Every level models its sky as a node at the **world origin**, so a dome
/// left where the data puts it clips through terrain that sits elsewhere —
/// which every level's does. This is the measurement behind
/// [`bft_game::level_view::follow_sky_dome`].
#[test]
fn every_level_has_a_sky_dome_at_the_origin() {
    let Some(paths) = paths() else { return };

    let mut app = app_for(paths);
    let levels: Vec<_> = app
        .world()
        .resource::<LevelCatalog>()
        .levels
        .iter()
        .map(|entry| (entry.name.clone(), entry.gamez.clone()))
        .collect();

    if levels.is_empty() {
        eprintln!("the cache holds no levels; skipping");
        return;
    }

    for (name, path) in levels {
        let handle = load_blocking::<bft_assets::LevelData>(&mut app, &path.to_string());
        let data = app.world().resource::<Assets<bft_assets::LevelData>>();
        let level = data.get(&handle).expect("loaded");

        let sky = level
            .nodes
            .iter()
            .find(|node| node.common().name == SKY_NODE)
            .unwrap_or_else(|| panic!("{name} has no `{SKY_NODE}` node"));

        // Always an identity, i.e. the dome sits at (0, 0, 0).
        if let bft_assets::NodeDesc::Object3d(object) = sky {
            assert_eq!(
                object.transform,
                bft_assets::TransformDesc::Initial,
                "{name}: the sky is expected at the world origin"
            );
        }

        // The dome straddles its own origin — a ring centred on (0, 0, 0),
        // which is why leaving it there clips a level sitting elsewhere. m2,
        // m6 and m10 model it as a *group* whose children hold the geometry,
        // so the box to check is the node's or its children's.
        let boxes = std::iter::once(sky.common().model_bbox).chain(
            sky.common()
                .children
                .iter()
                .filter_map(|id| level.nodes.get(*id as usize))
                .map(|child| child.common().model_bbox),
        );
        let straddles = boxes
            .filter(|(min, max)| min != max)
            .any(|(min, max)| max.x > 0.0 && min.x < 0.0);
        assert!(straddles, "{name}: the dome should straddle its own origin");
    }
}
