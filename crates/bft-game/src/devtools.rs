//! Developer tools: the egui inspector, the gizmo overlays and `inspect`
//! (FR-T1–T3).
//!
//! [`DevToolsPlugin`] is **not** part of [`GamePlugin`](crate::GamePlugin):
//! `bft-bin` adds it for `--dev` and for every debug build. Without it the
//! game runs without them: `F1` shows the viewer's
//! four-line panel and, in play, toggles the vehicle gizmos. With it, `F1`
//! opens the inspector instead, which carries both of those as a readout and
//! a checkbox.
//!
//! # Adding a panel
//!
//! A module with state worth looking at implements [`DevPanel`] and pushes
//! it onto [`DevPanels`] from its own plugin, guarded so it costs nothing
//! when the dev tools are off:
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bft_game::devtools::{DevPanel, DevPanels, egui};
//!
//! #[derive(Default)]
//! struct NetPanel;
//!
//! impl DevPanel for NetPanel {
//!     fn title(&self) -> &str {
//!         "Net"
//!     }
//!
//!     fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
//!         let entities = world.entities().len();
//!         ui.label(format!("{entities} entities"));
//!     }
//! }
//!
//! struct NetPlugin;
//!
//! impl Plugin for NetPlugin {
//!     fn build(&self, app: &mut App) {
//!         // `DevPanels` exists only when `DevToolsPlugin` was added, and
//!         // plugins are built in the order they were added, so look in
//!         // `finish`, which runs after every `build`.
//!     }
//!
//!     fn finish(&self, app: &mut App) {
//!         if let Some(mut panels) = app.world_mut().get_resource_mut::<DevPanels>() {
//!             panels.0.push(Box::new(NetPanel));
//!         }
//!     }
//! }
//! ```
//!
//! The panel gets the whole `World`, so it reads whatever it likes with
//! `world.query` and `world.get_resource`, and may write too — the level
//! panel sets [`SelectedNode`] that way. [`sim_panels`] holds the AI, anim
//! and mission panels. There is no net panel, as there is no multiplayer yet,
//! and [`DebugFlags::net`] draws nothing.

use std::time::{Duration, Instant};

use bevy::diagnostic::{
    DiagnosticsStore, EntityCountDiagnosticsPlugin, FrameTimeDiagnosticsPlugin,
};
use bevy::pbr::wireframe::WireframePlugin;
use bevy::prelude::*;
use bevy_egui::{EguiContext, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext};
use bft_assets::axes::AxisConvention;
use bft_assets::catalog::LevelCatalog;
use bft_sim::level::NodeIndex;
use bft_sim::schedule::SimSet;
use bft_sim::states::AppState;

use crate::level_view::LoadedLevel;
use crate::ui::{Action, ActionInput};

pub mod ai_tuning_panel;
pub mod free_camera;
pub mod inspect;
pub mod overlays;
pub mod panels;
pub mod sim_panels;
pub mod sound_panel;
pub mod vehicle_panel;

pub use bevy_egui::egui;
pub use bft_sim::physics::DebugFlags;

/// The dev tools: inspector, overlays, frame diagnostics.
///
/// Added only with `--dev` or in a debug build; see the module docs.
pub struct DevToolsPlugin;

/// Whether the inspector window is up. Toggled by `Action::DebugPanel` (F1).
///
/// Its presence is also how the rest of the crate tells that the dev tools
/// are on: the viewer's own `F1` panel and the HUD's `F1` vehicle toggle
/// stand down when it exists.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct DevPanelOpen(pub bool);

/// The node the inspector has selected, which the overlays highlight.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct SelectedNode(pub Option<Entity>);

/// One tab of the inspector.
pub trait DevPanel: Send + Sync {
    /// The tab's label.
    fn title(&self) -> &str;
    /// Draw the tab's contents. Runs only while the tab is showing.
    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World);
}

/// Every registered tab, in the order they were added.
#[derive(Resource, Default)]
pub struct DevPanels(pub Vec<Box<dyn DevPanel>>);

/// Which tab is showing.
#[derive(Resource, Debug, Clone, Copy, Default)]
struct ActiveTab(usize);

