//! Sim messages → [`PlaySound`]s.
//!
//! Every sound here is chosen by the data where the data chooses one:
//! `weapons.zrd` for fire, flight and impact, `pickup.zrd` for pickups,
//! `vehicle.zrd` for the weapon-select click, and the anim VM's `Sound` and
//! `SoundNode` events for everything scripted. `sounds.ron` fills the two
//! gaps — destruction and form change ([`SoundTuning`]).
//!
//! The sim writes these messages in `FixedUpdate`; these systems read them in
//! `Update`. A projectile is read through its `Transform` rather than its
//! `GlobalTransform`, because it was spawned this frame and transform
//! propagation (in `PostUpdate`) has not reached it yet.

use avian3d::prelude::ColliderAabb;
use bevy::prelude::*;
use bft_sim::anim::AnimOutput;
use bft_sim::damage::Destroyed;
use bft_sim::pickups::{PickedUp, PickupTable};
use bft_sim::states::AppState;
use bft_sim::vehicle::{FormChanged, Vehicle, VehicleTable};
use bft_sim::weapons::{
    MineBounced, Mover, ProjectileHit, ProjectileSpawned, Surface, WeaponSelected, WeaponTable,
};
use rand::seq::IndexedRandom;

use super::{AudioSystems, PlaySound, SoundLibrary, SoundTuning};

/// The systems that listen to the sim.
pub struct BridgesPlugin;

impl Plugin for BridgesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                weapon_sounds,
                bounce_sounds,
                destruction_sounds,
                pickup_sounds,
                form_change_sounds,
                weapon_select_sounds,
                anim_sounds,
            )
                .in_set(AudioSystems::Bridge)
                .run_if(in_state(AppState::InGame)),
        );
    }
}

/// Fire, flight and impact, all from `weapons.zrd`.
///
/// A beam weapon has no projectile: its [`ProjectileHit`] names the shooter
/// as the projectile, and is also where its fire sound comes from.
pub fn weapon_sounds(
    mut spawned: MessageReader<ProjectileSpawned>,
    mut hits: MessageReader<ProjectileHit>,
    mut play: MessageWriter<PlaySound>,
    table: Option<Res<WeaponTable>>,
    transforms: Query<&Transform>,
) {
    let Some(table) = table else {
        spawned.clear();
        hits.clear();
        return;
    };

    for event in spawned.read() {
        let Some(def) = table.by_index(event.weapon) else {
            continue;
        };
        let Ok(at) = transforms.get(event.entity) else {
            continue;
        };
        if let Some(sound) = &def.fire.sound {
            play.write(PlaySound::at(sound, at.translation));
        }
        // The in-flight loop rides the projectile and dies with it.
        if let Some(sound) = &def.flyout.sound {
            play.write(PlaySound::on(sound, event.entity).looping());
        }
    }

    let mut rng = rand::rng();
    for hit in hits.read() {
        let Some(def) = table.by_index(hit.weapon) else {
            continue;
        };
        if def.mover == Mover::Beam
            && let Some(sound) = &def.fire.sound
        {
            play.write(PlaySound::on(sound, hit.projectile));
        }

        // Lava and anything else without its own block falls back to
        // `default`, which every weapon has.
        let impact = def
            .impact
            .get(&hit.surface)
            .or_else(|| def.impact.get(&Surface::Default));
        if let Some(sound) = impact.and_then(|impact| impact.sounds.choose(&mut rng)) {
            play.write(PlaySound::at(sound, hit.at));
        }
    }
}

/// A mine hitting the ground or a wall: the surface's `BOUNCE_SOUND`, or the
/// weapon's own `BOUNCE/SOUND` where the surface has none.
pub fn bounce_sounds(
    mut bounces: MessageReader<MineBounced>,
    mut play: MessageWriter<PlaySound>,
    table: Option<Res<WeaponTable>>,
) {
    let Some(table) = table else {
        bounces.clear();
        return;
    };

    for bounce in bounces.read() {
        let Some(def) = table.by_index(bounce.weapon) else {
            continue;
        };
        let sound = def
            .impact
            .get(&bounce.surface)
            .and_then(|impact| impact.bounce_sound.as_ref())
            .or(def.bounce_sound.as_ref());
        if let Some(sound) = sound {
            play.write(PlaySound::at(sound, bounce.at));
        }
    }
}

/// An explosion where something died, sized by its collider.
pub fn destruction_sounds(
    mut destroyed: MessageReader<Destroyed>,
    mut play: MessageWriter<PlaySound>,
    tuning: Res<SoundTuning>,
    bounds: Query<&ColliderAabb>,
) {
    let mut rng = rand::rng();
    for event in destroyed.read() {
        let list = if is_large(bounds.get(event.entity).ok(), tuning.large_radius) {
            &tuning.destroyed_large
        } else {
            &tuning.destroyed_small
        };
        if let Some(sound) = list.choose(&mut rng) {
            play.write(PlaySound::at(sound, event.at));
        }
    }
}

/// Whether a destroyed thing is big enough for the large explosion.
///
/// Something already gone, or with no collider, counts as small.
pub fn is_large(bounds: Option<&ColliderAabb>, large_radius: f32) -> bool {
    bounds.is_some_and(|aabb| {
        let half = (aabb.max - aabb.min) * 0.5;
        half.max_element() >= large_radius
    })
}

