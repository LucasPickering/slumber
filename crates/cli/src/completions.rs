//! Shell completion utilities
//!
//! To test this locally:
//! - `cargo install --path .` (current version of Slumber must be in $PATH)
//! - `COMPLETE=<shell> slumber` and pipe that to `source`
//!
//! That will enable completions for the current shell

use clap_complete::{ArgValueCompleter, CompletionCandidate, PathCompleter};
use slumber_core::{
    collection::{Collection, CollectionFile, ProfileId},
    database::Database,
};
use std::{ffi::OsStr, ops::Deref};

/// Build a completer for profile IDs
pub fn complete_profile() -> ArgValueCompleter {
    fn inner(current: &OsStr) -> Vec<CompletionCandidate> {
        let Ok(collection) = load_collection() else {
            return Vec::new();
        };

        get_candidates(
            collection.profiles.keys().map(ProfileId::to_string),
            current,
        )
    }
    ArgValueCompleter::new(inner)
}

/// Build a completer for recipe IDs
pub fn complete_recipe() -> ArgValueCompleter {
    fn inner(current: &OsStr) -> Vec<CompletionCandidate> {
        let Ok(collection) = load_collection() else {
            return Vec::new();
        };

        get_candidates(
            collection
                .recipes
                .iter()
                // Include recipe IDs only. Folder IDs are never passed to the
                // CLI
                .filter_map(|(_, node)| Some(node.recipe()?.id.to_string())),
            current,
        )
    }
    ArgValueCompleter::new(inner)
}

/// Build a completer for request IDs
pub fn complete_request_id() -> ArgValueCompleter {
    fn inner(current: &OsStr) -> Vec<CompletionCandidate> {
        let Ok(database) = Database::load() else {
            return Vec::new();
        };
        let Ok(exchanges) = database.get_all_requests() else {
            return Vec::new();
        };
        get_candidates(
            exchanges
                .into_iter()
                .map(|exchange| exchange.id.to_string()),
            current,
        )
    }
    ArgValueCompleter::new(inner)
}

/// Build a completer for `.yml` and `.yaml` files
pub fn complete_collection_path() -> ArgValueCompleter {
    ArgValueCompleter::new(PathCompleter::file().filter(|path| {
        let extension = path.extension();
        extension == Some(OsStr::new("yml"))
            || extension == Some(OsStr::new("yaml"))
    }))
}

fn load_collection() -> anyhow::Result<Collection> {
    // For now we just lean on the default collection paths. In the future we
    // should be able to look for a --file arg in the command and use that path
    let collection_file = CollectionFile::new(None)?;
    collection_file.load()
}

fn get_candidates<T: Into<String>>(
    iter: impl Iterator<Item = T>,
    current: &OsStr,
) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    // Only include IDs prefixed by the input we've gotten so far
    iter.map(T::into)
        .filter(|value| value.starts_with(current))
        .map(|value| CompletionCandidate::new(value.deref()))
        .collect()
}
