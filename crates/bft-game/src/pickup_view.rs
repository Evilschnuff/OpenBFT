//! What a pickup looks like: its `puNNN` template, cloned onto it.
//!
//! `bft_sim::pickups` spawns the trigger and the state and leaves the model
//! to the game. Nothing drew one, so every pickup in the level was a sensor
//! sphere you could only find by driving over it and hearing the sound.
//!
//! One system rather than a call in `place_pickups`, because pickups arrive
//! from two places: the level's `puppies.zrd` placements on entering, and an
//! enemy's drops whenever one dies.

use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::level::LevelData;
use bft_sim::ai::templates::{self, LevelTemplates};
use bft_sim::level::NodeActive;
use bft_sim::pickups::{Pickup, RespawnTimer, TakenPickup, template_for};
use bft_sim::states::AppState;

use crate::level_view::LoadedLevel;

/// The template's child that is its trigger volume, not part of its look.
///
/// A flat-coloured 6 m billboard in every `puNNN` (m1's is blue): drawn, it
/// covers the pickup it bounds.
const VOLUME_NODE: &str = "bvol";

/// A pickup whose model has been attached, or that has none to attach.
#[derive(Component, Debug, Clone, Copy)]
pub struct PickupModel;

/// Draws the pickups.
pub struct PickupViewPlugin;

impl Plugin for PickupViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                reclone_respawned_pickups,
                attach_pickup_models,
                hide_taken_pickups,
                show_taken_pickups,
            )
                .chain()
                .run_if(in_state(AppState::InGame)),
        );
    }
}

/// Clone each new pickup's `puNNN` template under it.
pub fn attach_pickup_models(
    mut commands: Commands,
    loaded: Option<Res<LoadedLevel>>,
    levels: Res<Assets<LevelData>>,
    axes: Option<Res<AxisConvention>>,
    mut index: Local<Option<(AssetId<LevelData>, LevelTemplates)>>,
    pickups: Query<(Entity, &Pickup), Without<PickupModel>>,
) {
    if pickups.is_empty() {
        return;
    }
    let Some(loaded) = loaded else {
        return;
    };
    let Some(level) = levels.get(&loaded.data) else {
        return;
    };
    let axes = axes
        .map(|axes| *axes)
        .unwrap_or(AxisConvention::GAMEZ_DEFAULT);
    // Built once per level and kept: `enemies` removes its copy of the
    // resource with the level, and pickups drop long after it was inserted.
    let id = loaded.data.id();
    if index.as_ref().is_none_or(|(built_for, _)| *built_for != id) {
        *index = Some((id, LevelTemplates::from_level(level)));
    }
    let Some((_, index)) = index.as_ref() else {
        return;
    };

    for (entity, pickup) in &pickups {
        // The pickup itself has no `Visibility`: the sim builds without a
        // renderer. The clone under it needs one to inherit.
        commands
            .entity(entity)
            .insert((PickupModel, Visibility::default()));

        let Some(root) = template_for(&pickup.kind).and_then(|name| index.root(&name)) else {
            warn!(
                "pickup `{}` ({}) has no template in this level; it stays invisible",
                pickup.instance, pickup.kind
            );
            continue;
        };

        let cloned = templates::clone_template(
            &mut commands,
            level,
            &axes,
            root,
            Transform::IDENTITY,
            &format!("{} model", pickup.instance),
        );
        commands.entity(cloned.root).insert(ChildOf(entity));

        if let Some(volume) = cloned.find(VOLUME_NODE) {
            commands.entity(volume).insert(NodeActive(false));
        }
    }
}

/// Hide a taken pickup until it respawns.
///
/// `RespawnTimer` is the sim's "taken" marker (deathmatch only; a campaign
/// pickup is despawned).
pub fn hide_taken_pickups(
    mut pickups: Query<(&mut Visibility, Has<RespawnTimer>), With<PickupModel>>,
) {
    for (mut visibility, taken) in &mut pickups {
        let wanted = if taken {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// Give a taken pickup a `Visibility` for its model to inherit.
///
/// The sim moves the model onto a [`TakenPickup`] to play its way into the
/// vehicle, and the sim builds without a renderer.
pub fn show_taken_pickups(
    mut commands: Commands,
    taken: Query<Entity, (With<TakenPickup>, Without<Visibility>)>,
) {
    for entity in &taken {
        commands.entity(entity).insert(Visibility::default());
    }
}

/// Clone a model again for a pickup that has come back.
///
/// Its first model went into the vehicle that took it, so a
/// deathmatch pickup returns with nothing to show until it gets another.
pub fn reclone_respawned_pickups(
    mut commands: Commands,
    mut respawned: RemovedComponents<RespawnTimer>,
    pickups: Query<(), With<PickupModel>>,
) {
    for entity in respawned.read() {
        if pickups.contains(entity) {
            commands.entity(entity).remove::<PickupModel>();
        }
    }
}
