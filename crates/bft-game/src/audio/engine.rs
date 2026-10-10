//! Engine sounds: a loop on every vehicle near enough to hear, pitched by
//! speed.
//!
//! The sound, its `pitch_scale` and its `volume_scale` are per form in
//! `vehicle.zrd` (`<form>_mode/sounds`), so a form change swaps the loop. The
//! player's engine always plays; an enemy's only within
//! [`SoundTuning::engine_range`] of the listener, since 83 engines idling
//! across m1 would be both inaudible and wasted mixing.

use avian3d::prelude::LinearVelocity;
use bevy::audio::PlaybackSettings;
use bevy::prelude::*;
use bft_assets::sounds::SoundBank;
use bft_sim::states::AppState;
use bft_sim::vehicle::{PlayerVehicle, Vehicle, VehicleTable};

use super::{AudioSystems, PlaySound, PlayingSound, SoundLibrary, SoundTuning, resolve_sound};

/// A vehicle's engine loop. A child of the vehicle.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct EngineSound {
    /// The logical name playing, so a form change can tell it needs another.
    pub name: String,
    /// The record's `VOLUME`, which speed then scales.
    pub base_gain: f32,
}

pub struct EnginePlugin;

impl Plugin for EnginePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (manage_engines, drive_engines)
                .chain()
                .after(AudioSystems::Play)
                .before(AudioSystems::Gain)
                .run_if(in_state(AppState::InGame)),
        );
    }
}

/// Playback rate and loudness for a vehicle moving at `speed_fraction` of
/// its top speed.
///
/// Rate runs from `idle_pitch` at a standstill to `idle_pitch +
/// pitch_scale` flat out; loudness from `idle_volume × volume_scale` to
/// `volume_scale`.
pub fn engine_voice(
    speed_fraction: f32,
    pitch_scale: f32,
    volume_scale: f32,
    tuning: &SoundTuning,
) -> (f32, f32) {
    let t = speed_fraction.clamp(0.0, 1.0);
    let pitch = tuning.engine_idle_pitch + pitch_scale * t;
    let volume = volume_scale * (tuning.engine_idle_volume + (1.0 - tuning.engine_idle_volume) * t);
    (pitch.max(0.05), volume)
}

/// Start, swap and stop engine loops.
#[allow(clippy::too_many_arguments)]
fn manage_engines(
    mut commands: Commands,
    table: Option<Res<VehicleTable>>,
    library: Res<SoundLibrary>,
    banks: Res<Assets<SoundBank>>,
    tuning: Res<SoundTuning>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    vehicles: Query<(Entity, &Vehicle, &GlobalTransform, Has<PlayerVehicle>)>,
    engines: Query<(Entity, &EngineSound, &ChildOf)>,
) {
    let Some(table) = table else {
        return;
    };
    let Some(bank) = library.bank.as_ref().and_then(|h| banks.get(h)) else {
        return;
    };
    let ear = listener.iter().next().map(GlobalTransform::translation);

    for (vehicle_entity, vehicle, transform, is_player) in &vehicles {
        let in_range = is_player
            || ear.is_some_and(|ear| ear.distance(transform.translation()) <= tuning.engine_range);
        let wanted = in_range
            .then(|| table.kind(&vehicle.kind))
            .flatten()
            .and_then(|def| def.mode(vehicle.form))
            .and_then(|mode| mode.sounds.engine.clone());

        let current = engines
            .iter()
            .find(|(_, _, child_of)| child_of.parent() == vehicle_entity);

        if let Some((entity, engine, _)) = current {
            if wanted.as_deref() == Some(engine.name.as_str()) {
                continue;
            }
            commands.entity(entity).despawn();
        }

        let Some(name) = wanted else {
            continue;
        };
        let request = PlaySound::on(&name, vehicle_entity).looping();
        let Ok(resolved) = resolve_sound(&request, &library, Some(bank), &tuning) else {
            continue;
        };
        let base_gain = resolved.playing.gain;
        commands.spawn((
            Name::new(format!("engine {name}")),
            AudioPlayer(resolved.source),
            resolved.settings,
            resolved.playing,
            EngineSound { name, base_gain },
            Transform::default(),
            ChildOf(vehicle_entity),
        ));
    }
}

/// An engine loop, its gain, its sinks once playing, and its start rate.
type EngineVoice<'a> = (
    &'a EngineSound,
    &'a ChildOf,
    &'a mut PlayingSound,
    Option<&'a mut AudioSink>,
    Option<&'a mut SpatialAudioSink>,
    &'a mut PlaybackSettings,
);

/// Pitch and loudness from speed, every frame.
fn drive_engines(
    table: Option<Res<VehicleTable>>,
    tuning: Res<SoundTuning>,
    vehicles: Query<(&Vehicle, Option<&LinearVelocity>)>,
    mut engines: Query<EngineVoice>,
) {
    let Some(table) = table else {
        return;
    };

    for (engine, child_of, mut playing, sink, spatial, mut settings) in &mut engines {
        let Ok((vehicle, velocity)) = vehicles.get(child_of.parent()) else {
            continue;
        };
        let Some(mode) = table
            .kind(&vehicle.kind)
            .and_then(|def| def.mode(vehicle.form))
        else {
            continue;
        };
        let speed = velocity.map_or(0.0, |v| v.0.length());
        let fraction = if mode.max_speed > 0.0 {
            speed / mode.max_speed
        } else {
            0.0
        };
        let (pitch, volume) = engine_voice(
            fraction,
            mode.sounds.pitch_scale.unwrap_or(0.0),
            mode.sounds.volume_scale.unwrap_or(1.0),
            &tuning,
        );

        playing.gain = engine.base_gain * volume;
        // Before the sink exists the settings carry the rate, so the loop
        // starts at the right pitch rather than jumping on its second frame.
        if settings.speed != pitch && sink.is_none() && spatial.is_none() {
            settings.speed = pitch;
        }
        if let Some(sink) = sink {
            sink.set_speed(pitch);
        }
        if let Some(sink) = spatial {
            sink.set_speed(pitch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_engine_runs_at_the_base_rate_and_a_fast_one_above_it() {
        let tuning = SoundTuning::default();
        // The player's track mode: pitch_scale 0.5, volume_scale 0.9.
        let (idle_pitch, idle_volume) = engine_voice(0.0, 0.5, 0.9, &tuning);
        let (full_pitch, full_volume) = engine_voice(1.0, 0.5, 0.9, &tuning);

        assert_eq!(idle_pitch, tuning.engine_idle_pitch);
        assert!((full_pitch - (tuning.engine_idle_pitch + 0.5)).abs() < 1e-6);
        assert!((idle_volume - 0.9 * tuning.engine_idle_volume).abs() < 1e-6);
        assert!((full_volume - 0.9).abs() < 1e-6);
    }

    #[test]
    fn overspeed_does_not_run_the_pitch_away() {
        let tuning = SoundTuning::default();
        assert_eq!(
            engine_voice(3.0, 0.5, 1.0, &tuning),
            engine_voice(1.0, 0.5, 1.0, &tuning)
        );
    }
}
