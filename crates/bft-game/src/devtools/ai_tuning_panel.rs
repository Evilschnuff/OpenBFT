//! Live tuning of the enemy AI's own knobs.
//!
//! Edits the [`SteeringTuning`] resource in place; `restyle_brains` hands the
//! change to every brain already in the world, with `ai.ron`'s per-kind
//! layer re-applied, and the turrets read the resource directly. So waking,
//! perception, aim and steering change the moment a number does.
//!
//! Only *our* knobs are here — the ones `tuning.rs` marks `// approximation`.
//! The data's own radii, dwells and strategies are per net and stay the
//! data's; `activation_scale` is the one lever on them, as a factor.
//!
//! **Save** writes the knobs to `assets/tuning/ai.ron`: every one that
//! differs from the compiled default is written, every one that matches is
//! cleared, and the file's comment header and its per-kind, per-net and
//! per-turret sections are kept. **Revert** goes back to the file as last
//! loaded or saved.

use bevy::prelude::*;
use bft_sim::ai::{AiBrain, AiOverrides, AiState, SteeringTuning};

use super::{DevPanel, egui};
use crate::enemies::{AI_TUNING, AiTuningHandle};

/// The AI's knobs, live.
#[derive(Default)]
pub struct AiTuningPanel {
    /// What the last save or revert did, shown under the buttons.
    status: Option<String>,
}

impl DevPanel for AiTuningPanel {
    fn title(&self) -> &str {
        "AI tuning"
    }

    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let Some(before) = world.get_resource::<SteeringTuning>().copied() else {
            ui.label("No AI tuning resource.");
            return;
        };

        ui.label(census(world));
        ui.separator();

        let mut tuning = before;
        egui::ScrollArea::vertical()
            .id_salt("ai_tuning")
            .auto_shrink([false, true])
            .show(ui, |ui| edit(ui, &mut tuning));

        // Only on a real change, so `restyle_brains` is not woken every frame
        // the panel is open.
        if tuning != before {
            tuning.reaction_ticks = tuning.reaction_ticks.max(1);
            tuning.los_lost_ticks = tuning.los_lost_ticks.max(1);
            *world.resource_mut::<SteeringTuning>() = tuning;
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button("Save to ai.ron")
                .on_hover_text("Write every knob that differs from the default to the tuning file.")
                .clicked()
            {
                self.status = Some(match save(world) {
                    Ok(message) => message,
                    Err(message) => format!("not saved: {message}"),
                });
            }
            if ui
                .button("Revert")
                .on_hover_text("Go back to ai.ron as last loaded or saved.")
                .clicked()
            {
                self.status = Some(match revert(world) {
                    Ok(()) => "reverted to the file".to_string(),
                    Err(message) => format!("not reverted: {message}"),
                });
            }
            if ui
                .button("Defaults")
                .on_hover_text("The compiled defaults, as if ai.ron were empty. Not saved.")
                .clicked()
            {
                *world.resource_mut::<SteeringTuning>() = SteeringTuning::default();
                self.status = Some("compiled defaults (not saved)".to_string());
            }
        });
        if let Some(status) = &self.status {
            ui.label(status);
        }
    }
}

/// How many brains are in each state: the quickest read on "are they doing
/// anything".
fn census(world: &mut World) -> String {
    let mut counts = [0usize; 7];
    let order = [
        AiState::Dormant,
        AiState::Roam,
        AiState::Patrol,
        AiState::Engage,
        AiState::Return,
        AiState::Hide,
        AiState::Dead,
    ];

    for brain in world.query::<&AiBrain>().iter(world) {
        if let Some(index) = order.iter().position(|state| *state == brain.state) {
            counts[index] += 1;
        }
    }

    order
        .iter()
        .zip(counts)
        .map(|(state, count)| format!("{state:?} {count}"))
        .collect::<Vec<_>>()
        .join("   ")
}