/// How long each [`SimSet`] of the fixed tick takes, smoothed.
///
/// Measured by wall clock between marker systems ordered on the set
/// boundaries, so a set's figure is "from the end of the previous set to the
/// end of this one": anything unordered that the executor happens to run in
/// that gap counts too. Good enough to see which phase a spike is in.
#[derive(Resource, Debug, Clone, Default)]
pub struct SimTimings {
    /// One per [`SIM_SETS`] entry.
    pub sets: [Duration; SIM_SETS.len()],
    /// The whole chain.
    pub tick: Duration,
    stamps: [Option<Instant>; SIM_SETS.len() + 1],
}

/// The sets of the fixed tick in chain order, with their names.
pub const SIM_SETS: [(&str, SimSet); 9] = [
    ("Input", SimSet::Input),
    ("Vehicle", SimSet::Vehicle),
    ("Weapons", SimSet::Weapons),
    ("Physics", SimSet::Physics),
    ("Damage", SimSet::Damage),
    ("Ai", SimSet::Ai),
    ("Anim", SimSet::Anim),
    ("Mission", SimSet::Mission),
    ("Replicate", SimSet::Replicate),
];

/// How much of each new measurement goes into the smoothed figure.
const TIMING_SMOOTHING: f32 = 0.1;

impl SimTimings {
    /// Record the time at boundary `index` (0 is before the first set).
    fn stamp(&mut self, index: usize) {
        if index == 0 {
            self.stamps = Default::default();
        }
        self.stamps[index] = Some(Instant::now());

        if index == SIM_SETS.len() {
            self.finish();
        }
    }

    /// Fold a completed tick into the smoothed figures.
    fn finish(&mut self) {
        let mut tick = Duration::ZERO;

        for (i, set) in self.sets.iter_mut().enumerate() {
            // A tick the inspector opened halfway through has gaps; skip it.
            let (Some(start), Some(end)) = (self.stamps[i], self.stamps[i + 1]) else {
                return;
            };
            let measured = end.saturating_duration_since(start);
            *set = set.mul_f32(1.0 - TIMING_SMOOTHING) + measured.mul_f32(TIMING_SMOOTHING);
            tick += *set;
        }

        self.tick = tick;
    }
}

/// A marker system for boundary `I`.
fn stamp<const I: usize>(mut timings: ResMut<SimTimings>) {
    timings.stamp(I);
}

fn inspector_open(open: Res<DevPanelOpen>) -> bool {
    open.0
}

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(FrameTimeDiagnosticsPlugin::default());
        }
        if !app.is_plugin_added::<EntityCountDiagnosticsPlugin>() {
            app.add_plugins(EntityCountDiagnosticsPlugin::default());
        }
        // Wireframes need a GPU feature Metal, Vulkan and DX12 all have;
        // without it the plugin logs a warning and the checkbox does nothing.
        if !app.is_plugin_added::<WireframePlugin>() {
            app.add_plugins(WireframePlugin::default());
        }

        app.init_resource::<DebugFlags>()
            .init_resource::<DevPanelOpen>()
            .init_resource::<SelectedNode>()
            .init_resource::<ActiveTab>()
            .init_resource::<SimTimings>()
            .insert_resource(DevPanels(vec![
                Box::new(panels::LevelPanel::default()),
                Box::new(panels::ModelsPanel::default()),
                Box::new(sim_panels::AiPanel),
                Box::new(sim_panels::AnimPanel::default()),
                Box::new(sim_panels::MissionPanel),
                Box::new(vehicle_panel::VehiclePanel::default()),
                Box::new(ai_tuning_panel::AiTuningPanel::default()),
                Box::new(sound_panel::SoundPanel::default()),
            ]))
            .add_systems(Update, toggle_inspector)
            .add_systems(OnExit(AppState::Viewer), clear_selection)
            .add_systems(OnExit(AppState::InGame), clear_selection)
            .add_systems(
                EguiPrimaryContextPass,
                (
                    draw_inspector.run_if(inspector_open),
                    draw_fps.run_if(|flags: Res<DebugFlags>, open: Res<DevPanelOpen>| {
                        flags.fps && !open.0
                    }),
                ),
            )
            .add_systems(
                FixedUpdate,
                (
                    stamp::<0>.before(SimSet::Input),
                    stamp::<1>.after(SimSet::Input).before(SimSet::Vehicle),
                    stamp::<2>.after(SimSet::Vehicle).before(SimSet::Weapons),
                    stamp::<3>.after(SimSet::Weapons).before(SimSet::Physics),
                    stamp::<4>.after(SimSet::Physics).before(SimSet::Damage),
                    stamp::<5>.after(SimSet::Damage).before(SimSet::Ai),
                    stamp::<6>.after(SimSet::Ai).before(SimSet::Anim),
                    stamp::<7>.after(SimSet::Anim).before(SimSet::Mission),
                    stamp::<8>.after(SimSet::Mission).before(SimSet::Replicate),
                    stamp::<9>.after(SimSet::Replicate),
                )
                    .run_if(inspector_open),
            )
            .add_plugins((overlays::OverlaysPlugin, free_camera::FreeCameraPlugin));
    }
}

