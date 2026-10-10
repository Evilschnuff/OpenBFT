//! The in-game HUD (FR-U1, FR-A4, FR-M3): health, weapons, reticle,
//! messages and the objective list. The automap is [`super::automap`].
//!
//! # Two HUDs
//!
//! [`HudStyle`] picks one. **Original** is the game's own art and layout,
//! read from the install ([`original`]). **Fallback** is drawn from our own
//! widgets in this file, and is what shows without
//! an install, when any of the original's textures is missing, or when the
//! player turns the original off in the settings. The markers below are
//! shared, so the dev tools' toggles and the automap work with either.
//!
//! Everything reads the local player's components — [`Health`],
//! [`WeaponLoadout`], [`AimResult`], [`Vehicle`] — and [`MissionState`], and
//! writes nothing back except draining the mission's message queue, which
//! the sim leaves for the HUD to consume.
//!
//! # The reticle is the cursor
//!
//! Aiming is a free cursor ([`crate::aim`]) and the turret slews to
//! where it points, so the reticle *is* the cursor ([`super::cursor`]): the
//! original's `ret_*` images under the original HUD, our own crosshair
//! otherwise. The turret's own aim point is never drawn — the
//! original drew none, since the two are meant to coincide.

use std::collections::VecDeque;

use bevy::prelude::*;
use bft_assets::messages::Messages;
use bft_sim::damage::Health;
use bft_sim::mission::{MissionScript, MissionState, ObjectiveChanged, ObjectiveState};
use bft_sim::physics::DebugFlags;
use bft_sim::states::AppState;
use bft_sim::vehicle::{PlayerVehicle, Vehicle};
use bft_sim::weapons::{WeaponLoadout, WeaponTable};

pub mod layout;
pub mod original;
pub mod sensor;

use super::bindings::{Action, ActionInput};
use super::gameplay_input_enabled;
use super::theme::{
    ACCENT, BODY_SIZE, CYAN, ERROR, GREEN, SMALL_SIZE, TEXT, TEXT_DIM, UiFonts, message_text,
};
use crate::devtools::DevPanelOpen;
use crate::globals::{self, Globals};
use crate::ui::settings::Settings;

/// The HUD's root; hiding it hides everything but the cursor.
#[derive(Component, Debug, Clone, Copy)]
pub struct HudRoot;

/// The health bar.
#[derive(Component, Debug, Clone, Copy)]
pub struct HudHealth;

/// The weapon slots and the selected weapon's line.
#[derive(Component, Debug, Clone, Copy)]
pub struct HudWeapon;

/// The automap's frame.
#[derive(Component, Debug, Clone, Copy)]
pub struct HudMap;

/// The message log.
#[derive(Component, Debug, Clone, Copy)]
pub struct HudMessages;

/// The sensor window (original HUD).
#[derive(Component, Debug, Clone, Copy)]
pub struct HudSensor;

/// The targeted enemy's health bar (original HUD).
#[derive(Component, Debug, Clone, Copy)]
pub struct HudEnemyHealth;

/// The message tab (original HUD).
#[derive(Component, Debug, Clone, Copy)]
pub struct HudObjectiveTab;

/// The score (original HUD).
#[derive(Component, Debug, Clone, Copy)]
pub struct HudScore;

/// The mission clock (original HUD).
#[derive(Component, Debug, Clone, Copy)]
pub struct HudClock;

/// The four form lights (original HUD).
#[derive(Component, Debug, Clone, Copy)]
pub struct HudForms;

/// Which HUD this install can draw.
///
/// `Pending` until the global reader and `image.zbd` have loaded; then
/// `Original` with the parsed layout and the art, or `Fallback` when either
/// is missing or incomplete. The settings can still ask for the fallback
/// over an available original ([`uses_original`]).
#[derive(Resource, Debug, Default)]
pub enum HudStyle {
    #[default]
    Pending,
    Original(Box<original::OriginalHud>),
    Fallback,
}

/// Whether the original HUD is the one to draw: available and wanted.
pub fn uses_original(style: &HudStyle, settings: &Settings) -> bool {
    matches!(style, HudStyle::Original(_)) && settings.hud.original
}