/// `pickup.zrd`'s sound for the kind, on whoever picked it up.
pub fn pickup_sounds(
    mut picked: MessageReader<PickedUp>,
    mut play: MessageWriter<PlaySound>,
    table: Option<Res<PickupTable>>,
) {
    let Some(table) = table else {
        picked.clear();
        return;
    };
    for event in picked.read() {
        if let Some(sound) = table.get(&event.kind).and_then(|def| def.sound.as_ref()) {
            play.write(PlaySound::on(sound, event.by));
        }
    }
}

/// The transform sound for the form being entered.
pub fn form_change_sounds(
    mut changed: MessageReader<FormChanged>,
    mut play: MessageWriter<PlaySound>,
    tuning: Res<SoundTuning>,
) {
    for event in changed.read() {
        if let Some(sound) = tuning.form_change.get(&event.to) {
            play.write(PlaySound::on(sound, event.entity));
        }
    }
}

/// `vehicle.zrd`'s `weapon_select` for the vehicle's kind.
pub fn weapon_select_sounds(
    mut selected: MessageReader<WeaponSelected>,
    mut play: MessageWriter<PlaySound>,
    table: Option<Res<VehicleTable>>,
    vehicles: Query<&Vehicle>,
) {
    let Some(table) = table else {
        selected.clear();
        return;
    };
    for event in selected.read() {
        let sound = vehicles
            .get(event.entity)
            .ok()
            .and_then(|vehicle| table.kind(&vehicle.kind))
            .and_then(|def| def.sounds.weapon_select.as_ref());
        if let Some(sound) = sound {
            play.write(PlaySound::on(sound, event.entity));
        }
    }
}

/// The anim VM's `Sound` and `SoundNode` events.
///
/// Both play at the event's own node where it names one the VM could
/// resolve, else at the definition's root, offset by the event's `pos`.
/// Whether it loops is the sound record's `LOOPED` — every `SoundNode` in
/// the game is a looping machine or ambience bar one — and a loop lasts as
/// long as its node, falling silent while the node is switched off.
pub fn anim_sounds(
    mut outputs: MessageReader<AnimOutput>,
    mut play: MessageWriter<PlaySound>,
    library: Res<SoundLibrary>,
) {
    for output in outputs.read() {
        if !matches!(output.kind, "Sound" | "SoundNode") {
            continue;
        }
        if let Some(request) = anim_sound_request(output, &library) {
            play.write(request);
        }
    }
}

/// The [`PlaySound`] one anim sound event asks for.
pub fn anim_sound_request(output: &AnimOutput, library: &SoundLibrary) -> Option<PlaySound> {
    let name = output.value.get("name")?.as_str()?;
    let looped = library.table.get(name).is_some_and(|def| def.looped);

    let place = output
        .value
        .get("at_node")
        .or_else(|| output.value.get("translate").and_then(|t| t.get("AtNode")));
    let offset = place
        .and_then(|place| place.get("pos"))
        .map(|pos| {
            let axis = |key: &str| pos.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            Vec3::new(axis("x"), axis("y"), axis("z"))
        })
        .unwrap_or(Vec3::ZERO);

    Some(PlaySound {
        name: name.to_string(),
        at: Some(offset),
        attached: output.node.or(output.at),
        looped,
        volume: 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::reader::sounds::SoundDef;

    fn library_with(name: &str, looped: bool) -> SoundLibrary {
        let mut library = SoundLibrary::default();
        library.table.sounds.insert(
            name.to_string(),
            SoundDef {
                name: name.to_string(),
                file: "x".into(),
                set: "COMMON".into(),
                positional: true,
                looped,
                range: None,
                volume: 1.0,
                frequency: false,
                voice: false,
                purgeable: false,
                hardware: false,
            },
        );
        library
    }

    fn output(
        kind: &'static str,
        value: serde_json::Value,
        at: Entity,
        node: Option<Entity>,
    ) -> AnimOutput {
        AnimOutput {
            kind,
            value,
            def: "def".into(),
            at: Some(at),
            node,
        }
    }

    #[test]
    fn a_sound_event_plays_at_its_node_with_its_offset() {
        let root = Entity::from_raw_u32(1).expect("valid");
        let turret = Entity::from_raw_u32(2).expect("valid");
        let request = anim_sound_request(
            &output(
                "Sound",
                serde_json::json!({ "name": "snd_hit",
                    "at_node": { "name": "turret", "pos": { "x": 0.0, "y": 1.0, "z": -1.0 } } }),
                root,
                Some(turret),
            ),
            &library_with("snd_hit", false),
        )
        .expect("a request");

        assert_eq!(request.attached, Some(turret));
        assert_eq!(request.at, Some(Vec3::new(0.0, 1.0, -1.0)));
        assert!(!request.looped);
    }

    #[test]
    fn a_sound_node_loops_when_its_record_does_and_falls_back_to_the_root() {
        let root = Entity::from_raw_u32(1).expect("valid");
        let request = anim_sound_request(
            &output(
                "SoundNode",
                serde_json::json!({ "name": "snd_fire1", "active_state": true, "translate": null }),
                root,
                None,
            ),
            &library_with("snd_fire1", true),
        )
        .expect("a request");

        assert_eq!(request.attached, Some(root));
        assert_eq!(request.at, Some(Vec3::ZERO));
        assert!(request.looped);
    }

    #[test]
    fn a_large_collider_takes_the_large_explosion() {
        let small = ColliderAabb::new(Vec3::ZERO, Vec3::splat(1.0));
        let large = ColliderAabb::new(Vec3::ZERO, Vec3::new(5.0, 1.0, 1.0));
        assert!(!is_large(Some(&small), 4.0));
        assert!(is_large(Some(&large), 4.0));
        assert!(!is_large(None, 4.0));
    }
}
