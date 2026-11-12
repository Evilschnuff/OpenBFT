//! Engine sounds: a loop on every vehicle near enough to hear, pitched by
//! speed.
//!
//! The sound, its `pitch_scale` and its `volume_scale` are per form in
//! `vehicle.zrd` (`<form>_mode/sounds`), so a form change swaps the loop. The
//! player's engine always plays; an enemy's only within
//! [`SoundTuning::engine_range`] of the listener, since 83 engines idling
//! across m1 would be both inaudible and wasted mixing.
//!
//! The loop plays through [`varispeed`](super::varispeed), not rodio's own
//! speed control, which only takes a new rate when its resampler starts a new
//! span and so moves the pitch in audible steps. The original's engines are
//! `FREQUENCY` records whose rate it changes as it runs, and that is heard as
//! a smooth sweep with the tank's speed.
//!
//! A destroyed vehicle has no engine. Its entity outlives it, because the
//! wreck stays in the world and the death anim plays on its model, so its loop
//! is stopped when its health runs out, not when the entity goes.
//! // approximation: in the original a wreck makes no engine noise (from
//! play, not yet an observation entry); that it stops at once rather than
//! winding down is the simplest reading.

use std::sync::Arc;

use avian3d::prelude::LinearVelocity;
use bevy::audio::{AudioSource, PlaybackMode};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::sounds::SoundBank;
use bft_sim::damage::Health;
use bft_sim::states::AppState;
use bft_sim::vehicle::{PlayerVehicle, Vehicle, VehicleTable};

use super::varispeed::{LoopSamples, RateControl, VariLoop};
use super::{AudioSystems, PlaySound, PlayingSound, SoundLibrary, SoundTuning, resolve_sound};

/// A vehicle's engine loop. A child of the vehicle.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct EngineSound {
    /// The logical name playing, so a form change can tell it needs another.
    pub name: String,
    /// The record's `VOLUME`, which speed then scales.
    pub base_gain: f32,
    /// The loop's playback rate, set from speed every frame.
    pub rate: RateControl,
}

/// Engine WAVs decoded for [`VariLoop`], by the sound they came from, so
/// every engine playing one shares its samples. `None` for one that would
/// not decode, so it is not tried again.
#[derive(Resource, Debug, Default)]
pub struct DecodedLoops(HashMap<AssetId<AudioSource>, Option<Arc<LoopSamples>>>);

impl DecodedLoops {
    /// The samples of `source`, decoding them the first time. `None` while
    /// the sound has not loaded, and for one that does not decode.
    pub fn get(
        &mut self,
        source: &Handle<AudioSource>,
        sources: Option<&Assets<AudioSource>>,
    ) -> Option<Arc<LoopSamples>> {
        if let Some(found) = self.0.get(&source.id()) {
            return found.clone();
        }
        // Not loaded yet: asked again next frame.
        let audio = sources?.get(source)?;
        let decoded = LoopSamples::decode(audio).map(Arc::new);
        if decoded.is_none() {
            warn!(
                "engine sound {:?} has no samples; it will be silent",
                source.id()
            );
        }
        self.0.insert(source.id(), decoded.clone());
        decoded
    }
}

pub struct EnginePlugin;

