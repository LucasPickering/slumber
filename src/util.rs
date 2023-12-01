use crate::http::RequestError;
use std::{
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

#[cfg(test)]
pub(crate) use assert_err;
