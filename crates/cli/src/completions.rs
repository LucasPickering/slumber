//! Shell completion utilities

use clap_complete::CompletionCandidate;
use slumber_core::collection::{Collection, CollectionFile};
use std::{ffi::OsStr, ops::Deref};

/// Provide completions for profile IDs
pub fn complete_profile(current: &OsStr) -> Vec<CompletionCandidate> {
    let Ok(collection) = load_collection() else {
        return Vec::new();
    };

    get_candidates(collection.profiles.keys(), current)
}

/// Provide completions for recipe IDs
pub fn complete_recipe(current: &OsStr) -> Vec<CompletionCandidate> {
    let Ok(collection) = load_collection() else {
        return Vec::new();
    };

    get_candidates(
        collection
            .recipes
            .iter()
            // Include recipe IDs only. Folder IDs are never passed to the CLI
            .filter_map(|(_, node)| Some(&node.recipe()?.id)),
        current,
    )
}

fn load_collection() -> anyhow::Result<Collection> {
    // For now we just lean on the default collection paths. In the future we
    // should be able to look for a --file arg in the command and use that path
    let path = CollectionFile::try_path(None, None)?;
    Collection::load(&path)
}

fn get_candidates<'a, T: 'a + Deref<Target = str>>(
    iter: impl Iterator<Item = &'a T>,
    current: &OsStr,
) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    // Only include IDs prefixed by the input we've gotten so far
    iter.filter(|value| value.starts_with(current))
        .map(|value| CompletionCandidate::new(value.deref()))
        .collect()
}
