//! Import from external formats into Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod insomnia;
mod openapi;
mod rest;

use std::{
    convert::Infallible,
    ffi::OsStr,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::Context;
use futures::TryFutureExt;
pub use insomnia::from_insomnia;
pub use openapi::from_openapi;
use reqwest::{Response, Url};
pub use rest::from_rest;
use tokio::fs;
use tracing::info;

/// Pointer to a file that should be imported
#[derive(Clone, Debug, derive_more::Display)]
pub enum ImportInput {
    #[display("{_0}")]
    Url(Url),
    #[display("{}", _0.display())]
    Path(PathBuf),
}

impl ImportInput {
    /// Load the contents of the import input
    async fn load(&self) -> anyhow::Result<String> {
        match self {
            Self::Url(url) => {
                info!(%url, "Fetching remote file for import");
                let content = reqwest::get(url.clone())
                    .and_then(Response::text)
                    .await
                    .with_context(|| {
                        format!("Error importing HTTP URL {url}")
                    })?;
                Ok(content)
            }
            Self::Path(path) => {
                info!(?path, "Reading local file for import");
                let content =
                    fs::read_to_string(path).await.with_context(|| {
                        format!("Error importing local file {path:?}")
                    })?;
                Ok(content)
            }
        }
    }

    /// Get the name of the input file. For a path this just grabs the
    /// file name from the path. For a URL it treats the path component of the
    /// URL as a file path and gets the file namefrom there.
    ///
    /// ```
    /// assert_eq!(
    ///     ImportInput::from_str("./openapi.json").unwrap().file_name(),
    ///     Some("openapi.json")
    /// );
    /// assert_eq!(
    ///     ImportInput::from_str("https://example.com/openapi.json")
    ///         .unwrap()
    ///         .file_name(),
    ///     Some("openapi.json")
    /// );
    /// assert_eq!(
    ///     ImportInput::from_str("https://example.com/")
    ///         .unwrap()
    ///         .file_name(),
    ///     None
    /// );
    /// ```
    fn file_name(&self) -> Option<&OsStr> {
        match self {
            Self::Url(url) => Path::new(url.path()).file_name(),
            Self::Path(path) => path.file_name(),
        }
    }
}

impl FromStr for ImportInput {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // If it's a URL, it's a URL. Otherwise, it's a path
        if let Ok(url) = s.parse::<Url>() {
            Ok(Self::Url(url))
        } else {
            Ok(Self::Path(PathBuf::from(s)))
        }
    }
}
