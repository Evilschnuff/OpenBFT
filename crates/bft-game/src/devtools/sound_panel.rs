//! Live tuning of the engine sounds, by ear against the original.
//!
//! Edits the [`SoundTuning`] resource in place; `drive_engines` reads it
//! every frame, so the idle rate, the climb to top speed and the loudness
//! change the moment a number does, while driving. The readout shows what
//! the player's engine is doing right now, including how close its loudest
//! sample comes to full scale at the speaker.
//!
//! Only the engine knobs, the distance falloff and the mix headroom are here:
//! the first two are what no reader key or observation settles
//! (`audio/tuning.rs` marks them `// approximation`), and the headroom is what
//! keeps a loud engine from clipping. Switching the falloff while a fight
//! goes on is the quickest way to hear which curve sounds like the original. The data's own `pitch_scale` and `volume_scale` stay the data's,
//! per mode; these knobs act on top of them, for every vehicle alike.
//!
//! **Save** rewrites just these values in `assets/tuning/sounds.ron`, keeping
//! its comments. **Revert** goes back to the file as last loaded or saved,
//! **Defaults** to the compiled ones.

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;
use bft_assets::config::AudioSettings;
use bft_sim::vehicle::{PlayerVehicle, Vehicle, VehicleTable};

use super::{DevPanel, egui};
use crate::audio::engine::{engine_voice, speed_fraction};
use crate::audio::tuning::rewrite_values;
use crate::audio::varispeed::VariLoop;
use crate::audio::{EngineSound, Falloff, SOUNDS_TUNING, SoundTuning, SoundTuningHandle};

/// The engine knobs, live.
#[derive(Default)]
pub struct SoundPanel {
    /// What the last save or revert did, shown under the buttons.
    status: Option<String>,
}

impl DevPanel for SoundPanel {
    fn title(&self) -> &str {
        "Sound"
    }

    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let Some(before) = world.get_resource::<SoundTuning>().cloned() else {
            ui.label("No sound tuning resource.");
            return;
        };

        match readout(world, &before) {
            Some(now) => {
                ui.label(now.text);
                if let Some(peak) = now.peak {
                    let line = format!("engine peak at top speed: {peak:.2} of full scale");
                    if peak > 1.0 {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("{line}, clips"));
                    } else {
                        ui.label(line);
                    }
                }
            }
            None => {
                ui.label("No player vehicle. Start a level to hear its engine.");
            }
        }
        ui.separator();

        let mut tuning = before.clone();
        edit(ui, &mut tuning);
        if tuning != before {
            *world.resource_mut::<SoundTuning>() = tuning;
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button("Save to sounds.ron")
                .on_hover_text("Write these values to the tuning file, comments kept.")
                .clicked()
            {
                self.status = Some(match save(world) {
                    Ok(message) => message,
                    Err(message) => format!("not saved: {message}"),
                });
            }
            if ui
                .button("Revert")
                .on_hover_text("Go back to sounds.ron as last loaded or saved.")
                .clicked()
            {
                self.status = Some(match revert(world) {
                    Ok(()) => "reverted to the file".to_string(),
                    Err(message) => format!("not reverted: {message}"),
                });
            }
            if ui
                .button("Defaults")
                .on_hover_text("The compiled values, as if sounds.ron were empty. Not saved.")
                .clicked()
            {
                let mut tuning = world.resource_mut::<SoundTuning>();
                copy_mix_knobs(&mut tuning, &SoundTuning::default());
                self.status = Some("compiled defaults (not saved)".to_string());
            }
        });
        if let Some(status) = &self.status {
            ui.label(status);
        }
    }
}

/// What the readout shows.
struct Readout {
    text: String,
    /// The engine's loudest sample at top speed, as a fraction of full scale
    /// at the speaker. `None` until its loop is playing.
    peak: Option<f32>,
}

