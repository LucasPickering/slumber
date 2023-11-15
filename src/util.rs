use anyhow::anyhow;
use std::{
    fmt::Debug,
    ops::Deref,
    path::{Path, PathBuf},
};
use tracing::error;

/// Where to store data and log files. The value is contextual:
/// - In development, use a directory in the current directory
/// - In release, use a platform-specific directory in the user's home
pub fn data_directory() -> PathBuf {
    if cfg!(debug_assertions) {
        Path::new("./data/").into()
    } else {
        // According to the docs, this dir will be present on all platforms
        // https://docs.rs/dirs/latest/dirs/fn.data_dir.html
        dirs::data_dir().unwrap().join("slumber")
    }
}

/// A slightly spaghetti helper for finding an item in a list by value. We
/// expect the item to be there, so if it's missing return an error with a
/// friendly message for the user.
pub fn find_by<E, K: Debug + PartialEq>(
    mut vec: Vec<E>,
    extractor: impl Fn(&E) -> &K,
    target: &K,
    not_found_message: &str,
) -> anyhow::Result<E> {
    let index = vec.iter().position(|element| extractor(element) == target);
    match index {
        Some(index) => Ok(vec.swap_remove(index)),
        None => {
            let options: Vec<&K> = vec.iter().map(extractor).collect();
            Err(anyhow!(
                "{not_found_message} {target:?}; Options are: {options:?}"
            ))
        }
    }
}

pub trait ResultExt<T, E>: Sized {
    /// If this is an error, trace it. Return the same result.
    fn traced(self) -> Self;
}

// This is deliberately *not* implemented for non-anyhow errors, because we only
// want to trace errors that have full context attached
impl<T> ResultExt<T, anyhow::Error> for anyhow::Result<T> {
    fn traced(self) -> Self {
        if let Err(err) = &self {
            error!(error = err.deref());
        }
        self
    }
}

impl<T> ResultExt<T, RequestError> for Result<T, RequestError> {
    fn traced(self) -> Self {
        if let Err(err) = &self {
            error!(error = %err);
        }
        self
    }
}

#[cfg(test)]
macro_rules! assert_err {
    ($e:expr, $msg:expr) => {{
        use itertools::Itertools as _;

        let msg = $msg;
        // Include all source errors so wrappers don't hide the important stuff
        let error: anyhow::Error = $e.unwrap_err().into();
        let actual = error.chain().map(ToString::to_string).join(": ");
        assert!(
            actual.contains(msg),
            "Expected error message to contain {msg:?}, but was: {actual:?}"
        )
    }};
}

use crate::http::RequestError;
#[cfg(test)]
pub(crate) use assert_err;
