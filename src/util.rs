use crate::{collection::CollectionId, http::RequestError};
use std::{
    fs,
    ops::Deref,
    path::{Path, PathBuf},
};
use tracing::error;

/// A wrapper around `PathBuf` that makes it impossible to access a directory
/// path without creating the dir first. The idea is to prevent all the possible
/// bugs that could occur when a directory doesn't exist.
///
/// If you just want to print the path without having to create it (e.g. for
/// debug output), use the `Debug` or `Display` impls.
#[derive(Debug, Display)]
#[display("{}", _0.display())]
pub struct Directory(PathBuf);

impl Directory {
    /// Root directory for all generated files. The value is contextual:
    /// - In development, use a directory in the current directory
    /// - In release, use a platform-specific directory in the user's home
    pub fn root() -> Self {
        if cfg!(debug_assertions) {
            Self(Path::new("./data/").into())
        } else {
            // According to the docs, this dir will be present on all platforms
            // https://docs.rs/dirs/latest/dirs/fn.data_dir.html
            Self(dirs::data_dir().unwrap().join("slumber"))
        }
    }

    /// Directory to store log files
    pub fn log() -> Self {
        Self(Self::root().0.join("log"))
    }

    /// Directory to store collection-specific data files
    pub fn data(collection_id: &CollectionId) -> Self {
        Self(Self::root().0.join(collection_id.as_str()))
    }

    /// Create this directory, and return the path. This is the only way to
    /// access the path value directly, enforcing that it can't be used without
    /// being created.
    pub fn create(self) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(&self.0)
            .context("Error creating directory `{self}`")?;
        Ok(self.0)
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

use anyhow::Context;
#[cfg(test)]
pub(crate) use assert_err;
use derive_more::Display;