/// The player's engine right now: speed, and the rate and gain it gets.
fn readout(world: &mut World, tuning: &SoundTuning) -> Option<Readout> {
    let (player, vehicle, velocity) = world
        .query_filtered::<(Entity, &Vehicle, Option<&LinearVelocity>), With<PlayerVehicle>>()
        .iter(world)
        .next()
        .map(|(entity, vehicle, velocity)| {
            (
                entity,
                vehicle.clone(),
                velocity.map_or(0.0, |v| v.0.length()),
            )
        })?;
    let table = world.get_resource::<VehicleTable>()?;
    let mode = table.kind(&vehicle.kind)?.mode(vehicle.form)?;
    let sounds = &mode.sounds;
    let pitch_scale = sounds.pitch_scale.unwrap_or(0.0);
    let volume_scale = sounds.volume_scale.unwrap_or(1.0);

    let fraction = speed_fraction(velocity, mode.max_speed);
    let (rate, gain) = engine_voice(fraction, pitch_scale, volume_scale, true, tuning);
    let (idle_rate, idle_gain) = engine_voice(0.0, pitch_scale, volume_scale, true, tuning);
    let (top_rate, top_gain) = engine_voice(1.0, pitch_scale, volume_scale, true, tuning);

    let text = format!(
        "{:?} `{}`   {:.1} m/s ({:.0}% of top)\n\
         now: rate {:.2}×  gain {:.2}\n\
         idle: rate {:.2}×  gain {:.2}     top: rate {:.2}×  gain {:.2}\n\
         data: pitch_scale {:.2}  volume_scale {:.2}",
        vehicle.form,
        sounds.engine.as_deref().unwrap_or("no engine"),
        velocity,
        fraction.min(1.0) * 100.0,
        rate,
        gain,
        idle_rate,
        idle_gain,
        top_rate,
        top_gain,
        pitch_scale,
        volume_scale,
    );
    let peak = engine_peak(world, player, top_gain, tuning);
    Some(Readout { text, peak })
}

/// How loud the player's engine gets at the speaker, as a fraction of full
/// scale: its WAV's loudest sample through every gain on the way, at top
/// speed and close to the listener.
fn engine_peak(
    world: &mut World,
    player: Entity,
    top_gain: f32,
    tuning: &SoundTuning,
) -> Option<f32> {
    let (base_gain, handle) = world
        .query::<(&EngineSound, &ChildOf, &AudioPlayer<VariLoop>)>()
        .iter(world)
        .find(|(_, child_of, _)| child_of.parent() == player)
        .map(|(engine, _, audio)| (engine.base_gain, audio.0.clone()))?;
    let peak = world
        .resource::<Assets<VariLoop>>()
        .get(&handle)?
        .samples
        .peak();
    let setting = world
        .get_resource::<AudioSettings>()
        .map_or(1.0, AudioSettings::effects_gain);
    Some(peak * base_gain * top_gain * setting * tuning.mix_headroom)
}

