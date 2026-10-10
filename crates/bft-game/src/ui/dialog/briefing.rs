//! The briefing, which is also the loading screen: `briefing.zrd`'s
//! `CAMPAIGN<n>` while level n loads.
//!
//! `hud.zrd`'s `BRIEFING` has words for a loading state ("Transporting" in
//! `STAGES` pulses) and a done state ("PROCEED"). // approximation: so here
//! the level loads behind the screen and then holds at the briefing
//! ([`BriefingHold`]) until the player clicks the bar or presses a key.
//!
//! On the six campaign levels the screen also briefs: each objective in
//! turn — its picture, its summary and text from the mission's
//! `objectives.zrd` and the string table — for the file's `READ_TIME`, with
//! its `LOCATOR<n>` marked on the map, while the campaign's voice-over
//! (`snd_briefing_c<n>`, the `BRIEFING<n>` sound sets) plays. The
//! multiplayer maps' layouts have only the map, the name and the bar.
//!
//! What is ours, because the data does not say: every objective is shown,
//! the hidden ones too (the layouts have a locator for each, so the
//! original showed them); an objective's `M<k>:` prefix goes in the
//! `MISSION_NAME` slot and the rest in `OBJECTIVE_SUMMARY`; the locator
//! mark is our own ring (no bitmap is named for it); and the loading words
//! are text in the file's colour, pulsing, rather than any bitmap.

use bevy::prelude::*;
use bft_assets::dialog::{BriefingLayout, ControlKind, TransportText};
use bft_assets::messages::Messages;
use bft_assets::reader::ReaderData;
use bft_assets::textures::{TextureSet, texture_key};
use bft_sim::mission::{MissionScript, build_mission_script};
use bft_sim::states::{AppState, PendingLevel};

use super::render::{Backdrop, CanvasText, DialogPainter, OnCanvas};
use super::{MenuArtLoad, MenuStyle, original_menus};
use crate::audio::{PlaySound, PlayingSound, SoundLibrary};
use crate::globals::{self, Globals};
use crate::level_view::LoadedLevel;
use crate::loading::{BriefingHold, LoadingScreen, level_progress};
use crate::ui::settings::Settings;
use crate::ui::theme::{ArtButton, Disabled, UiAction, UiButton, UiClick, UiFonts, message_text};

/// The briefing on screen, for level `level`.
#[derive(Component, Debug, Clone, Copy)]
struct BriefingScreen;

/// What the briefing is showing.
#[derive(Resource, Debug, Clone)]
struct Briefing {
    /// The level's number, `n` of `CAMPAIGN<n>`.
    campaign: u32,
    /// Real seconds when it went up.
    since: f32,
    /// The objectives, once the level's reader is in.
    script: Option<MissionScript>,
    /// The voice-over's logical name, while it may be playing.
    voice: Option<String>,
    /// Whether the level is in and the bar says to proceed.
    ready: bool,
}

#[derive(Component, Debug, Clone, Copy)]
struct ObjectivePicture;

#[derive(Component, Debug, Clone, Copy)]
enum ObjectiveLine {
    Name,
    Summary,
    Text,
}

#[derive(Component, Debug, Clone, Copy)]
struct Locator(usize);

#[derive(Component, Debug, Clone, Copy)]
struct BriefingMessage;

/// The transport bar's fill.
#[derive(Component, Debug, Clone, Copy)]
struct TransportFill {
    /// The bar's size in canvas pixels.
    size: Vec2,
}

/// The words on the bar.
#[derive(Component, Debug, Clone, Copy)]
struct TransportWords;

/// The bar itself, which is the Proceed button.
#[derive(Component, Debug, Clone, Copy)]
struct TransportBar;

/// The briefing's systems.
pub fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            show_briefing,
            read_objectives,
            brief_objectives,
            fill_transport,
            proceed,
        )
            .chain()
            .after(crate::ui::theme::dispatch_clicks)
            .run_if(in_state(AppState::Loading)),
    )
    .add_systems(OnExit(AppState::Loading), end_briefing);
}

