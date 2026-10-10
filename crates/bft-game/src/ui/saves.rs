//! The save lists: the pause menu's Save and Load views and the front end's
//! Load screen share one list builder.
//!
//! A list shows the newest [`LIST_LENGTH`] saves from [`SaveListing`]; a
//! button's action carries the entry's index there. A save this build cannot
//! load — unreadable, or written by a newer format (SAVEGAME §4) — is listed
//! greyed out with the reason, so a player can see it exists.

use bevy::prelude::*;
use bft_assets::messages::Messages;

use super::MenuScreen;
use super::theme::{
    BODY_SIZE, Disabled, ERROR, SMALL_SIZE, TEXT_DIM, TITLE_SIZE, UiAction, UiClick, UiFonts,
    button, label_text, screen_root, title_text,
};
use crate::globals::{self, Globals};
use crate::savegame::{LoadRequest, SaveListing, describe};

/// How many saves a list shows, newest first.
pub const LIST_LENGTH: usize = 10;

/// What a list is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMode {
    /// Pick a save to overwrite, or make a new one.
    Save,
    /// Pick a save to load.
    Load,
}

/// The front end's Load screen, and the load clicks from any list.
pub fn plugin(app: &mut App) {
    app.add_systems(
        OnEnter(MenuScreen::Load),
        spawn_load_screen.run_if(super::dialog::screens::fallback_page(
            super::dialog::screens::Page::Loads,
        )),
    )
    .add_systems(Update, load_clicked.after(super::theme::dispatch_clicks));
}

/// Buttons for the saves, under `parent`.
pub fn spawn_save_list(
    parent: &mut ChildSpawnerCommands,
    listing: &SaveListing,
    mode: ListMode,
    messages: Option<&Messages>,
) {
    if mode == ListMode::Save {
        parent.spawn(button("New save", UiAction::SaveNew, 520.0));
    }

    if listing.entries.is_empty() {
        parent.spawn(label_text("No saved games.", BODY_SIZE, TEXT_DIM));
        return;
    }

    for (index, entry) in listing.entries.iter().enumerate().take(LIST_LENGTH) {
        let action = match mode {
            ListMode::Save => UiAction::SaveOver(index as u16),
            ListMode::Load => UiAction::LoadSave(index as u16),
        };
        let mut row = parent.spawn(button(describe(entry, messages), action, 520.0));
        // Overwriting a save this build cannot read is allowed; loading one
        // is not.
        if mode == ListMode::Load && !entry.is_loadable() {
            row.insert(Disabled);
        }
        if let Some(problem) = &entry.problem {
            parent.spawn(label_text(problem.clone(), SMALL_SIZE, ERROR));
        }
    }
    if listing.entries.len() > LIST_LENGTH {
        parent.spawn(label_text(
            format!("… and {} older", listing.entries.len() - LIST_LENGTH),
            SMALL_SIZE,
            TEXT_DIM,
        ));
    }
}

fn spawn_load_screen(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut listing: ResMut<SaveListing>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
) {
    listing.refresh();
    let messages = globals::messages(globals.as_deref(), &tables);

    commands
        .spawn((screen_root(), DespawnOnExit(MenuScreen::Load)))
        .with_children(|root| {
            root.spawn(title_text("Load game", TITLE_SIZE, &fonts));
            spawn_save_list(root, &listing, ListMode::Load, messages);
            root.spawn(button("Back", UiAction::Back, 520.0));
        });
}

/// A load button, from the front end or the pause menu.
fn load_clicked(
    mut clicks: MessageReader<UiClick>,
    listing: Res<SaveListing>,
    mut loads: MessageWriter<LoadRequest>,
) {
    for click in clicks.read() {
        let UiAction::LoadSave(index) = click.0 else {
            continue;
        };
        if let Some(entry) = listing.entries.get(index as usize)
            && entry.is_loadable()
        {
            loads.write(LoadRequest::Path(entry.path.clone()));
        }
    }
}
