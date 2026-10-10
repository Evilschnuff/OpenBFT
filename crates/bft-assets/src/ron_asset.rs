//! A generic RON asset loader, for the `assets/tuning/*.ron` override files.
//!
//! The tuning files are **ours** (`assets/tuning/README.md`) and are an
//! override layer over the game's own reader data (DESIGN §5.3), so every
//! leaf in them is optional and an empty file is the correct default. One
//! loader serves all of them: they differ only in their type.
//!
//! ```no_run
//! # use bevy::prelude::*;
//! # use bft_assets::ron_asset::RonAssetLoader;
//! # #[derive(Asset, TypePath, serde::Deserialize, Default)]
//! # struct MyTuning;
//! # fn register(app: &mut App) {
//! app.init_asset::<MyTuning>()
//!     .register_asset_loader(RonAssetLoader::<MyTuning>::new(&["tuning.ron"]));
//! # }
//! ```

use std::marker::PhantomData;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::*;
use serde::Deserialize;

/// What can go wrong loading a RON asset.
#[derive(Debug, thiserror::Error)]
pub enum RonAssetError {
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),
    /// RON's own error already carries the line, column and what it wanted,
    /// which is what a hand-edited tuning file needs to report.
    #[error("not valid RON: {0}")]
    Ron(#[from] ron::error::SpannedError),
}

/// Loads any `Deserialize` asset from a RON file.
///
/// The extensions are given per instance rather than per type so two tuning
/// files with different shapes (`vehicle.ron`, `camera.ron`) can each claim
/// their own name without a newtype apiece.
#[derive(TypePath, Debug)]
pub struct RonAssetLoader<A: Asset> {
    extensions: Vec<&'static str>,
    marker: PhantomData<fn() -> A>,
}

impl<A: Asset> RonAssetLoader<A> {
    /// A loader for these extensions.
    ///
    /// Give the *full* extension as Bevy sees it — `"vehicle.ron"`, not
    /// `"ron"` — so one tuning file's loader never picks up another's.
    pub fn new(extensions: &[&'static str]) -> RonAssetLoader<A> {
        RonAssetLoader {
            extensions: extensions.to_vec(),
            marker: PhantomData,
        }
    }
}

impl<A> AssetLoader for RonAssetLoader<A>
where
    A: Asset + for<'de> Deserialize<'de>,
{
    type Asset = A;
    type Settings = ();
    type Error = RonAssetError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _context: &mut LoadContext<'_>,
    ) -> Result<A, RonAssetError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        Ok(ron::de::from_bytes(&bytes)?)
    }

    fn extensions(&self) -> &[&str] {
        &self.extensions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Asset, TypePath, Deserialize, Debug, Default, PartialEq)]
    struct Tuning {
        speed: Option<f32>,
        name: Option<String>,
    }

    fn parse(text: &str) -> Result<Tuning, RonAssetError> {
        Ok(ron::de::from_bytes(text.as_bytes())?)
    }

    /// The shape every tuning file has: all-optional, so an empty record is
    /// "use the game's own values" and not an error.
    #[test]
    fn an_all_default_record_parses_to_the_default() {
        assert_eq!(parse("()").expect("parses"), Tuning::default());
    }

    #[test]
    fn a_partial_record_leaves_the_rest_none() {
        let tuning = parse("(speed: Some(12.0))").expect("parses");

        assert_eq!(tuning.speed, Some(12.0));
        assert_eq!(tuning.name, None);
    }

    /// A hand-edited file's mistake has to say where it is, or nobody can fix
    /// it — the loader exists for files humans type.
    #[test]
    fn a_syntax_error_reports_its_position() {
        let error = parse("(speed: Some(12.0)").expect_err("should not parse");

        let message = error.to_string();
        assert!(message.contains("1:"), "no position in {message:?}");
    }
}