/// Put the briefing up in place of the simple loading screen, once the
/// original menus are known to be there.
// The level, the style and what it is drawn with, the screens it replaces,
// the voice-over, and whether anyone is there to proceed.
#[allow(clippy::too_many_arguments)]
fn show_briefing(
    mut commands: Commands,
    pending: Option<Res<PendingLevel>>,
    style: Res<MenuStyle>,
    settings: Res<Settings>,
    fonts: Res<UiFonts>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    library: Option<Res<SoundLibrary>>,
    time: Res<Time<Real>>,
    shown: Query<(), With<BriefingScreen>>,
    fallback: Query<Entity, With<LoadingScreen>>,
    mut sounds: MessageWriter<PlaySound>,
    automated: Option<Res<crate::viewer::ScreenshotRun>>,
) {
    if !shown.is_empty() {
        return;
    }
    let Some(pending) = pending.filter(|pending| pending.target == AppState::InGame) else {
        return;
    };
    let Some(menus) = original_menus(Some(&style), Some(&settings)) else {
        return;
    };
    let campaign = u32::from(pending.id.0);
    let Some(layout) = menus.briefing(campaign) else {
        return;
    };
    let messages = globals::messages(globals.as_deref(), &tables);

    for entity in &fallback {
        commands.entity(entity).despawn();
    }
    let root = spawn_briefing(
        &mut commands,
        menus,
        layout,
        menus.transport.as_ref(),
        messages,
        &fonts,
    );
    commands
        .entity(root)
        .insert((BriefingScreen, DespawnOnExit(AppState::Loading)));
    // An automated capture run has no one to press a key: it goes on as
    // soon as the level is in.
    if automated.is_none() {
        commands.insert_resource(BriefingHold::default());
    }

    let voice = format!("snd_briefing_c{campaign}");
    let voice = library
        .filter(|library| library.table.get(&voice).is_some())
        .map(|_| voice);
    if let Some(voice) = &voice {
        sounds.write(PlaySound::ui(voice.clone()));
    }
    commands.insert_resource(Briefing {
        campaign,
        since: time.elapsed_secs(),
        script: None,
        voice,
        ready: false,
    });
}

