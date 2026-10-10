//! The assets every level needs, loaded once during boot.
//!
//! `BootPhase::LoadingGlobals` is where the message table, the global reader
//! archive and the sound bank belong: the string table (FR-U4) is what every
//! menu, HUD line and objective resolves through, the global reader holds the
//! vehicle, weapon, pickup and sound tables are built from, and the bank is every sound effect. All three are per-install, not
//! per-level, so loading them once at boot keeps a level change from
//! re-reading 127 reader entries, 749 strings and 280 WAVs.
//!
//! The [`Campaign`] is built here too, from the catalog: it is the list of
//! playable missions the front end offers and the thing progress is recorded
//! against.

use bevy::asset::LoadState;
use bevy::prelude::*;
use bft_assets::catalog::LevelCatalog;
use bft_assets::messages::Messages;
use bft_assets::reader::ReaderData;
use bft_assets::sounds::SoundBank;
use bft_sim::mission::{Progress, build_campaign};

/// Handles to the globals, and whether they have settled.
#[derive(Resource, Debug, Clone, Default)]
pub struct Globals {
    /// The string table. `None` when the cache has no `messages.json`.
    pub messages: Option<Handle<Messages>>,
    /// The global reader archive: `vehicle.zrd`, `weapons.zrd`, `pickup.zrd`,
    /// `sounds.zrd`, `briefing.zrd`.
    pub reader: Option<Handle<ReaderData>>,
    /// The sound effects. `None` when the cache has no sound
    /// archive; the game then runs silent.
    pub sounds: Option<Handle<SoundBank>>,
}

impl Globals {
    /// Whether every handle has finished, one way or the other.
    ///
    /// A failed global is *settled*, not pending: the engine runs without a
    /// string table (showing `MSG_*` keys) and boot must not hang waiting for
    /// one that will never arrive.
    pub fn is_settled(&self, server: &AssetServer) -> bool {
        let states = [
            self.messages.as_ref().map(|handle| handle.id().untyped()),
            self.reader.as_ref().map(|handle| handle.id().untyped()),
            self.sounds.as_ref().map(|handle| handle.id().untyped()),
        ];

        states.into_iter().flatten().all(|id| {
            matches!(
                server.load_state(id),
                LoadState::Loaded | LoadState::Failed(_)
            )
        })
    }
}

/// Start loading the globals and build the campaign.
///
/// Called once, when boot reaches `LoadingGlobals`.
pub fn start_loading_globals(
    commands: &mut Commands,
    catalog: &LevelCatalog,
    server: &AssetServer,
) {
    let globals = Globals {
        messages: catalog
            .messages
            .clone()
            .map(|path| server.load::<Messages>(path)),
        reader: catalog
            .global_reader
            .clone()
            .map(|path| server.load::<ReaderData>(path)),
        sounds: catalog
            .sounds
            .clone()
            .map(|path| server.load::<SoundBank>(path)),
    };

    if globals.messages.is_none() {
        // Not fatal: every label falls back to its `MSG_*` key, which is
        // legible enough to play by and obvious enough to report.
        warn!("no messages.json in the cache; the UI will show message keys");
    }
    if globals.sounds.is_none() {
        warn!("no sound archive in the cache; the game will be silent");
    }

    let campaign = build_campaign(catalog);
    info!(
        "{} campaign mission(s), {} level(s) in all",
        campaign.missions.len(),
        catalog.levels.len()
    );

    commands.insert_resource(globals);
    commands.insert_resource(campaign);
}

/// Read the saved progress, or start a fresh campaign.
///
/// `unlock_all` is `bft-bin`'s `--unlock-all` (FR-M7): it opens every mission
/// and every vehicle form, mounts every weapon full, and neither reads nor
/// writes the save file, so a test run does not overwrite a real campaign.
pub fn load_progress(unlock_all: bool) -> Progress {
    if unlock_all {
        info!("--unlock-all: every mission and form is available");
        return Progress::unlocked_all();
    }

    match Progress::load() {
        Ok(progress) => {
            if !progress.completed.is_empty() {
                info!("{} mission(s) completed so far", progress.completed.len());
            }
            progress
        }
        Err(e) => {
            // A save that will not parse is a save from an older build or a
            // corrupted one; starting over beats refusing to boot.
            warn!("cannot read the saved progress ({e}); starting a new campaign");
            Progress::default()
        }
    }
}

