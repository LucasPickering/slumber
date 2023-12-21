//! A request collection defines recipes, profiles, etc. that make requests
//! possible

mod cereal;
mod insomnia;
mod models;

pub use models::*;

use crate::util::{parse_yaml, ResultExt};
use anyhow::{anyhow, Context};
use std::{
    fmt::Debug,
    fs,
    future::Future,
    path::{Path, PathBuf},
};
use tokio::task;
use tracing::{info, warn};

/// The support file names to be automatically loaded as a config. We only
/// support loading from one file at a time, so if more than one of these is
/// defined, we'll take the earliest and print a warning.
pub const CONFIG_FILES: &[&str] = &[
    "slumber.yml",
    "slumber.yaml",
    ".slumber.yml",
    ".slumber.yaml",
];

/// A wrapper around a request collection, to handle functionality around the
/// file system.
#[derive(Debug)]
pub struct CollectionFile {
    /// Path to the file that this collection was loaded from
    path: PathBuf,
    pub collection: Collection,
}

impl CollectionFile {
    /// Create a new collection file with the given path and a default
    /// collection. Useful when the collection failed to load and you want a
    /// placeholder.
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            path,
            collection: Default::default(),
        }
    }

    /// Load config from the given file. The caller is responsible for using
    /// [Self::detect_path] to find the file themself. This pattern enables the
    /// TUI to start up and watch the collection file, even if it's invalid.
    pub async fn load(path: PathBuf) -> anyhow::Result<Self> {
        let collection = load_collection(path.clone()).await?;
        Ok(Self { path, collection })
    }

    /// Reload a new collection from the same file used for this one.
    ///
    /// Returns `impl Future` to unlink the future from `&self`'s lifetime.
    pub fn reload(&self) -> impl Future<Output = anyhow::Result<Collection>> {
        load_collection(self.path.clone())
    }

    /// Get the path of the file that this collection was loaded from
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the path to the collection file, returning an error if none is
    /// available. This will use the override if given, otherwise it will fall
    /// back to searching the current directory for a collection.
    pub fn try_path(override_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
        override_path.or_else(detect_path).ok_or(anyhow!(
            "No collection file given and none found in current directory"
        ))
    }
}

/// Search the current directory for a config file matching one of the known
/// file names, and return it if found
fn detect_path() -> Option<PathBuf> {
    let paths: Vec<&Path> = CONFIG_FILES
        .iter()
        .map(Path::new)
        // This could be async but I'm being lazy and skipping it for now,
        // since we only do this at startup anyway (mid-process reloading
        // reuses the detected path so we don't re-detect)
        .filter(|p| p.exists())
        .collect();
    match paths.as_slice() {
        [] => None,
        [path] => Some(path.to_path_buf()),
        [first, rest @ ..] => {
            // Print a warning, but don't actually fail
            warn!(
                "Multiple config files detected. {first:?} will be used \
                    and the following will be ignored: {rest:?}"
            );
            Some(first.to_path_buf())
        }
    }
}

/// Load a collection from the given file. Takes an owned path because it
/// needs to be passed to a future
async fn load_collection(path: PathBuf) -> anyhow::Result<Collection> {
    info!(?path, "Loading collection file");
    // A bit pessimistic, huh... This gets around some lifetime struggles
    let error_context = format!("Error loading data from {path:?}");

    // This async block is really just a try block
    let result =
        task::spawn_blocking::<_, anyhow::Result<Collection>>(move || {
            let bytes = fs::read(path)?;
            let collection = parse_yaml(&bytes)?;
            Ok(collection)
        })
        .await;

    // Flatten the join error result into the inner task result. Result::flatten
    // is experimental :(
    // https://doc.rust-lang.org/std/result/enum.Result.html#method.flatten
    let result = match result {
        Ok(result) => result,
        Err(error) => Err(error.into()),
    };

    result.context(error_context).traced()
}