fn spawn_briefing(
    commands: &mut Commands,
    menus: &super::OriginalMenus,
    layout: &BriefingLayout,
    transport: Option<&TransportText>,
    messages: Option<&Messages>,
    fonts: &UiFonts,
) -> Entity {
    let dialog = &layout.dialog;
    let (root, painter) = DialogPainter::open(
        commands,
        menus,
        dialog,
        messages,
        fonts,
        Backdrop::Black,
        10,
    );

    // The bar is drawn here, not by the generic renderer: it is a
    // progress bar and a button at once.
    painter.controls(commands, |control| {
        if matches!(control.kind, ControlKind::Slider { .. }) {
            super::render::Wiring::hidden()
        } else {
            super::render::Wiring::inert()
        }
    });

    let text = |commands: &mut Commands, name: &str, marker: ObjectiveLine| {
        let primitive = dialog.primitive(name)?;
        let font = primitive.font.unwrap_or(0);
        let entity = match primitive.wordwrap {
            Some(size) => {
                painter.wrapped(commands, primitive.pos.as_vec2(), size.x as f32, font, "")
            }
            None => painter.text(commands, primitive.pos.as_vec2(), font, ""),
        };
        commands.entity(entity).insert(marker);
        Some(entity)
    };
    text(commands, "MISSION_NAME", ObjectiveLine::Name);
    text(commands, "OBJECTIVE_SUMMARY", ObjectiveLine::Summary);
    text(commands, "OBJECTIVE_DESC", ObjectiveLine::Text);

    if let Some(primitive) = dialog.primitive("OBJECTIVE_PICT") {
        commands.spawn((
            ObjectivePicture,
            OnCanvas::sized(primitive.pos.as_vec2(), Vec2::ZERO),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            ImageNode::default(),
            Visibility::Hidden,
            Pickable::IGNORE,
            ChildOf(painter.canvas),
        ));
    }
    if let Some(primitive) = dialog.primitive("MESSAGES") {
        let entity = painter.text(
            commands,
            primitive.pos.as_vec2(),
            primitive.font.unwrap_or(0),
            message_text(messages, "MSG_LOAD_NEXT_MISSION"),
        );
        commands.entity(entity).insert(BriefingMessage);
    }

    // The locators, in the halted-transmission font's colour: the red the
    // layout keeps for warnings.
    let mark = dialog
        .primitive("TRANSMISSION_HALTED")
        .and_then(|p| p.font)
        .and_then(|font| painter.font(font))
        .map_or(Color::srgb(1.0, 0.2, 0.2), |font| {
            let [r, g, b] = font.colour;
            Color::srgb_u8(r, g, b)
        });
    // Our mark, in canvas pixels.
    const RING: f32 = 12.0;
    for (index, at) in layout.locators().into_iter().enumerate() {
        commands.spawn((
            Locator(index),
            OnCanvas::sized(at.as_vec2() - Vec2::splat(RING / 2.0), Vec2::splat(RING)),
            Node {
                position_type: PositionType::Absolute,
                border: UiRect::all(px(2)),
                border_radius: BorderRadius::MAX,
                ..default()
            },
            BorderColor::all(mark),
            Visibility::Hidden,
            Pickable::IGNORE,
            ChildOf(painter.canvas),
        ));
    }

    if let Some(bar) = layout.transport() {
        let (empty, full) = match &bar.kind {
            ControlKind::Slider { fill } => {
                (bar.bitmap.as_ref().map(|b| b.name.as_str()), Some(fill))
            }
            _ => (None, None),
        };
        let size = empty
            .and_then(|name| menus.art(name))
            .map_or(Vec2::ZERO, |art| art.size);
        let base = commands
            .spawn((
                TransportBar,
                OnCanvas::sized(bar.pos.as_vec2(), size),
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                ImageNode {
                    image: empty
                        .and_then(|name| menus.art(name))
                        .map(|art| art.image.clone())
                        .unwrap_or_default(),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Button,
                ArtButton,
                UiButton(UiAction::Proceed),
                Disabled,
                ChildOf(painter.canvas),
            ))
            .id();
        if let Some(art) = full.and_then(|name| menus.art(name)) {
            commands.spawn((
                TransportFill { size },
                OnCanvas::sized(Vec2::ZERO, Vec2::new(0.0, size.y)),
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                ImageNode {
                    image: art.image.clone(),
                    image_mode: NodeImageMode::Stretch,
                    rect: Some(Rect::new(0.0, 0.0, 0.0, art.size.y)),
                    ..default()
                },
                Pickable::IGNORE,
                ChildOf(base),
            ));
        }
        if let Some(words) = transport {
            let height = words.size as f32;
            commands.spawn((
                TransportWords,
                OnCanvas {
                    pos: Vec2::new(0.0, (size.y - height) / 2.0),
                    size: None,
                    wrap: Some(size.x),
                },
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                Text::new(words.loading.clone()),
                TextFont {
                    font: fonts.bold.clone().into(),
                    ..TextFont::from_font_size(height)
                },
                TextColor(rgb(words.loading_colour)),
                CanvasText(height),
                TextLayout::justify(Justify::Center),
                Pickable::IGNORE,
                ChildOf(base),
            ));
        }
    }
    root
}

fn rgb([r, g, b]: [u8; 3]) -> Color {
    Color::srgb_u8(r, g, b)
}

/// Build the objectives once the level's reader has loaded.
fn read_objectives(
    briefing: Option<ResMut<Briefing>>,
    loaded: Option<Res<LoadedLevel>>,
    readers: Res<Assets<ReaderData>>,
) {
    let (Some(mut briefing), Some(loaded)) = (briefing, loaded) else {
        return;
    };
    if briefing.script.is_some() {
        return;
    }
    if let Some(reader) = loaded
        .reader
        .as_ref()
        .and_then(|handle| readers.get(handle))
    {
        briefing.script = build_mission_script(reader);
    }
}

/// Which objective the briefing is on after `elapsed` seconds, each shown
/// for `read_time`, round and round.
pub fn objective_at(elapsed: f32, read_time: f32, count: usize) -> Option<usize> {
    (count > 0).then(|| ((elapsed / read_time.max(0.5)) as usize) % count)
}

/// Split a summary into its `M<k>` prefix and the rest: "M1: Operation
/// Short Circuit" is the mission's first of four, the original's own
/// numbering.
pub fn split_summary(summary: &str) -> (String, String) {
    match summary.split_once(':') {
        Some((prefix, rest)) if prefix.len() <= 4 => {
            (prefix.trim().to_string(), rest.trim().to_string())
        }
        _ => (String::new(), summary.trim().to_string()),
    }
}

/// Show the current objective: picture, lines, and its locator blinking.
// Nine parameters: the briefing, the art and strings it reads, and the
// four kinds of node it fills.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn brief_objectives(
    briefing: Option<Res<Briefing>>,
    time: Res<Time<Real>>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    load: Res<MenuArtLoad>,
    sets: Res<Assets<TextureSet>>,
    images: Res<Assets<Image>>,
    mut lines: Query<(&ObjectiveLine, &mut Text)>,
    mut pictures: Query<
        (&mut ImageNode, &mut OnCanvas, &mut Visibility),
        (With<ObjectivePicture>, Without<Locator>),
    >,
    mut locators: Query<(&Locator, &mut Visibility), Without<ObjectivePicture>>,
) {
    let Some(briefing) = briefing else {
        return;
    };
    let Some(script) = &briefing.script else {
        return;
    };
    let elapsed = time.elapsed_secs() - briefing.since;
    let Some(index) = objective_at(elapsed, script.read_time, script.objectives.len()) else {
        return;
    };
    let objective = &script.objectives[index];
    let messages = globals::messages(globals.as_deref(), &tables);

    let (name, summary) = split_summary(&message_text(messages, &objective.summary_key));
    let description = message_text(messages, &objective.text_key);
    for (line, mut text) in &mut lines {
        let wanted = match line {
            ObjectiveLine::Name => &name,
            ObjectiveLine::Summary => &summary,
            ObjectiveLine::Text => &description,
        };
        if text.0 != *wanted {
            text.0 = wanted.clone();
        }
    }

    let picture = load
        .set
        .as_ref()
        .and_then(|set| sets.get(set))
        .and_then(|set| set.image(&texture_key(&objective.picture)))
        .cloned();
    for (mut image, mut placed, mut visibility) in &mut pictures {
        match &picture {
            Some(handle) => {
                if image.image != *handle {
                    image.image = handle.clone();
                    image.image_mode = NodeImageMode::Stretch;
                    let size = images
                        .get(handle)
                        .map_or(Vec2::ZERO, |i| i.size().as_vec2());
                    placed.size = Some(size);
                }
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }

    // Twice a second.
    let lit = ((elapsed * 2.0) as u32).is_multiple_of(2);
    for (locator, mut visibility) in &mut locators {
        visibility.set_if_neq(if locator.0 == index && lit {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// Fill the transport bar as the level loads, pulse the loading words
/// through their stages, and say to proceed once it is in.
// The level and its loader, the words, and the nodes the bar and the
// loading line are made of.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn fill_transport(
    mut commands: Commands,
    briefing: Option<ResMut<Briefing>>,
    loaded: Option<Res<LoadedLevel>>,
    server: Res<AssetServer>,
    style: Res<MenuStyle>,
    time: Res<Time<Real>>,
    images: Res<Assets<Image>>,
    mut fills: Query<(&TransportFill, &mut OnCanvas, &mut ImageNode)>,
    mut words: Query<(&mut Text, &mut TextColor), With<TransportWords>>,
    mut lines: Query<&mut Visibility, With<BriefingMessage>>,
    bars: Query<(Entity, Has<Disabled>), With<TransportBar>>,
) {
    let Some(mut briefing) = briefing else {
        return;
    };
    let progress = loaded
        .as_ref()
        .map_or(0.0, |loaded| level_progress(loaded, &server));
    let ready = loaded.as_ref().is_some_and(|loaded| loaded.is_spawned());
    if briefing.ready != ready {
        briefing.ready = ready;
    }

    for (fill, mut placed, mut image) in &mut fills {
        let width = fill.size.x * progress;
        let texels = images
            .get(&image.image)
            .map_or(fill.size.x, |image| image.width() as f32);
        placed.set_if_neq(OnCanvas::sized(Vec2::ZERO, Vec2::new(width, fill.size.y)));
        let rect = Rect::new(0.0, 0.0, texels * progress, fill.size.y);
        if image.rect != Some(rect) {
            image.rect = Some(rect);
        }
    }

    let transport = match &*style {
        MenuStyle::Original(menus) => menus.transport.as_ref(),
        _ => None,
    };
    if let Some(transport) = transport {
        let elapsed = time.elapsed_secs() - briefing.since;
        let (line, colour) = if ready {
            (&transport.ready, rgb(transport.ready_colour))
        } else {
            let stages = transport.stages.max(1);
            let stage = (elapsed * stages as f32) as u32 % stages;
            let brightness = 0.4 + 0.6 * (stage + 1) as f32 / stages as f32;
            let colour = rgb(transport.loading_colour).to_srgba();
            (
                &transport.loading,
                Color::srgba(
                    colour.red * brightness,
                    colour.green * brightness,
                    colour.blue * brightness,
                    1.0,
                ),
            )
        };
        for (mut text, mut text_colour) in &mut words {
            if text.0 != *line {
                text.0 = line.clone();
            }
            text_colour.set_if_neq(TextColor(colour));
        }
    }

    // "Loading next mission..." only while it is.
    for mut visibility in &mut lines {
        visibility.set_if_neq(if ready {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        });
    }

    for (bar, disabled) in &bars {
        if ready && disabled {
            commands.entity(bar).remove::<Disabled>();
        }
    }
}

/// Proceed: the bar clicked, or any key, once the level is in.
fn proceed(
    briefing: Option<Res<Briefing>>,
    mut clicks: MessageReader<UiClick>,
    keys: Res<ButtonInput<KeyCode>>,
    hold: Option<ResMut<BriefingHold>>,
) {
    let clicked = clicks.read().any(|click| click.0 == UiAction::Proceed);
    let (Some(briefing), Some(mut hold)) = (briefing, hold) else {
        return;
    };
    if briefing.ready && (clicked || keys.get_just_pressed().next().is_some()) {
        hold.released = true;
    }
}

/// Leaving the briefing: its voice-over stops with it.
fn end_briefing(
    mut commands: Commands,
    briefing: Option<Res<Briefing>>,
    playing: Query<(Entity, &PlayingSound)>,
) {
    let Some(briefing) = briefing else {
        return;
    };
    if let Some(voice) = &briefing.voice {
        for (entity, sound) in &playing {
            if sound.name == *voice {
                commands.entity(entity).despawn();
            }
        }
    }
    debug!("briefing for level {} done", briefing.campaign);
    commands.remove_resource::<Briefing>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_briefing_cycles_through_the_objectives() {
        assert_eq!(objective_at(0.0, 8.0, 4), Some(0));
        assert_eq!(objective_at(8.5, 8.0, 4), Some(1));
        assert_eq!(objective_at(33.0, 8.0, 4), Some(0));
        assert_eq!(objective_at(3.0, 8.0, 0), None);
    }

    #[test]
    fn a_summary_splits_into_its_number_and_its_name() {
        assert_eq!(
            split_summary("M1: Operation Short Circuit"),
            ("M1".to_string(), "Operation Short Circuit".to_string())
        );
        assert_eq!(
            split_summary("No number here"),
            (String::new(), "No number here".to_string())
        );
        assert_eq!(
            split_summary("A longer prefix: then text"),
            (String::new(), "A longer prefix: then text".to_string())
        );
    }
}