/// `F1` opens and closes the inspector.
fn toggle_inspector(input: ActionInput, mut open: ResMut<DevPanelOpen>) {
    if input.just_pressed(Action::DebugPanel) {
        open.0 = !open.0;
    }
}

/// A selection does not survive its level.
fn clear_selection(mut selected: ResMut<SelectedNode>) {
    selected.0 = None;
}

/// The primary window's egui context, cloned out of the world.
///
/// `egui::Context` is a reference-counted handle, so the clone is the same
/// context; taking it out is what lets a panel have `&mut World` while it
/// draws.
fn primary_context(world: &mut World) -> Option<egui::Context> {
    let mut contexts = world.query_filtered::<&mut EguiContext, With<PrimaryEguiContext>>();
    let mut context = contexts.single_mut(world).ok()?;
    Some(context.get_mut().clone())
}

/// The inspector window: the header, the flags, then the active tab.
fn draw_inspector(world: &mut World) {
    let Some(ctx) = primary_context(world) else {
        return;
    };

    world.resource_scope(|world, mut panels: Mut<DevPanels>| {
        egui::Window::new("Dev tools")
            .default_pos([8.0, 8.0])
            .default_width(420.0)
            .default_height(640.0)
            .show(&ctx, |ui| {
                header(ui, world);

                egui::CollapsingHeader::new("Overlays")
                    .default_open(true)
                    .show(ui, |ui| flags_ui(ui, world));

                egui::CollapsingHeader::new("Sim tick")
                    .default_open(false)
                    .show(ui, |ui| timings_ui(ui, world));

                ui.separator();

                let mut tab = world.resource::<ActiveTab>().0;
                ui.horizontal_wrapped(|ui| {
                    for (i, panel) in panels.0.iter().enumerate() {
                        if ui.selectable_label(tab == i, panel.title()).clicked() {
                            tab = i;
                        }
                    }
                });
                world.resource_mut::<ActiveTab>().0 = tab;
                ui.separator();

                if let Some(panel) = panels.0.get_mut(tab) {
                    panel.ui(ui, world);
                }
            });
    });
}

/// FPS, entity count, the level, the camera and the axes.
fn header(ui: &mut egui::Ui, world: &mut World) {
    let (fps, frame_ms, entities) = world
        .get_resource::<DiagnosticsStore>()
        .map(|store| {
            let smoothed = |path| {
                store
                    .get(path)
                    .and_then(|diagnostic| diagnostic.smoothed())
                    .unwrap_or(0.0)
            };
            (
                smoothed(&FrameTimeDiagnosticsPlugin::FPS),
                smoothed(&FrameTimeDiagnosticsPlugin::FRAME_TIME),
                smoothed(&EntityCountDiagnosticsPlugin::ENTITY_COUNT),
            )
        })
        .unwrap_or_default();

    ui.label(format!(
        "{fps:.0} fps   {frame_ms:.1} ms   {entities:.0} entities"
    ));

    let state = world
        .get_resource::<State<AppState>>()
        .map(|state| format!("{:?}", state.get()))
        .unwrap_or_default();
    let level = world
        .get_resource::<LoadedLevel>()
        .map(|loaded| {
            world
                .get_resource::<LevelCatalog>()
                .and_then(|catalog| catalog.level(loaded.id))
                .map(|entry| entry.name.clone())
                .unwrap_or_else(|| loaded.id.to_string())
        })
        .unwrap_or_else(|| "no level".to_string());
    let nodes = world.get_resource::<NodeIndex>().map_or(0, NodeIndex::len);
    ui.label(format!("{state}   {level}   {nodes} nodes"));

    let mut cameras =
        world.query_filtered::<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>();
    if let Ok(camera) = cameras.single(world) {
        let position = camera.translation();
        ui.label(format!(
            "camera {:.1}, {:.1}, {:.1}",
            position.x, position.y, position.z
        ));
    }

    let axes = world
        .get_resource::<AxisConvention>()
        .copied()
        .unwrap_or_default();
    ui.label(format!("axes {}", crate::viewer::axis_readout(axes)));
}