/// The editable knobs.
fn edit(ui: &mut egui::Ui, tuning: &mut SoundTuning) {
    egui::Grid::new("engine_knobs")
        .num_columns(2)
        .show(ui, |ui| {
            let hint = "How every 3D sound fades between its RANGE distances. Linear: \
                        straight down to silence at the second. 1/distance: halving with \
                        each doubling past the first (DirectSound 3D's rule), silent past \
                        the second.";
            ui.label("falloff").on_hover_text(hint);
            ui.horizontal(|ui| {
                ui.radio_value(&mut tuning.falloff, Falloff::Linear, "linear")
                    .on_hover_text(hint);
                ui.radio_value(&mut tuning.falloff, Falloff::Inverse, "1/distance")
                    .on_hover_text(hint);
            });
            ui.end_row();
            row(
                ui,
                "engine_idle_pitch",
                &mut tuning.engine_idle_pitch,
                0.01,
                "Playback rate at a standstill, 1 being the WAV's own. Lower is deeper.",
            );
            row(
                ui,
                "engine_pitch_range",
                &mut tuning.engine_pitch_range,
                0.01,
                "Factor on the mode's pitch_scale: how far the rate climbs to top speed.",
            );
            row(
                ui,
                "engine_idle_volume",
                &mut tuning.engine_idle_volume,
                0.01,
                "Loudness at a standstill, as a fraction of the loudness at top speed.",
            );
            row(
                ui,
                "player_engine_volume",
                &mut tuning.player_engine_volume,
                0.01,
                "Gain on your own engine, against the other sounds.",
            );
            row(
                ui,
                "enemy_engine_volume",
                &mut tuning.enemy_engine_volume,
                0.01,
                "Gain on every other vehicle's engine. 1 is the data's own balance.",
            );
            row(
                ui,
                "mix_headroom",
                &mut tuning.mix_headroom,
                0.01,
                "Gain on everything, effects and music alike, 0..1. Keeps the loudest sound under \
                 full scale; the system volume makes up the level.",
            );
            row(
                ui,
                "engine_range",
                &mut tuning.engine_range,
                5.0,
                "m. Enemy engines are heard within this.",
            );
        });
    tuning.engine_idle_pitch = tuning.engine_idle_pitch.max(0.05);
    tuning.engine_pitch_range = tuning.engine_pitch_range.max(0.0);
    tuning.engine_idle_volume = tuning.engine_idle_volume.clamp(0.0, 1.0);
    tuning.player_engine_volume = tuning.player_engine_volume.max(0.0);
    tuning.enemy_engine_volume = tuning.enemy_engine_volume.max(0.0);
    tuning.engine_range = tuning.engine_range.max(0.0);
    tuning.mix_headroom = tuning.mix_headroom.clamp(0.0, 1.0);
}

/// One labelled number.
fn row(ui: &mut egui::Ui, label: &str, value: &mut f32, speed: f64, hint: &str) {
    ui.label(label).on_hover_text(hint);
    ui.add(egui::DragValue::new(value).speed(speed).max_decimals(3))
        .on_hover_text(hint);
    ui.end_row();
}

/// Set `to`'s engine and mix knobs from `from`, leaving the rest alone.
fn copy_mix_knobs(to: &mut SoundTuning, from: &SoundTuning) {
    to.engine_range = from.engine_range;
    to.engine_idle_pitch = from.engine_idle_pitch;
    to.engine_pitch_range = from.engine_pitch_range;
    to.engine_idle_volume = from.engine_idle_volume;
    to.player_engine_volume = from.player_engine_volume;
    to.enemy_engine_volume = from.enemy_engine_volume;
    to.mix_headroom = from.mix_headroom;
    to.falloff = from.falloff;
}

/// Write the live engine and mix knobs to `sounds.ron`.
fn save(world: &mut World) -> Result<String, String> {
    let tuning = world.resource::<SoundTuning>().clone();

    let path = bft_assets::asset_directory().join(SOUNDS_TUNING);
    let existing =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = rewrite_values(&existing, &tuning.mix_knobs())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;

    // Keep the loaded asset in step with the file, so a revert goes back to
    // what was just saved.
    let handle = world
        .get_resource::<SoundTuningHandle>()
        .map(|handle| handle.0.clone());
    if let Some(handle) = handle
        && let Some(mut asset) = world.resource_mut::<Assets<SoundTuning>>().get_mut(&handle)
    {
        copy_mix_knobs(&mut asset, &tuning);
    }

    let defaults = SoundTuning::default();
    let note = if tuning.mix_knobs() == defaults.mix_knobs() {
        ""
    } else {
        "; the compiled defaults in audio/tuning.rs still hold the old values"
    };
    Ok(format!("saved to {}{note}", path.display()))
}

/// Back to the file as last loaded or saved.
fn revert(world: &mut World) -> Result<(), String> {
    let handle = world
        .get_resource::<SoundTuningHandle>()
        .ok_or("sounds.ron was never requested")?
        .0
        .clone();
    let file = world
        .resource::<Assets<SoundTuning>>()
        .get(&handle)
        .ok_or("sounds.ron has not loaded")?
        .clone();

    copy_mix_knobs(&mut world.resource_mut::<SoundTuning>(), &file);
    Ok(())
}
