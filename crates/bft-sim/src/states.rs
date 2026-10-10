//! The application lifecycle: which of boot, menu, loading or play is running.
//!
//! These types live in `bft-sim` rather than `bft-game` because [`SimSet`](crate::schedule::SimSet)
//! is configured here and its run conditions name [`AppState`]. A dedicated
//! server has no `bft-game` at all (NFR-4) but still boots, loads a level and
//! then plays, so the state machine is not a presentation concern.
//! `bft_game::states` re-exports every item, so either path works.

use bevy::prelude::*;
use bft_assets::catalog::LevelId;

/// What the application is doing.
///
/// `Boot` is the default so an `App` that never sets a state starts where a
/// real run does: looking for the data.
#[derive(States, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum AppState {
    /// Finding the install, extracting what the cache is missing, loading
    /// globals. [`crate::states::AppState::FrontEnd`] follows, or `Loading`
    /// when the command line already asked for a level.
    #[default]
    Boot,
    /// Menus. The sim is not running.
    FrontEnd,
    /// A level's assets are loading and its hierarchy is being spawned.
    Loading,
    /// Playing: the sim runs in `FixedUpdate`.
    InGame,
    /// The free-fly level viewer (FR-T1). No sim.
    Viewer,
}

impl AppState {
    /// Whether this is a state [`PendingLevel`] may target.
    ///
    /// Only `InGame` and `Viewer` are somewhere a loaded level can be shown;
    /// a request for anything else is a bug in the caller, and the loading
    /// path treats it as `Viewer` rather than stranding the user.
    pub fn is_level_target(&self) -> bool {
        matches!(self, AppState::InGame | AppState::Viewer)
    }
}

/// Which kind of game is being played, while [`AppState::InGame`].
///
/// The level decides it: m1–m6 are campaigns, m9 is the race map and m13 the
/// teleporter arena, the rest deathmatch (docs/data/install.md). Only campaign and
/// deathmatch exist yet; `Race` and `Teleport` come with something to run.
#[derive(SubStates, Clone, PartialEq, Eq, Hash, Debug, Default)]
#[source(AppState = AppState::InGame)]
pub enum GameMode {
    /// One of the six campaign missions.
    #[default]
    Campaign,
    /// A multiplayer map.
    Deathmatch,
}

/// The level that has been asked for but not yet loaded.
///
/// Set it and go to [`AppState::Loading`]; the loading state reads it, hands
/// the id to the level loader and enters [`Self::target`] when the level is
/// spawned.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct PendingLevel {
    /// Which level.
    pub id: LevelId,
    /// Where to go once it is spawned: [`AppState::InGame`] or
    /// [`AppState::Viewer`].
    pub target: AppState,
}

impl PendingLevel {
    /// Ask for a level in the free-fly viewer.
    pub fn viewer(id: LevelId) -> PendingLevel {
        PendingLevel {
            id,
            target: AppState::Viewer,
        }
    }

    /// Ask for a level to play.
    pub fn in_game(id: LevelId) -> PendingLevel {
        PendingLevel {
            id,
            target: AppState::InGame,
        }
    }

    /// The state to enter once the level is up, guarding against a target
    /// that is not somewhere a level can be shown.
    pub fn target_state(&self) -> AppState {
        if self.target.is_level_target() {
            self.target.clone()
        } else {
            warn!(
                "level {} was asked for with target {:?}, which cannot show a \
                 level; opening the viewer instead",
                self.id, self.target
            );
            AppState::Viewer
        }
    }
}

/// Whether the simulation is paused.
///
/// Read by the [`SimSet`](crate::schedule::SimSet) run conditions, so a
/// paused game stops stepping without any subsystem knowing about pausing.
/// The pause overlay sets it.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Paused(pub bool);

/// Whether the simulation should step this tick: playing, and not paused.
///
/// A run condition rather than a method so every `SimSet` can share it.
pub fn sim_running(
    state: Option<Res<State<AppState>>>,
    paused: Option<Res<Paused>>,
    restoring: Option<Res<crate::save::RestoreGate>>,
) -> bool {
    // No `State<AppState>` at all means nobody set the machine up — a unit
    // test driving `FixedUpdate` directly, say. Running is the useful answer
    // there; a state machine that exists is always honoured.
    let playing = state.is_none_or(|state| *state.get() == AppState::InGame);
    let paused = paused.is_some_and(|paused| paused.0);
    // A save is about to replace this world: nothing may step it first.
    let restoring = restoring.is_some();

    playing && !paused && !restoring
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_in_game_and_viewer_can_show_a_level() {
        assert!(AppState::InGame.is_level_target());
        assert!(AppState::Viewer.is_level_target());
        assert!(!AppState::FrontEnd.is_level_target());
        assert!(!AppState::Loading.is_level_target());
        assert!(!AppState::Boot.is_level_target());
    }

    #[test]
    fn a_nonsense_target_opens_the_viewer_rather_than_stranding_the_level() {
        let pending = PendingLevel {
            id: LevelId(1),
            target: AppState::FrontEnd,
        };

        assert_eq!(pending.target_state(), AppState::Viewer);
    }

    #[test]
    fn the_sim_runs_only_while_playing_and_unpaused() {
        fn check(state: AppState, paused: bool) -> bool {
            let mut world = World::new();
            world.insert_resource(State::new(state));
            world.insert_resource(Paused(paused));
            world.run_system_cached(sim_running).unwrap()
        }

        assert!(check(AppState::InGame, false));
        assert!(!check(AppState::InGame, true));
        assert!(!check(AppState::Viewer, false));
        assert!(!check(AppState::FrontEnd, false));
    }

    /// A `bft-sim` unit test that drives `FixedUpdate` without ever adding
    /// `StatesPlugin` must still see its systems run, or every existing sim
    /// test would have to learn about a state machine it does not use.
    #[test]
    fn no_state_machine_means_the_sim_still_runs() {
        let mut world = World::new();

        assert!(world.run_system_cached(sim_running).unwrap());
    }
}