impl Plugin for EnginePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DecodedLoops>().add_systems(
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
/// Rate runs from `engine_idle_pitch` at a standstill and climbs by
/// `pitch_scale × engine_pitch_range` to top speed; loudness runs from
/// `engine_idle_volume × volume_scale` to `volume_scale`, all of it times the
/// player's or the enemies' engine volume.
pub fn engine_voice(
    speed_fraction: f32,
    pitch_scale: f32,
    volume_scale: f32,
    is_player: bool,
    tuning: &SoundTuning,
) -> (f32, f32) {
    let t = speed_fraction.clamp(0.0, 1.0);
    let pitch = tuning.engine_idle_pitch + pitch_scale * tuning.engine_pitch_range * t;
    let volume = tuning.engine_volume(is_player)
        * volume_scale
        * (tuning.engine_idle_volume + (1.0 - tuning.engine_idle_volume) * t);
    (pitch.max(0.05), volume.max(0.0))
}

/// How much of its top speed a vehicle is doing; what [`engine_voice`] takes.
/// Zero for a mode with no top speed.
pub fn speed_fraction(speed: f32, max_speed: f32) -> f32 {
    if max_speed > 0.0 {
        speed / max_speed
    } else {
        0.0
    }
}

/// Whether a vehicle's engine is running where it can be heard.
///
/// Only while it is alive, and then always for the player's and for anyone
/// else's within `range` of the listener. `distance` is to the listener,
/// `None` when there is none.
pub fn engine_audible(is_player: bool, alive: bool, distance: Option<f32>, range: f32) -> bool {
    alive && (is_player || distance.is_some_and(|distance| distance <= range))
}

/// A vehicle that may have an engine loop: where it is, whose it is, and
/// whether it is still alive.
type EngineOwner<'a> = (
    Entity,
    &'a Vehicle,
    &'a GlobalTransform,
    Has<PlayerVehicle>,
    Option<&'a Health>,
);

/// Start, swap and stop engine loops.
#[allow(clippy::too_many_arguments)]
fn manage_engines(
    mut commands: Commands,
    table: Option<Res<VehicleTable>>,
    library: Res<SoundLibrary>,
    banks: Res<Assets<SoundBank>>,
    tuning: Res<SoundTuning>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    vehicles: Query<EngineOwner>,
    engines: Query<(Entity, &EngineSound, &ChildOf)>,
    sources: Option<Res<Assets<AudioSource>>>,
    mut loops: ResMut<Assets<VariLoop>>,
    mut decoded: ResMut<DecodedLoops>,
) {
    let Some(table) = table else {
        return;
    };
    let Some(bank) = library.bank.as_ref().and_then(|h| banks.get(h)) else {
        return;
    };
    let ear = listener.iter().next().map(GlobalTransform::translation);

    for (vehicle_entity, vehicle, transform, is_player, health) in &vehicles {
        let alive = health.is_none_or(|health| !health.is_dead());
        let distance = ear.map(|ear| ear.distance(transform.translation()));
        let wanted = engine_audible(is_player, alive, distance, tuning.engine_range)
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
        let Some(samples) = decoded.get(&resolved.source, sources.as_deref()) else {
            continue;
        };
        // `drive_engines` sets the real rate before the loop starts playing,
        // later this frame.
        let rate = RateControl::new(tuning.engine_idle_pitch);
        let mut settings = resolved.settings;
        // The loop never ends by itself; repeating it would buffer it forever.
        settings.mode = PlaybackMode::Once;
        let base_gain = resolved.playing.gain;
        commands.spawn((
            Name::new(format!("engine {name}")),
            AudioPlayer(loops.add(VariLoop {
                samples,
                rate: rate.clone(),
            })),
            settings,
            resolved.playing,
            EngineSound {
                name,
                base_gain,
                rate,
            },
            Transform::default(),
            ChildOf(vehicle_entity),
        ));
    }
}

