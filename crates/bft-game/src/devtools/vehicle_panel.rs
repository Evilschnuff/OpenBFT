//! Live tuning of the player vehicle's handling.
//!
//! Edits the resolved [`VehicleTable`] in place. `drive` reads the table every
//! fixed tick, so speed, acceleration, steering, grip and suspension change
//! the moment a number does. The few values baked into the hull at spawn —
//! mass, angular damping, the turret rate — are written onto the player's
//! components as well, so they take effect live too.
//!
//! **Save** writes the player's overrides to `assets/tuning/vehicle.ron`:
//! every edited field that differs from the game's own value (the table built
//! with no overrides) is written, every one that matches is cleared, and the
//! file's comment header and anything the panel does not edit are kept.
//! **Revert** rebuilds the table from the file as last loaded or saved.

use avian3d::prelude::*;
use bevy::prelude::*;
use bft_assets::reader::ReaderData;
use bft_sim::vehicle::tuning::{DEFAULT_MASS, MASS_UNIT_KG};
use bft_sim::vehicle::{
    A_DAMPING_DIVISOR, Difficulty, ModeTuning, PlayerVehicle, Traction, TurretRate, Vehicle,
    VehicleForm, VehicleKindId, VehicleOverrides, VehicleTable, build_vehicle_table,
};

use super::{DevPanel, egui};
use crate::play::{TuningHandles, VEHICLE_TUNING};

/// The player's handling, live.
#[derive(Default)]
pub struct VehiclePanel {
    /// What the last save or revert did, shown under the buttons.
    status: Option<String>,
}

/// What the readout shows about the player right now.
struct Readout {
    entity: Entity,
    kind: VehicleKindId,
    form: VehicleForm,
    speed: f32,
    yaw_rate: f32,
    traction: Traction,
}

impl DevPanel for VehiclePanel {
    fn title(&self) -> &str {
        "Vehicle"
    }

    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let Some(now) = readout(world) else {
            ui.label("No player vehicle. Start a level to tune it.");
            return;
        };

        ui.label(format!(
            "{:?}   {:.1} m/s ({:.0} km/h)   yaw {:.2} rad/s   {}",
            now.form,
            now.speed,
            now.speed * 3.6,
            now.yaw_rate,
            if now.traction.is_skidding() {
                format!("SKIDDING, slip {:.1} m/s", now.traction.slip_speed)
            } else {
                "gripping".to_string()
            },
        ));
        ui.separator();

        let Some(table) = world.get_resource::<VehicleTable>() else {
            ui.label("No vehicle table.");
            return;
        };
        let Some(def) = table.kind(&now.kind) else {
            return;
        };
        let Some(mut mode) = def.mode(now.form).cloned() else {
            return;
        };
        let mut turret = def
            .turret_rate
            .map(|[yaw, pitch]| TurretRate { yaw, pitch })
            .unwrap_or_default();
        let (mode_before, turret_before) = (mode.clone(), turret);

        egui::ScrollArea::vertical()
            .id_salt("vehicle")
            .auto_shrink([false, true])
            .show(ui, |ui| edit(ui, &mut mode, &mut turret));

        // Only on a real change, so nothing watching the table is told it
        // changed every frame the panel is open.
        let body_changed = mode.mass != mode_before.mass || mode.a_damping != mode_before.a_damping;
        if mode != mode_before || turret != turret_before {
            let mut table = world.resource_mut::<VehicleTable>();
            if let Some(def) = table.kinds.get_mut(&now.kind) {
                def.modes.insert(now.form, mode.clone());
                def.turret_rate = Some([turret.yaw, turret.pitch]);
            }
            let mut player = world.entity_mut(now.entity);
            player.insert(turret);
            if body_changed {
                player.insert((Mass(mass_kg(&mode)), AngularDamping(angular_damping(&mode))));
            }
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button("Save to vehicle.ron")
                .on_hover_text("Write every form's changes for this vehicle to the tuning file.")
                .clicked()
            {
                self.status = Some(match save(world, &now.kind) {
                    Ok(message) => message,
                    Err(message) => format!("not saved: {message}"),
                });
            }
            if ui
                .button("Revert")
                .on_hover_text(
                    "Rebuild the table from vehicle.ron as last loaded or saved. Mass, \
                     damping and turret rate stay as they are until the next spawn.",
                )
                .clicked()
                && let Some(mut handles) = world.get_resource_mut::<TuningHandles>()
            {
                // `build_table` rebuilds on the next frame.
                handles.built_for = None;
                self.status = Some("reverted to the file".to_string());
            }
        });
        if let Some(status) = &self.status {
            ui.label(status);
        }
    }
}

