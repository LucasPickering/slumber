use anyhow::anyhow;
use std::{fmt::Debug, ops::Deref};
use tracing::error;

/// A slightly spaghetti helper for finding an item in a list by value. We
/// expect the item to be there, so if it's missing return an error with a
/// friendly message for the user.
pub fn find_by<E, T: Debug + PartialEq>(
    iter: impl Iterator<Item = E>,
    extractor: impl Fn(&E) -> T,
    target: T,
    not_found_message: &str,
) -> anyhow::Result<E> {
    // Track which ones don't match, for a potential error message
    let mut misses = Vec::new();

    for element in iter {
        let ass = extractor(&element);
        if ass == target {
            return Ok(element);
        }
        misses.push(ass);
    }

    Err(anyhow!(
        "{not_found_message} {target:?}; Options are: {misses:?}"
    ))
}

pub trait ResultExt<T>: Sized {
    /// If the result is an error, log it and swallow it. If not, return the
    /// value. Useful for low-priority errors that shouldn't be shown to the
    /// user at all.
    fn ok_or_trace(self) -> Option<T>;
}

impl<T> ResultExt<T> for anyhow::Result<T> {
    fn ok_or_trace(self) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(err) => {
                // The error should already have useful context attached
                error!(error = err.deref());
                None
            }
        }
    }
}