/// The message table, once it has loaded.
///
/// A helper rather than a resource so a caller that has no table — a headless
/// server, a run against a cache with no `messages.json` — gets `None` and
/// falls back to the key, instead of every call site repeating the lookup.
pub fn messages<'a>(
    globals: Option<&Globals>,
    tables: &'a Assets<Messages>,
) -> Option<&'a Messages> {
    tables.get(globals?.messages.as_ref()?)
}

/// A `MSG_*` key resolved through the table, or the key itself.
pub fn text(globals: Option<&Globals>, tables: &Assets<Messages>, key: &str) -> String {
    messages(globals, tables)
        .map(|table| table.text(key))
        .unwrap_or_else(|| key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::catalog::{LevelEntry, LevelId, LevelKind};
    use bft_sim::vehicle::FormSet;

    fn catalog() -> LevelCatalog {
        let level = |id: u16, kind: LevelKind| LevelEntry {
            id: LevelId(id),
            name: format!("m{id}"),
            title_key: format!("MSG_CAMPAIGN{id}_NAME"),
            gamez: format!("cache://gamez/m{id}-gamez.gamez.zip").into(),
            anim: Some(format!("cache://anim/m{id}-anim.anim.zip").into()),
            reader: format!("cache://reader/m{id}-zrdr.rdr.zip").into(),
            zmap: None,
            zmap_is_placeholder: false,
            textures: Vec::new(),
            kind,
        };

        LevelCatalog {
            levels: vec![
                level(2, LevelKind::Campaign { campaign: 2 }),
                level(1, LevelKind::Campaign { campaign: 1 }),
                level(
                    9,
                    LevelKind::Multiplayer {
                        mode: bft_assets::catalog::MultiplayerMode::Race,
                    },
                ),
            ],
            ..default()
        }
    }

    /// The campaign is the campaign levels, in order, and nothing else: a
    /// deathmatch map is not a mission.
    #[test]
    fn the_campaign_is_the_campaign_levels_in_order() {
        let campaign = build_campaign(&catalog());

        assert_eq!(
            campaign
                .missions
                .iter()
                .map(|mission| mission.id.0)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(campaign.missions[0].title_key, "MSG_CAMPAIGN1_NAME");
        assert_eq!(
            campaign.missions[0].briefing_video.as_deref(),
            Some("video/m01_01.avi")
        );
        assert_eq!(campaign.missions[1].briefing_layout, 2);
    }

    #[test]
    fn the_next_mission_is_the_one_after_it() {
        let campaign = build_campaign(&catalog());

        assert_eq!(
            campaign.next_after(LevelId(1)).map(|mission| mission.id),
            Some(LevelId(2))
        );
        assert_eq!(campaign.next_after(LevelId(2)), None);
    }

    /// `--unlock-all` must not read or write the save file.
    #[test]
    fn unlock_all_opens_everything_without_touching_the_save() {
        let progress = load_progress(true);

        assert_eq!(progress.unlocked, FormSet::all());
        assert!(progress.unlock_all, "so it is never saved, and arms fully");
        for mission in 1..=6 {
            assert!(progress.is_unlocked(LevelId(mission)));
        }
    }

    /// An empty `Globals` is settled: a cache with no globals must not hang
    /// boot waiting for handles that do not exist.
    #[test]
    fn globals_with_no_handles_are_already_settled() {
        let mut app = App::new();
        app.add_plugins((bevy::MinimalPlugins, bevy::asset::AssetPlugin::default()));

        let server = app.world().resource::<AssetServer>();

        assert!(Globals::default().is_settled(server));
    }
}