/// The player's state, or `None` when there is no player.
fn readout(world: &mut World) -> Option<Readout> {
    world
        .query_filtered::<(
            Entity,
            &Vehicle,
            &LinearVelocity,
            &AngularVelocity,
            &GlobalTransform,
            &Traction,
        ), With<PlayerVehicle>>()
        .iter(world)
        .next()
        .map(|(entity, vehicle, linear, angular, transform, traction)| {
            let rotation = transform.rotation();
            Readout {
                entity,
                kind: vehicle.kind.clone(),
                form: vehicle.form,
                speed: linear.dot(rotation * Vec3::NEG_Z),
                yaw_rate: angular.dot(rotation * Vec3::Y),
                traction: *traction,
            }
        })
}

/// The hull's mass in kilograms, as `spawn_player_vehicle` computes it.
fn mass_kg(mode: &ModeTuning) -> f32 {
    mode.mass.unwrap_or(DEFAULT_MASS) * MASS_UNIT_KG
}

/// The hull's angular damping, as `spawn_player_vehicle` computes it.
fn angular_damping(mode: &ModeTuning) -> f32 {
    mode.a_damping.unwrap_or(40.0) / A_DAMPING_DIVISOR
}

/// The editable fields, grouped the way the controller uses them.
fn edit(ui: &mut egui::Ui, mode: &mut ModeTuning, turret: &mut TurretRate) {
    let accel = mode.engine_force / mass_kg(mode);
    let breakaway = mode.static_grip * mode.grip_force_scale / mode.turns[1].max(f32::EPSILON);

    section(ui, "Speed", |ui| {
        row(
            ui,
            "max_speed",
            &mut mode.max_speed,
            0.5,
            "Top forward speed, m/s.",
        );
        row(
            ui,
            "reverse_speed",
            &mut mode.reverse_speed,
            0.5,
            "Top reverse speed, m/s.",
        );
        row(
            ui,
            "engine_force",
            &mut mode.engine_force,
            100.0,
            "Constant drive force up to the speed cap, N. Also drives reverse.",
        );
        ui.label("");
        ui.label(format!(
            "= {accel:.1} m/s², {:.1} s to top speed",
            mode.max_speed / accel.max(f32::EPSILON)
        ));
        ui.end_row();
        row(
            ui,
            "rate_damping[0]",
            &mut mode.rate_damping[0],
            0.05,
            "Drag per second with no throttle held: how quickly it coasts down.",
        );
    });

    section(ui, "Turning", |ui| {
        row(
            ui,
            "turns[0]",
            &mut mode.turns[0],
            0.05,
            "Turn rate standing still, rad/s.",
        );
        row(
            ui,
            "turns[1]",
            &mut mode.turns[1],
            0.05,
            "Turn rate moving, rad/s.",
        );
        row(
            ui,
            "stationary_speed",
            &mut mode.stationary_speed,
            0.1,
            "Below this speed, m/s, turns[0] applies instead of turns[1].",
        );
        row(
            ui,
            "turn_damping",
            &mut mode.turn_damping,
            0.5,
            "How fast the yaw rate closes on the target: higher is snappier.",
        );
    });

    section(ui, "Grip", |ui| {
        row(
            ui,
            "static_grip",
            &mut mode.static_grip,
            0.1,
            "Grip before breaking loose (friction[0] / 10).",
        );
        row(
            ui,
            "lateral_grip",
            &mut mode.lateral_grip,
            0.1,
            "Grip while sliding (friction[1] / 10).",
        );
        row(
            ui,
            "grip_force_scale",
            &mut mode.grip_force_scale,
            0.1,
            "Scales both grips into a force. The main knob for when skids start.",
        );
        row(
            ui,
            "regrip_fraction",
            &mut mode.regrip_fraction,
            0.01,
            "A slide ends once it decays to this fraction of the speed that started it.",
        );
        ui.label("");
        ui.label(format!("= full-rate turns skid above {breakaway:.1} m/s"));
        ui.end_row();
        row(
            ui,
            "air_control",
            &mut mode.air_control,
            0.01,
            "Authority off the ground, 0..1.",
        );
    });

    section(ui, "Upright", |ui| {
        row(
            ui,
            "upright_rate_air",
            &mut mode.upright_rate_air,
            0.05,
            "How fast the hull levels in the air, rad/s.",
        );
        row(
            ui,
            "upright_rate_ground",
            &mut mode.upright_rate_ground,
            0.05,
            "How fast it follows the ground's normal, rad/s.",
        );
        row(
            ui,
            "max_tilt",
            &mut mode.max_tilt,
            0.01,
            "The furthest it may lean from world up, radians.",
        );
    });

    section(ui, "Suspension", |ui| {
        let suspension = &mut mode.suspension;
        row(
            ui,
            "rest",
            &mut suspension.rest,
            0.05,
            "Spring rest length, m.",
        );
        row(
            ui,
            "stiffness",
            &mut suspension.stiffness,
            100.0,
            "N/m per ray.",
        );
        row(
            ui,
            "damping",
            &mut suspension.damping,
            50.0,
            "Ns/m per ray.",
        );
        row(
            ui,
            "max_len",
            &mut suspension.max_len,
            0.05,
            "Ray reach, m.",
        );
    });

    section(ui, "Body", |ui| {
        let mut mass = mode.mass.unwrap_or(DEFAULT_MASS);
        if row(ui, "mass", &mut mass, 0.05, "Reader tonnes.") {
            mode.mass = Some(mass);
        }
        let mut a_damping = mode.a_damping.unwrap_or(40.0);
        if row(
            ui,
            "a_damping",
            &mut a_damping,
            0.5,
            "Angular damping, reader units (divided by 10).",
        ) {
            mode.a_damping = Some(a_damping);
        }
    });

    section(ui, "Turret (all forms)", |ui| {
        row(ui, "traverse", &mut turret.yaw, 0.05, "rad/s.");
        row(ui, "elevation", &mut turret.pitch, 0.05, "rad/s.");
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

/// One labelled number. Returns whether it changed.
fn row(ui: &mut egui::Ui, label: &str, value: &mut f32, speed: f64, hint: &str) -> bool {
    ui.label(label).on_hover_text(hint);
    let changed = ui
        .add(egui::DragValue::new(value).speed(speed).max_decimals(3))
        .on_hover_text(hint)
        .changed();
    ui.end_row();
    changed
}

/// Write `kind`'s live tuning, every form, to `vehicle.ron`.
fn save(world: &mut World, kind: &VehicleKindId) -> Result<String, String> {
    let handles = world
        .get_resource::<TuningHandles>()
        .ok_or("no tuning handles")?;
    let reader = handles
        .reader
        .as_ref()
        .and_then(|handle| world.resource::<Assets<ReaderData>>().get(handle))
        .ok_or("the reader archive is not loaded")?;
    let pristine = build_vehicle_table(
        reader,
        *world.resource::<Difficulty>(),
        &VehicleOverrides::default(),
    )
    .map_err(|e| e.to_string())?;
    let live = world.resource::<VehicleTable>();
    let (Some(now), Some(was)) = (live.kind(kind), pristine.kind(kind)) else {
        return Err(format!("no `{}` in the table", kind.as_str()));
    };

    let path = bft_assets::asset_directory().join(VEHICLE_TUNING);
    let existing =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut file: VehicleOverrides =
        ron::de::from_str(&existing).map_err(|e| format!("{}: {e}", path.display()))?;

    for (form, mode) in &now.modes {
        if let Some(original) = was.mode(*form) {
            file.record_mode(kind.as_str(), *form, mode, original);
        }
    }
    file.record_turret_rate(kind.as_str(), now, was);

    let text = file.to_ron_file(&existing).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;

    // Keep the loaded asset in step with the file, so a rebuild (a revert, a
    // difficulty change) keeps these values rather than the ones loaded at
    // startup.
    let handle = handles.vehicle.clone();
    if let Some(mut asset) = world
        .resource_mut::<Assets<VehicleOverrides>>()
        .get_mut(&handle)
    {
        *asset = file;
    }

    Ok(format!("saved to {}", path.display()))
}
