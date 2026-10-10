//! Tabs for the simulation: AI, the anim VM and the mission.
//!
//! Each is also an example of a [`DevPanel`](super::DevPanel). There is no
//! net panel: without multiplayer it has nothing to read.

use bevy::prelude::*;
use bft_sim::ai::{AiBrain, TurretAi};
use bft_sim::anim::{AnimInstance, AnimProgram};
use bft_sim::level::NodeName;
use bft_sim::mission::{MissionScript, MissionState};

use super::{DevPanel, SelectedNode, egui};

/// How many rows a list shows before it stops. A level has a few hundred
/// anim instances; the running ones are what matter.
const MAX_ROWS: usize = 300;

/// A name for an entity: its node name, its `Name`, or the id.
fn name_of(world: &World, entity: Entity) -> String {
    world
        .get::<NodeName>(entity)
        .map(|name| name.0.clone())
        .or_else(|| world.get::<Name>(entity).map(|name| name.to_string()))
        .unwrap_or_else(|| format!("{entity}"))
}

/// Every enemy brain and turret: state, target, and where it is going.
pub struct AiPanel;

impl DevPanel for AiPanel {
    fn title(&self) -> &str {
        "AI"
    }

    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let mut clicked = None;

        let brains: Vec<(Entity, AiBrain)> = world
            .query::<(Entity, &AiBrain)>()
            .iter(world)
            .map(|(entity, brain)| (entity, brain.clone()))
            .collect();
        let turrets: Vec<(Entity, TurretAi)> = world
            .query::<(Entity, &TurretAi)>()
            .iter(world)
            .map(|(entity, turret)| (entity, turret.clone()))
            .collect();

        ui.label(format!(
            "{} vehicles, {} turrets",
            brains.len(),
            turrets.len()
        ));

        egui::ScrollArea::vertical()
            .id_salt("ai")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::CollapsingHeader::new("Vehicles")
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new("ai_brains").striped(true).show(ui, |ui| {
                            for (entity, brain) in brains.iter().take(MAX_ROWS) {
                                if ui.link(name_of(world, *entity)).clicked() {
                                    clicked = Some(*entity);
                                }
                                ui.label(format!("{:?}", brain.state));
                                ui.label(format!("{:.1} s", brain.timer));
                                ui.label(
                                    brain
                                        .target
                                        .map_or("-".into(), |target| name_of(world, target)),
                                );
                                ui.label(format!(
                                    "net {}  wp {}",
                                    brain.net.map_or("-".into(), |net| net.to_string()),
                                    brain.waypoint.map_or("-".into(), |wp| wp.to_string())
                                ));
                                ui.end_row();
                            }
                        });
                    });

                egui::CollapsingHeader::new("Turrets")
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new("ai_turrets").striped(true).show(ui, |ui| {
                            for (entity, turret) in turrets.iter().take(MAX_ROWS) {
                                if ui.link(name_of(world, *entity)).clicked() {
                                    clicked = Some(*entity);
                                }
                                ui.label(
                                    turret
                                        .target
                                        .map_or("-".into(), |target| name_of(world, target)),
                                );
                                ui.label(format!(
                                    "yaw {:.0}°  pitch {:.0}°",
                                    turret.aim.x.to_degrees(),
                                    turret.aim.y.to_degrees()
                                ));
                                ui.label(if turret.deactivated { "off" } else { "" });
                                ui.end_row();
                            }
                        });
                    });
            });

        if clicked.is_some() {
            world.resource_mut::<SelectedNode>().0 = clicked;
        }
    }
}

/// The anim VM's instances: which are armed, running, done.
#[derive(Default)]
pub struct AnimPanel {
    running_only: bool,
    filter: String,
}

impl DevPanel for AnimPanel {
    fn title(&self) -> &str {
        "Anim"
    }

    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let Some(program) = world.get_resource::<AnimProgram>().cloned() else {
            ui.label("No anim program loaded.");
            return;
        };

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.running_only, "running only");
            ui.label("Find");
            ui.text_edit_singleline(&mut self.filter);
        });

        let needle = self.filter.trim().to_lowercase();
        let mut rows: Vec<(String, String, AnimInstance)> = world
            .query::<&AnimInstance>()
            .iter(world)
            .filter(|instance| !self.running_only || !instance.running.is_empty())
            .filter_map(|instance| {
                let def = program.0.defs.get(instance.def)?;
                let matches = needle.is_empty()
                    || def.anim_name.to_lowercase().contains(&needle)
                    || def.name.to_lowercase().contains(&needle);
                matches.then(|| (def.anim_name.clone(), def.name.clone(), instance.clone()))
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));

        let running = rows.iter().filter(|row| !row.2.running.is_empty()).count();
        ui.label(format!(
            "{} definitions, {} listed, {running} running",
            program.0.defs.len(),
            rows.len()
        ));

        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("anim")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("anim_instances")
                    .striped(true)
                    .show(ui, |ui| {
                        for (anim_name, root_name, instance) in rows.iter().take(MAX_ROWS) {
                            let label = format!("{anim_name}  ({root_name})");
                            match instance.root {
                                Some(root) => {
                                    if ui.link(label).clicked() {
                                        clicked = Some(root);
                                    }
                                }
                                None => {
                                    ui.label(label);
                                }
                            }
                            let mut state = Vec::new();
                            if instance.armed {
                                state.push("armed");
                            }
                            if instance.started {
                                state.push("started");
                            }
                            if instance.completed {
                                state.push("done");
                            }
                            if instance.invalidated {
                                state.push("invalid");
                            }
                            if instance.copy.is_some() {
                                state.push("copy");
                            }
                            ui.label(state.join(" "));
                            ui.label(if instance.running.is_empty() {
                                String::new()
                            } else {
                                format!("{} seq", instance.running.len())
                            });
                            ui.end_row();
                        }
                    });
                if rows.len() > MAX_ROWS {
                    ui.label(format!("… and {} more", rows.len() - MAX_ROWS));
                }
            });

        if clicked.is_some() {
            world.resource_mut::<SelectedNode>().0 = clicked;
        }
    }
}

/// The mission: objectives and how it is going.
pub struct MissionPanel;

impl DevPanel for MissionPanel {
    fn title(&self) -> &str {
        "Mission"
    }

    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let Some(state) = world.get_resource::<MissionState>() else {
            ui.label("No mission running.");
            return;
        };
        let script = world.get_resource::<MissionScript>();

        ui.label(format!(
            "{:?}   started {}   dying {}   outcome {}",
            state.mission,
            state.started,
            state.dying,
            state
                .outcome
                .map_or("-".to_string(), |outcome| format!("{outcome:?}"))
        ));
        ui.label(format!("{} queued message(s)", state.messages.len()));

        egui::Grid::new("objectives").striped(true).show(ui, |ui| {
            for (i, objective) in state.objectives.iter().enumerate() {
                let def = script.and_then(|script| script.objectives.get(i));
                ui.label(i.to_string());
                ui.label(format!("{objective:?}"));
                ui.label(def.map_or("", |def| def.summary_key.as_str()));
                ui.label(def.map_or(String::new(), |def| format!("{:?}", def.completion)));
                ui.end_row();
            }
        });
    }
}
