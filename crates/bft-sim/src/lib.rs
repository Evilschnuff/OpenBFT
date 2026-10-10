//! Fixed-timestep simulation: physics, vehicles, weapons, damage, AI, mission
//! state and the anim event VM.
//!
//! This crate never touches rendering, audio or windows. It takes
//! `PlayerInput` values, steps at a fixed 60 Hz, and leaves the result in
//! components for `bft-game` to draw. That separation is what makes headless
//! servers, deterministic replays and fast unit tests possible, and it is
//! cheap only if kept from the start — so the crate builds with Bevy's render,
//! window, audio and UI features off, and CI checks that it still does.

use bevy::prelude::*;

pub mod ai;
pub mod anim;
pub mod cinematic;
pub mod colliders;
pub mod crater;
pub mod damage;
pub mod level;
pub mod mission;
pub mod physics;
pub mod pickups;
pub mod save;
pub mod schedule;
pub mod states;
pub mod trails;
pub mod vehicle;
pub mod water;
pub mod weapons;

/// The crate's single top-level plugin; everything else is private to it.
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            schedule::SchedulePlugin,
            physics::PhysicsSetupPlugin,
            level::LevelPlugin,
            vehicle::input::InputPlugin,
            vehicle::VehiclePlugin,
            weapons::WeaponsPlugin,
            crater::CraterPlugin,
            damage::DamagePlugin,
            pickups::PickupsPlugin,
            trails::TrailsPlugin,
            ai::AiPlugin,
            anim::AnimPlugin,
            cinematic::CinematicPlugin,
            mission::MissionPlugin,
            save::SavePlugin,
        ));
    }
}