/// Which HUD a [`HudRoot`] was built as, so a settings change rebuilds it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
struct BuiltAs {
    original: bool,
}

/// Whether the HUD is drawn. The dev tools' toggle; on by default.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudVisible(pub bool);

impl Default for HudVisible {
    fn default() -> HudVisible {
        HudVisible(true)
    }
}

/// How long a message stays up, in seconds, and how much of that is the
/// fade at the end.
///
/// Presentation timing, not tuning: `objectives.zrd`'s `READ_TIME` (8 s in
/// all six missions) is how long the briefing text stays, and a HUD line is
/// shorter than a briefing.
const MESSAGE_LIFETIME: f32 = 6.0;
const MESSAGE_FADE: f32 = 1.5;
/// How many lines the log shows at once.
pub(crate) const MESSAGE_LINES: usize = 3;

/// The log: what was said and when.
#[derive(Resource, Debug, Clone, Default)]
pub struct HudLog {
    /// `(text, time shown)`, oldest first.
    lines: VecDeque<(String, f32)>,
}

impl HudLog {
    /// Add a line at `now`.
    pub fn push(&mut self, text: String, now: f32) {
        self.lines.push_back((text, now));
        while self.lines.len() > MESSAGE_LINES {
            self.lines.pop_front();
        }
    }

    /// The lines still up at `now`, with their opacity.
    pub fn visible(&self, now: f32) -> Vec<(&str, f32)> {
        self.lines
            .iter()
            .filter_map(|(text, shown)| {
                let age = now - shown;
                (age < MESSAGE_LIFETIME).then(|| {
                    let left = MESSAGE_LIFETIME - age;
                    (text.as_str(), (left / MESSAGE_FADE).clamp(0.0, 1.0))
                })
            })
            .collect()
    }
}

/// Whether the objective list is open.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectivesOpen(pub bool);

#[derive(Component, Debug, Clone, Copy)]
struct HealthFill;

#[derive(Component, Debug, Clone, Copy)]
struct HealthValue;

#[derive(Component, Debug, Clone, Copy)]
struct SlotBox(u8);

#[derive(Component, Debug, Clone, Copy)]
struct WeaponLine;

/// One line of the message log, in either HUD.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct MessageLine(pub(crate) usize);

#[derive(Component, Debug, Clone, Copy)]
struct ObjectiveList;

/// The HUD's systems.
pub fn plugin(app: &mut App) {
    app.init_resource::<HudVisible>()
        .init_resource::<HudLog>()
        .init_resource::<ObjectivesOpen>()
        .init_resource::<HudStyle>()
        .init_resource::<original::HudArtLoad>()
        .init_resource::<original::ObjectiveTab>()
        .add_systems(Startup, sensor::create_sensor_image)
        .add_systems(OnExit(AppState::InGame), clear_log)
        .add_systems(Update, original::resolve_style)
        .add_systems(
            Update,
            (sensor::layer_player_meshes, sensor::show_player_layer),
        )
        .add_systems(
            Update,
            (
                (rebuild_on_settings_change, spawn_hud).chain(),
                original::brief_on_mission_start,
                hud_hotkeys.run_if(gameplay_input_enabled),
                collect_messages,
                show_hud,
                update_messages,
                (update_health, update_weapons, update_objectives).run_if(fallback_drawn),
                (
                    original::update_frame,
                    original::relayout,
                    original::rebuild_strips,
                    original::update_filter,
                    original::update_enemy_bar,
                    original::update_objective_tab,
                    original::update_weapons,
                    original::update_health,
                    original::update_forms,
                    original::update_stats,
                    sensor::drive_sensor_camera,
                )
                    .chain()
                    .run_if(original_drawn),
            )
                .run_if(in_state(AppState::InGame)),
        );
}

/// Whether the HUD on screen is the original.
fn original_drawn(roots: Query<&BuiltAs, With<HudRoot>>) -> bool {
    roots.iter().any(|built| built.original)
}

/// Whether the HUD on screen is the fallback.
fn fallback_drawn(roots: Query<&BuiltAs, With<HudRoot>>) -> bool {
    roots.iter().any(|built| !built.original)
}