/// One checkbox per [`DebugFlags`] field.
fn flags_ui(ui: &mut egui::Ui, world: &mut World) {
    let mut flags = *world.resource::<DebugFlags>();

    egui::Grid::new("debug_flags")
        .num_columns(3)
        .show(ui, |ui| {
            ui.checkbox(&mut flags.physics, "colliders");
            ui.checkbox(&mut flags.water, "water");
            ui.checkbox(&mut flags.vehicle, "vehicle");
            ui.end_row();
            ui.checkbox(&mut flags.node_bboxes, "node boxes");
            ui.checkbox(&mut flags.partitions, "partitions");
            ui.checkbox(&mut flags.spawns, "spawns");
            ui.end_row();
            ui.checkbox(&mut flags.ai, "AI");
            ui.checkbox(&mut flags.anim, "anim");
            ui.checkbox(&mut flags.fps, "FPS when closed");
            ui.end_row();
            ui.add_enabled(false, egui::Checkbox::new(&mut flags.net, "net"))
                .on_disabled_hover_text("Nothing to show until multiplayer lands.");
            ui.end_row();
        });

    // Only on a real change, so the overlays' `Res<DebugFlags>` is not
    // marked changed every frame the inspector is open.
    if flags != *world.resource::<DebugFlags>() {
        *world.resource_mut::<DebugFlags>() = flags;
    }
}

/// The per-set tick times.
fn timings_ui(ui: &mut egui::Ui, world: &mut World) {
    let timings = world.resource::<SimTimings>().clone();

    egui::Grid::new("sim_timings")
        .num_columns(2)
        .show(ui, |ui| {
            for ((name, _), time) in SIM_SETS.iter().zip(timings.sets) {
                ui.label(*name);
                ui.label(format!("{:.3} ms", time.as_secs_f64() * 1000.0));
                ui.end_row();
            }
            ui.strong("tick");
            ui.strong(format!("{:.3} ms", timings.tick.as_secs_f64() * 1000.0));
            ui.end_row();
        });
}

/// A frame-rate readout in the corner, for [`DebugFlags::fps`].
fn draw_fps(world: &mut World) {
    let Some(ctx) = primary_context(world) else {
        return;
    };
    let fps = world
        .get_resource::<DiagnosticsStore>()
        .and_then(|store| store.get(&FrameTimeDiagnosticsPlugin::FPS))
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or(0.0);

    egui::Area::new(egui::Id::new("dev_fps"))
        .anchor(egui::Align2::RIGHT_TOP, [-8.0, 8.0])
        .interactable(false)
        .show(&ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("{fps:.0} fps"))
                    .monospace()
                    .color(egui::Color32::WHITE)
                    .background_color(egui::Color32::from_black_alpha(160)),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tick's figures are the gaps between consecutive boundaries.
    #[test]
    fn timings_measure_the_gaps_between_boundaries() {
        let mut timings = SimTimings::default();
        let start = Instant::now();

        for i in 0..=SIM_SETS.len() {
            timings.stamps[i] = Some(start + Duration::from_millis(i as u64 * 10));
        }
        timings.finish();

        for set in timings.sets {
            // One tenth of 10 ms, from zero.
            assert_eq!(set, Duration::from_millis(1));
        }
        assert_eq!(timings.tick, Duration::from_millis(9));
    }

    /// A tick the inspector opened partway through is not counted.
    #[test]
    fn a_partial_tick_is_ignored() {
        let mut timings = SimTimings::default();
        timings.stamp(4);
        timings.stamp(SIM_SETS.len());

        assert!(timings.sets.iter().all(|set| set.is_zero()));
    }

    /// The flags start off, so a dev build draws nothing until asked.
    #[test]
    fn every_overlay_starts_off() {
        assert_eq!(
            DebugFlags::default(),
            DebugFlags {
                physics: false,
                water: false,
                vehicle: false,
                node_bboxes: false,
                partitions: false,
                ai: false,
                spawns: false,
                anim: false,
                net: false,
                fps: false,
            }
        );
    }
}