/// Pitch and loudness from speed, every frame.
fn drive_engines(
    table: Option<Res<VehicleTable>>,
    tuning: Res<SoundTuning>,
    vehicles: Query<(&Vehicle, Option<&LinearVelocity>, Has<PlayerVehicle>)>,
    mut engines: Query<(&EngineSound, &ChildOf, &mut PlayingSound)>,
) {
    let Some(table) = table else {
        return;
    };

    for (engine, child_of, mut playing) in &mut engines {
        let Ok((vehicle, velocity, is_player)) = vehicles.get(child_of.parent()) else {
            continue;
        };
        let Some(mode) = table
            .kind(&vehicle.kind)
            .and_then(|def| def.mode(vehicle.form))
        else {
            continue;
        };
        let speed = velocity.map_or(0.0, |v| v.0.length());
        let (pitch, volume) = engine_voice(
            speed_fraction(speed, mode.max_speed),
            mode.sounds.pitch_scale.unwrap_or(0.0),
            mode.sounds.volume_scale.unwrap_or(1.0),
            is_player,
            &tuning,
        );

        playing.gain = engine.base_gain * volume;
        engine.rate.set(pitch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_engine_runs_at_the_base_rate_and_a_fast_one_above_it() {
        let tuning = SoundTuning::default();
        // The player's track mode: pitch_scale 0.5, volume_scale 0.9.
        let (idle_pitch, idle_volume) = engine_voice(0.0, 0.5, 0.9, true, &tuning);
        let (full_pitch, full_volume) = engine_voice(1.0, 0.5, 0.9, true, &tuning);

        let gain = tuning.player_engine_volume * 0.9;
        assert_eq!(idle_pitch, tuning.engine_idle_pitch);
        assert!(
            (full_pitch - (tuning.engine_idle_pitch + 0.5 * tuning.engine_pitch_range)).abs()
                < 1e-6
        );
        assert!((idle_volume - gain * tuning.engine_idle_volume).abs() < 1e-5);
        assert!((full_volume - gain).abs() < 1e-5);
    }

    #[test]
    fn the_range_widens_the_climb_and_the_volume_scales_it_all() {
        let tuning = SoundTuning {
            engine_idle_pitch: 0.6,
            engine_pitch_range: 2.0,
            engine_idle_volume: 0.5,
            player_engine_volume: 0.5,
            ..SoundTuning::default()
        };
        let (idle_pitch, idle_volume) = engine_voice(0.0, 0.5, 0.9, true, &tuning);
        let (full_pitch, full_volume) = engine_voice(1.0, 0.5, 0.9, true, &tuning);

        assert!((idle_pitch - 0.6).abs() < 1e-6);
        // 0.6 + 0.5 × 2.0.
        assert!((full_pitch - 1.6).abs() < 1e-6);
        assert!((idle_volume - 0.5 * 0.9 * 0.5).abs() < 1e-6);
        assert!((full_volume - 0.5 * 0.9).abs() < 1e-6);
    }

    /// The player's boost stays on the player's engine.
    #[test]
    fn enemies_take_their_own_engine_volume() {
        let tuning = SoundTuning {
            player_engine_volume: 2.0,
            enemy_engine_volume: 0.5,
            ..SoundTuning::default()
        };
        let (_, player) = engine_voice(1.0, 0.5, 0.8, true, &tuning);
        let (_, enemy) = engine_voice(1.0, 0.5, 0.8, false, &tuning);
        assert!((player - 1.6).abs() < 1e-6);
        assert!((enemy - 0.4).abs() < 1e-6);
    }

    #[test]
    fn a_wreck_is_silent_and_a_far_enemy_unheard() {
        let range = 250.0;
        // The player's is heard anywhere, even with no listener yet.
        assert!(engine_audible(true, true, None, range));
        assert!(engine_audible(true, true, Some(1000.0), range));
        // An enemy's only within range.
        assert!(engine_audible(false, true, Some(range), range));
        assert!(!engine_audible(false, true, Some(range + 1.0), range));
        assert!(!engine_audible(false, true, None, range));
        // Destroyed, nobody's runs, however close.
        assert!(!engine_audible(false, false, Some(5.0), range));
        assert!(!engine_audible(true, false, Some(0.0), range));
    }

    #[test]
    fn overspeed_does_not_run_the_pitch_away() {
        let tuning = SoundTuning::default();
        assert_eq!(
            engine_voice(3.0, 0.5, 1.0, true, &tuning),
            engine_voice(1.0, 0.5, 1.0, true, &tuning)
        );
    }
}