/// Rebuild the HUD when the settings switch between the two.
fn rebuild_on_settings_change(
    mut commands: Commands,
    settings: Res<Settings>,
    style: Res<HudStyle>,
    roots: Query<(Entity, &BuiltAs), With<HudRoot>>,
) {
    if !settings.is_changed() && !style.is_changed() {
        return;
    }
    let wanted = uses_original(&style, &settings);
    for (root, built) in &roots {
        if built.original != wanted {
            commands.entity(root).despawn();
            // The automap hangs off the root; let it build again.
            commands.remove_resource::<super::automap::Automap>();
        }
    }
}

/// Build the HUD once the style is known: the original when it is
/// available and wanted, the fallback otherwise.
#[allow(clippy::too_many_arguments)]
pub fn spawn_hud(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    style: Res<HudStyle>,
    settings: Res<Settings>,
    sensor_image: Option<Res<sensor::SensorImage>>,
    mut tab: ResMut<original::ObjectiveTab>,
    existing: Query<(), With<HudRoot>>,
) {
    if !existing.is_empty() || matches!(*style, HudStyle::Pending) {
        return;
    }

    if let HudStyle::Original(hud) = &*style
        && settings.hud.original
    {
        let image = sensor_image
            .map(|image| image.0.clone())
            .unwrap_or_default();
        let root = original::spawn(&mut commands, hud, &fonts, image);
        commands.entity(root).insert(BuiltAs { original: true });
        *tab = original::ObjectiveTab {
            pending: tab.pending,
            ..default()
        };
        return;
    }

    spawn_fallback(&mut commands, &fonts);
}

