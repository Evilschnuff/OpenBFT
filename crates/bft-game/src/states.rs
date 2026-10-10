//! The application state machine, re-exported.
//!
//! The types themselves live in [`bft_sim::states`], because
//! [`SimSet`](bft_sim::schedule::SimSet)'s run conditions name [`AppState`]
//! and `bft-sim` cannot depend on this crate — a dedicated server builds
//! without it (NFR-4). This module re-exports them as `bft_game::states`;
//! use either path.

pub use bft_sim::states::{AppState, GameMode, Paused, PendingLevel, sim_running};
