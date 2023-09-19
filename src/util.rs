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
    /// If this is an error, trace it. Return the same result.
    fn traced(self) -> Self;
}

// This is deliberately *not* implemented for non-anyhow errors, because we only
// want to trace errors that have full context attached
impl<T> ResultExt<T> for anyhow::Result<T> {
    fn traced(self) -> Self {
        if let Err(err) = &self {
            error!(error = err.deref());
        }
        self
    }
}

#[cfg(test)]
macro_rules! assert_err {
    ($e:expr, $msg:expr) => {
        let msg = $msg;
        let error = $e.unwrap_err().to_string();
        assert!(
            error.contains(msg),
            "Expected error message to contain {msg:?}, but was: {error:?}"
        )
    };
}

#[cfg(test)]
pub(crate) use assert_err;