/// The fallback HUD.
fn spawn_fallback(commands: &mut Commands, fonts: &UiFonts) {
    commands
        .spawn((
            HudRoot,
            BuiltAs { original: false },
            Name::new("hud"),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                ..default()
            },
            // The overlay must not eat clicks: the left button fires.
            Pickable::IGNORE,
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|hud| {
            // Top left: messages, then the objective list under them.
            hud.spawn((
                HudMessages,
                Pickable::IGNORE,
                Node {
                    position_type: PositionType::Absolute,
                    top: px(16),
                    left: px(16),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(4),
                    max_width: px(560),
                    ..default()
                },
            ))
            .with_children(|log| {
                for index in 0..MESSAGE_LINES {
                    log.spawn((
                        Text::default(),
                        TextFont::from_font_size(BODY_SIZE),
                        TextColor(TEXT),
                        hud_shadow(),
                        Pickable::IGNORE,
                        MessageLine(index),
                    ));
                }
                log.spawn((
                    ObjectiveList,
                    Text::default(),
                    TextFont::from_font_size(SMALL_SIZE),
                    TextColor(TEXT),
                    Pickable::IGNORE,
                    Node {
                        margin: UiRect::top(px(10)),
                        padding: UiRect::all(px(10)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
                    Visibility::Hidden,
                ));
            });

            // Bottom left: health.
            hud.spawn((
                HudHealth,
                Pickable::IGNORE,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(16),
                    left: px(16),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(4),
                    padding: UiRect::all(px(10)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
            ))
            .with_children(|panel| {
                panel
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        width: px(240),
                        ..default()
                    })
                    .with_children(|row| {
                        // "Shields": the game's own word for the player's
                        // health (`MSG_SHIELDS_AT_MAX`, `MSG_SHIELD_INCREASE`
                        // — docs/data/messages.md).
                        row.spawn(hud_text("SHIELDS", SMALL_SIZE, TEXT_DIM, fonts, true));
                        row.spawn((hud_text("", SMALL_SIZE, TEXT, fonts, true), HealthValue));
                    });
                panel
                    .spawn((
                        Node {
                            width: px(240),
                            height: px(10),
                            border: UiRect::all(px(1)),
                            ..default()
                        },
                        BorderColor::all(TEXT_DIM),
                        Pickable::IGNORE,
                    ))
                    .with_children(|bar| {
                        bar.spawn((
                            HealthFill,
                            Node {
                                width: percent(100),
                                height: percent(100),
                                ..default()
                            },
                            BackgroundColor(GREEN),
                            Pickable::IGNORE,
                        ));
                    });
            });

            // Bottom centre: the nine slots and the selected weapon.
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(16),
                    width: percent(100),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|centre| {
                centre
                    .spawn((
                        HudWeapon,
                        Pickable::IGNORE,
                        Node {
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            row_gap: px(6),
                            padding: UiRect::all(px(8)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
                    ))
                    .with_children(|panel| {
                        panel
                            .spawn((
                                Node {
                                    flex_direction: FlexDirection::Row,
                                    column_gap: px(4),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_children(|row| {
                                for slot in 1..=9 {
                                    row.spawn((
                                        SlotBox(slot),
                                        Node {
                                            width: px(30),
                                            height: px(30),
                                            border: UiRect::all(px(2)),
                                            justify_content: JustifyContent::Center,
                                            align_items: AlignItems::Center,
                                            ..default()
                                        },
                                        BorderColor::all(TEXT_DIM),
                                        BackgroundColor(Color::NONE),
                                        Pickable::IGNORE,
                                        children![hud_text(
                                            slot.to_string(),
                                            BODY_SIZE,
                                            TEXT_DIM,
                                            fonts,
                                            true
                                        )],
                                    ));
                                }
                            });
                        panel.spawn((hud_text("", BODY_SIZE, TEXT, fonts, false), WeaponLine));
                    });
            });
        });
}

/// A tight drop shadow, so HUD text reads over bright terrain. Bevy's
/// default is offset 4 px, which at HUD sizes reads as doubled text.
fn hud_shadow() -> TextShadow {
    TextShadow {
        offset: Vec2::splat(1.5),
        color: Color::srgba(0.0, 0.0, 0.0, 0.8),
    }
}

/// A HUD label, in the bold face when `bold`.
fn hud_text(
    text: impl Into<String>,
    size: f32,
    colour: Color,
    fonts: &UiFonts,
    bold: bool,
) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font: if bold {
                fonts.bold.clone().into()
            } else {
                default()
            },
            ..TextFont::from_font_size(size)
        },
        TextColor(colour),
        hud_shadow(),
        Pickable::IGNORE,
    )
}

fn clear_log(mut log: ResMut<HudLog>, mut objectives: ResMut<ObjectivesOpen>) {
    *log = HudLog::default();
    objectives.0 = false;
}

/// The keys the HUD answers: the objective list and the debug overlay.
/// `ToggleMap` is the automap's; `Screenshot` is `viewer::take_screenshot`'s,
/// which answers it in every state.
fn hud_hotkeys(
    input: ActionInput,
    mut objectives: ResMut<ObjectivesOpen>,
    debug: Option<ResMut<DebugFlags>>,
    devtools: Option<Res<DevPanelOpen>>,
) {
    if input.just_pressed(Action::ToggleObjectives) {
        objectives.0 = !objectives.0;
    }
    // With the dev tools on, `F1` opens their inspector, which has the
    // vehicle overlay as a checkbox.
    if input.just_pressed(Action::DebugPanel)
        && devtools.is_none()
        && let Some(mut debug) = debug
    {
        debug.vehicle = !debug.vehicle;
    }
}

/// Move the mission's queued lines into the log, and announce objectives as
/// they are revealed.
///
/// In the original HUD an objective's line is a *transmission*: the log says
/// `MSG_INCOMING` ("Incoming transmission - Press O key"), the message tab
/// fills, and the text itself waits in the tab's bar.
// Nine parameters: the two sources, the table that resolves them, the
// clock, and which HUD is up with its tab.
#[allow(clippy::too_many_arguments)]
fn collect_messages(
    time: Res<Time<Real>>,
    mut log: ResMut<HudLog>,
    mut state: Option<ResMut<MissionState>>,
    script: Option<Res<MissionScript>>,
    mut changes: MessageReader<ObjectiveChanged>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    roots: Query<&BuiltAs, With<HudRoot>>,
    mut tab: ResMut<original::ObjectiveTab>,
) {
    let messages = globals::messages(globals.as_deref(), &tables);
    let now = time.elapsed_secs();
    let transmissions = roots.iter().any(|built| built.original);
    let summary_of = |index: usize| {
        script
            .as_ref()
            .and_then(|script| script.objectives.get(index))
            .map(|objective| objective.summary_key.clone())
    };

    // The HUD drains `MissionState::messages`. Only
    // when it has something, so the resource is not marked changed every
    // frame.
    if let Some(state) = state.as_mut()
        && !state.messages.is_empty()
    {
        for line in state.messages.drain(..) {
            match line.objective {
                Some(index) if transmissions => {
                    tab.notify(summary_of(usize::from(index)).unwrap_or_default(), line.key);
                    log.push(message_text(messages, INCOMING_KEY), now);
                }
                _ => log.push(message_text(messages, &line.key), now),
            }
        }
    }

    // The sim queues completion lines only. A newly *active* objective — the
    // exit, once everything else is done — gets its summary here, or the
    // player would never be told to head for the VTOL.
    for change in changes.read() {
        if change.state != ObjectiveState::Active {
            continue;
        }
        let Some(objective) = script
            .as_ref()
            .and_then(|script| script.objectives.get(change.index as usize))
        else {
            continue;
        };
        if transmissions {
            tab.notify(objective.summary_key.clone(), objective.text_key.clone());
            log.push(message_text(messages, INCOMING_KEY), now);
        } else {
            log.push(message_text(messages, &objective.summary_key), now);
        }
    }
}

/// "Incoming transmission - Press O key" (docs/data/messages.md).
pub const INCOMING_KEY: &str = "MSG_INCOMING";

fn show_hud(
    visible: Res<HudVisible>,
    control: Option<Res<bft_sim::cinematic::PlayerControl>>,
    mut roots: Query<&mut Visibility, With<HudRoot>>,
) {
    // A cinematic is the scripts' camera; the HUD comes back with control.
    let cinematic = control.is_some_and(|control| control.is_cinematic());
    for mut visibility in &mut roots {
        visibility.set_if_neq(if visible.0 && !cinematic {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

fn update_health(
    players: Query<&Health, With<PlayerVehicle>>,
    mut fills: Query<(&mut Node, &mut BackgroundColor), With<HealthFill>>,
    mut values: Query<&mut Text, With<HealthValue>>,
) {
    let Some(health) = players.iter().next() else {
        return;
    };
    let fraction = health.fraction();

    for (mut node, mut colour) in &mut fills {
        node.width = percent(fraction * 100.0);
        colour.set_if_neq(BackgroundColor(health_colour(fraction)));
    }
    for mut text in &mut values {
        text.set_if_neq(Text::new(format!("{:.0}", health.current.max(0.0))));
    }
}

/// Green when healthy, orange when worn, red when nearly gone.
pub fn health_colour(fraction: f32) -> Color {
    if fraction > 0.5 {
        GREEN
    } else if fraction > 0.25 {
        ACCENT
    } else {
        ERROR
    }
}

/// How a slot box should look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotLook {
    /// Nothing in it.
    Empty,
    /// Something in it.
    Owned,
    /// The selected one.
    Selected,
}

/// The nine slots' looks for a loadout.
pub fn slot_looks(loadout: &WeaponLoadout) -> [SlotLook; 9] {
    std::array::from_fn(|index| {
        let slot = index as u8 + 1;
        if slot == loadout.selected {
            SlotLook::Selected
        } else if loadout.is_selectable(slot) {
            SlotLook::Owned
        } else {
            SlotLook::Empty
        }
    })
}

/// The selected weapon's line: short name, long name, variant, ammo.
pub fn weapon_line(
    loadout: &WeaponLoadout,
    table: Option<&WeaponTable>,
    messages: Option<&Messages>,
    form: Option<&str>,
) -> String {
    let variant = if loadout.strong { "STRONG" } else { "WEAK" };
    let Some(ammo) = loadout.selected_ammo() else {
        return format!("Slot {}  -  empty", loadout.selected);
    };
    let (short, long) = table
        .and_then(|table| table.by_index(ammo.weapon))
        .map(|def| (def.hud_name.clone(), message_text(messages, &def.desc_key)))
        .unwrap_or_else(|| (format!("#{}", ammo.weapon), String::new()));
    let rounds = if ammo.ammo == u32::MAX {
        "unlimited".to_string()
    } else {
        ammo.ammo.to_string()
    };
    let form = form
        .map(|form| format!("   |   {form}"))
        .unwrap_or_default();

    if long.is_empty() {
        format!("{short}   {variant}   Ammo {rounds}{form}")
    } else {
        format!("{short}  ({long})   {variant}   Ammo {rounds}{form}")
    }
}

#[allow(clippy::type_complexity)]
fn update_weapons(
    players: Query<(&WeaponLoadout, Option<&Vehicle>), With<PlayerVehicle>>,
    table: Option<Res<WeaponTable>>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    mut boxes: Query<(&SlotBox, &mut BorderColor, &mut BackgroundColor, &Children)>,
    mut digits: Query<&mut TextColor>,
    mut lines: Query<&mut Text, With<WeaponLine>>,
) {
    let Some((loadout, vehicle)) = players.iter().next() else {
        return;
    };
    let looks = slot_looks(loadout);

    for (slot, mut border, mut background, children) in &mut boxes {
        let look = looks[(slot.0 - 1) as usize];
        let (edge, fill, digit) = match look {
            SlotLook::Empty => (
                Color::srgba(1.0, 1.0, 1.0, 0.15),
                Color::NONE,
                TEXT_DIM.with_alpha(0.4),
            ),
            SlotLook::Owned => (TEXT_DIM, Color::NONE, TEXT),
            SlotLook::Selected => (ACCENT, ACCENT.with_alpha(0.3), TEXT),
        };
        border.set_if_neq(BorderColor::all(edge));
        background.set_if_neq(BackgroundColor(fill));
        for child in children.iter() {
            if let Ok(mut colour) = digits.get_mut(child) {
                colour.set_if_neq(TextColor(digit));
            }
        }
    }

    let messages = globals::messages(globals.as_deref(), &tables);
    let form = vehicle.map(|vehicle| format!("{:?}", vehicle.form).to_uppercase());
    let line = weapon_line(loadout, table.as_deref(), messages, form.as_deref());
    for mut text in &mut lines {
        text.set_if_neq(Text::new(line.clone()));
    }
}

fn update_messages(
    time: Res<Time<Real>>,
    log: Res<HudLog>,
    mut lines: Query<(&MessageLine, &mut Text, &mut TextColor)>,
) {
    let visible = log.visible(time.elapsed_secs());
    for (line, mut text, mut colour) in &mut lines {
        match visible.get(line.0) {
            Some((message, alpha)) => {
                text.set_if_neq(Text::new(*message));
                colour.set_if_neq(TextColor(CYAN.with_alpha(*alpha)));
            }
            None => {
                text.set_if_neq(Text::default());
            }
        }
    }
}

/// The objective list: every objective the player has been told about, done
/// or not.
pub fn objective_list(
    script: &MissionScript,
    state: &MissionState,
    messages: Option<&Messages>,
) -> String {
    let mut lines = vec!["OBJECTIVES".to_string()];
    for (objective, progress) in script.objectives.iter().zip(&state.objectives) {
        let mark = match progress {
            ObjectiveState::Inactive => continue,
            ObjectiveState::Active => "[  ]",
            ObjectiveState::Complete => "[x]",
        };
        lines.push(format!(
            "{mark}  {}",
            message_text(messages, &objective.summary_key)
        ));
    }
    if lines.len() == 1 {
        lines.push("No mission objectives here.".into());
    }
    lines.join("\n")
}

fn update_objectives(
    open: Res<ObjectivesOpen>,
    script: Option<Res<MissionScript>>,
    state: Option<Res<MissionState>>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    mut lists: Query<(&mut Text, &mut Visibility), With<ObjectiveList>>,
) {
    let text = match (&script, &state) {
        (Some(script), Some(state)) => objective_list(
            script,
            state,
            globals::messages(globals.as_deref(), &tables),
        ),
        _ => "OBJECTIVES\nNo mission objectives here.".to_string(),
    };

    for (mut list, mut visibility) in &mut lists {
        visibility.set_if_neq(if open.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        if open.0 {
            list.set_if_neq(Text::new(text.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_sim::mission::{Completion, ObjectiveDef};
    use bft_sim::weapons::{AmmoState, SlotState};

    #[test]
    fn the_log_keeps_three_lines_and_fades_them_out() {
        let mut log = HudLog::default();
        for (index, text) in ["a", "b", "c", "d"].into_iter().enumerate() {
            log.push(text.to_string(), index as f32);
        }

        let now = 3.0;
        let visible = log.visible(now);
        assert_eq!(
            visible.iter().map(|(text, _)| *text).collect::<Vec<_>>(),
            vec!["b", "c", "d"]
        );
        assert!(visible.iter().all(|(_, alpha)| *alpha == 1.0));

        // "b" was shown at 1.0: at 6.0 it has half its fade left.
        let fading = log.visible(1.0 + MESSAGE_LIFETIME - MESSAGE_FADE / 2.0);
        assert!((fading[0].1 - 0.5).abs() < 1e-6, "{fading:?}");

        assert!(log.visible(100.0).is_empty());
    }

    fn loadout() -> WeaponLoadout {
        let mut loadout = WeaponLoadout::default();
        loadout.slots[0] = SlotState {
            weak: Some(AmmoState {
                weapon: 0,
                ammo: u32::MAX,
                interval: 0.1,
                mounted: true,
            }),
            strong: None,
        };
        loadout.slots[3] = SlotState {
            weak: None,
            strong: Some(AmmoState {
                weapon: 7,
                ammo: 12,
                interval: 1.0,
                mounted: true,
            }),
        };
        loadout
    }

    #[test]
    fn the_slot_row_shows_what_is_carried_and_what_is_selected() {
        let looks = slot_looks(&loadout());

        assert_eq!(looks[0], SlotLook::Selected);
        assert_eq!(looks[3], SlotLook::Owned);
        assert_eq!(looks[1], SlotLook::Empty);
        assert_eq!(looks[8], SlotLook::Empty);
    }

    #[test]
    fn the_weapon_line_says_unlimited_rather_than_a_huge_number() {
        let line = weapon_line(&loadout(), None, None, Some("TRACK"));

        assert!(line.contains("unlimited"), "{line}");
        assert!(line.contains("WEAK"), "{line}");
        assert!(line.ends_with("TRACK"), "{line}");

        let mut strong = loadout();
        strong.selected = 4;
        strong.strong = true;
        assert!(weapon_line(&strong, None, None, None).contains("Ammo 12"));
    }

    #[test]
    fn health_turns_red_as_it_runs_out() {
        assert_eq!(health_colour(0.9), GREEN);
        assert_eq!(health_colour(0.4), ACCENT);
        assert_eq!(health_colour(0.1), ERROR);
    }

    #[test]
    fn the_objective_list_hides_what_has_not_been_revealed() {
        let objective = |index: u8, active: bool| ObjectiveDef {
            index,
            picture: String::new(),
            summary_key: format!("MSG_O{index}"),
            text_key: String::new(),
            complete_key: String::new(),
            initially_active: active,
            completion: Completion::ExitAnim,
            read_sound: None,
            hint: None,
        };
        let script = MissionScript {
            objectives: vec![objective(0, true), objective(1, false), objective(2, true)],
            read_time: 8.0,
            review_delay: 1.0,
            review_sound: String::new(),
            objective_sound: String::new(),
            new_game_anim: String::new(),
            load_game_anim: String::new(),
            movers: Vec::new(),
            death_anim: String::new(),
            final_mission: false,
        };
        let mut state = MissionState::new(bft_assets::catalog::LevelId(1), &script);
        state.objectives[0] = ObjectiveState::Complete;

        let list = objective_list(&script, &state, None);

        assert_eq!(list, "OBJECTIVES\n[x]  MSG_O0\n[  ]  MSG_O2");
    }
}