/// The editable knobs, grouped by what they govern.
fn edit(ui: &mut egui::Ui, tuning: &mut SteeringTuning) {
    section(ui, "Waking", |ui| {
        row(
            ui,
            "activation_scale",
            &mut tuning.activation_scale,
            0.05,
            "Factor on every net's activate_rad. 1 is the data's; raise it if enemies wake too late.",
        );
        row(
            ui,
            "roam_range",
            &mut tuning.roam_range,
            5.0,
            "m. A sleeping unit walks its net while the player is this near, without hunting. 0 keeps it parked.",
        );
    });

    section(ui, "Perception", |ui| {
        row(
            ui,
            "reaction_ticks",
            &mut tuning.reaction_ticks,
            0.1,
            "Ticks between perception passes (60 a second).",
        );
        row(
            ui,
            "los_lost_ticks",
            &mut tuning.los_lost_ticks,
            0.1,
            "Passes without line of sight before the target is dropped (the data's not_pursuit_dwell must also pass).",
        );
    });

    section(ui, "Aim", |ui| {
        row(
            ui,
            "lead_factor",
            &mut tuning.lead_factor,
            0.01,
            "How much of the interception lead to apply. 1 is perfect.",
        );
        row(
            ui,
            "aim_error_deg",
            &mut tuning.aim_error_deg,
            0.05,
            "Largest random aim error, degrees.",
        );
        row(
            ui,
            "fire_cone_deg",
            &mut tuning.fire_cone_deg,
            0.1,
            "How near the barrel has to be to the aim before firing, degrees. Keep above aim_error_deg.",
        );
    });

    section(ui, "Steering", |ui| {
        row(
            ui,
            "arrive_radius",
            &mut tuning.arrive_radius,
            0.5,
            "m. How close to a waypoint counts as reaching it.",
        );
        row(
            ui,
            "steer_gain",
            &mut tuning.steer_gain,
            0.1,
            "Higher turns harder for the same heading error.",
        );
        row(
            ui,
            "reverse_angle_deg",
            &mut tuning.reverse_angle_deg,
            1.0,
            "Heading error past which the goal counts as behind.",
        );
        row(
            ui,
            "reverse_max_distance",
            &mut tuning.reverse_max_distance,
            1.0,
            "m. A goal behind and nearer than this is backed up to; further, it turns round going forwards.",
        );
        row(
            ui,
            "min_engage_speed",
            &mut tuning.min_engage_speed,
            0.01,
            "Throttle floor while driving, 0..1.",
        );
        row(
            ui,
            "strafe_period",
            &mut tuning.strafe_period,
            0.1,
            "s per leg of a back_forth strafe.",
        );
    });

    section(ui, "Whiskers", |ui| {
        row(
            ui,
            "whisker_length",
            &mut tuning.whisker_length,
            0.5,
            "m. 0 turns wall avoidance off.",
        );
        row(
            ui,
            "whisker_angle_deg",
            &mut tuning.whisker_angle_deg,
            1.0,
            "Angle of the two side whiskers.",
        );
        row(
            ui,
            "whisker_height",
            &mut tuning.whisker_height,
            0.1,
            "m above the hull origin.",
        );
    });

    section(ui, "Turrets", |ui| {
        row(
            ui,
            "turret_yaw_rate",
            &mut tuning.turret_yaw_rate,
            0.05,
            "rad/s traverse of an ai.zrd emplacement.",
        );
        row(
            ui,
            "turret_pitch_rate",
            &mut tuning.turret_pitch_rate,
            0.05,
            "rad/s elevation.",
        );
    });
}

/// A collapsible group of rows.
fn section(ui: &mut egui::Ui, title: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(title)
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new(title).num_columns(2).show(ui, rows);
        });
}

/// One labelled number.
fn row<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut N,
    speed: f64,
    hint: &str,
) {
    ui.label(label).on_hover_text(hint);
    ui.add(egui::DragValue::new(value).speed(speed).max_decimals(3))
        .on_hover_text(hint);
    ui.end_row();
}

/// Write the live knobs to `ai.ron`.
fn save(world: &mut World) -> Result<String, String> {
    let tuning = *world.resource::<SteeringTuning>();

    let path = bft_assets::asset_directory().join(AI_TUNING);
    let existing =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut file: AiOverrides =
        ron::de::from_str(&existing).map_err(|e| format!("{}: {e}", path.display()))?;

    file.record_steering(&tuning);

    let text = file.to_ron_file(&existing).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;

    // Keep the loaded asset in step with the file, so a revert, and the
    // per-kind layer `restyle_brains` reads, see what was just saved.
    let handle = world
        .get_resource::<AiTuningHandle>()
        .map(|handle| handle.overrides.clone());
    if let Some(handle) = handle
        && let Some(mut asset) = world.resource_mut::<Assets<AiOverrides>>().get_mut(&handle)
    {
        *asset = file;
    }

    Ok(format!("saved to {}", path.display()))
}

/// Back to the file as last loaded or saved.
fn revert(world: &mut World) -> Result<(), String> {
    let handle = world
        .get_resource::<AiTuningHandle>()
        .ok_or("ai.ron was never requested")?
        .overrides
        .clone();
    let tuning = world
        .resource::<Assets<AiOverrides>>()
        .get(&handle)
        .ok_or("ai.ron has not loaded")?
        .steering();

    *world.resource_mut::<SteeringTuning>() = tuning;
    Ok(())
}
