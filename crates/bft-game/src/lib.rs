//! Presentation: level rendering, camera, HUD, menus, audio, dev tools.
//!
//! The simulation itself lives in `bft-sim`; this crate draws what it
//! produced. `GamePlugin` pulls `SimPlugin` in so a client runs both.
//!
//! # The application lifecycle
//!
//! `GamePlugin` wires the states in [`bft_sim::states`] to the five things
//! that happen in them: [`boot`] finds and extracts the data, [`ui`]'s menus
//! offer the missions and maps, [`loading`] turns a choice into a spawned level,
//! [`viewer`] flies around it and [`play`] puts the player in a vehicle on
//! it. The state types are re-exported from [`states`].

use bevy::prelude::*;
use bft_sim::SimPlugin;

pub mod aim;
pub mod anim_view;
pub mod audio;
pub mod boot;
pub mod camera;
pub mod camera_tuning;
pub mod chase_camera;
pub mod cinematic_view;
pub mod crater_view;
pub mod devtools;
pub mod effects;
pub mod enemies;
pub mod globals;
pub mod guided_view;
pub mod input_map;
pub mod level_view;
pub mod loading;
pub mod materials;
pub mod mesh_builder;
pub mod mission_setup;
pub mod physics_debug;
pub mod pickup_view;
pub mod play;
pub mod player_model;
pub mod player_rig;
pub mod savegame;
pub mod states;
pub mod ui;
pub mod vehicle_debug;
pub mod viewer;

/// The crate's single top-level plugin: presentation, plus the sim it draws.
///
/// `bft_assets::AssetsPlugin` is **not** added here: it carries the resolved
/// data paths and has to be added before `DefaultPlugins`, because Bevy
/// builds its asset sources when `AssetPlugin` is built. `bft-bin` adds it.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        // Two tuples, because Bevy's `Plugins` tuple stops at 15 and this
        // list passed it. The split is gameplay then presentation; it carries
        // no ordering meaning, since a plugin's systems are ordered by their
        // sets and states rather than by when it was added.
        app.add_plugins((
            SimPlugin,
            aim::AimPlugin,
            camera::FlyCameraPlugin,
            chase_camera::ChaseCameraPlugin,
            input_map::InputMapPlugin,
            play::PlayPlugin,
            enemies::EnemiesPlugin,
            player_model::PlayerModelPlugin,
            player_rig::PlayerRigPlugin,
            vehicle_debug::VehicleDebugPlugin,
        ))
        .add_plugins((
            materials::MaterialsPlugin,
            mesh_builder::MeshBuilderPlugin,
            physics_debug::PhysicsDebugPlugin,
            level_view::LevelViewPlugin,
            ui::UiPlugin,
            boot::BootPlugin,
            loading::LoadingPlugin,
            viewer::ViewerPlugin,
            mission_setup::MissionSetupPlugin,
            audio::AudioPlugin,
            effects::EffectsPlugin,
            crater_view::CraterViewPlugin,
            anim_view::AnimViewPlugin,
            pickup_view::PickupViewPlugin,
            cinematic_view::CinematicViewPlugin,
        ))
        .add_plugins(guided_view::GuidedViewPlugin)
        .add_plugins(savegame::SaveGamePlugin)
        // One UI camera for the whole run: boot, menu and loading screens all
        // draw through it, and the viewer's 3D camera is a second one with a
        // higher order. Spawned once rather than per state so no frame is
        // ever without one.
        .add_systems(Startup, spawn_ui_camera);
    }
}

/// The 2D camera every Bevy UI screen in this crate renders through.
///
/// `order` is below the viewer's 3D camera so the level draws first and the
/// UI on top of it.
fn spawn_ui_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            // The 3D camera clears; this one must not paint over the level.
            clear_color: ClearColorConfig::None,
            ..default()
        },
    ));
}
