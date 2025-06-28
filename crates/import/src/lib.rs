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
use tokio::{
    fs,
    io::{self, AsyncReadExt, BufReader},
};
use tracing::info;

/// Pointer to a file that should be imported
#[derive(Clone, Debug, derive_more::Display)]
pub enum ImportInput {
    /// Import data from stdin
    #[display("stdin")]
    Stdin,
    /// Import from a local file
    #[display("{}", _0.display())]
    Path(PathBuf),
    /// Download a file via HTTP and import it
    #[display("{_0}")]
    Url(Url),
}

impl ImportInput {
    /// Load the contents of the import input
    async fn load(&self) -> anyhow::Result<String> {
        match self {
            Self::Stdin => {
                info!("Reading stdin for import");
                let mut reader = BufReader::new(io::stdin());
                let mut content = String::with_capacity(1024);
                reader
                    .read_to_string(&mut content)
                    .await
                    .context("Error importing from stdin")?;
                Ok(content)
            }
            Self::Path(path) => {
                info!(?path, "Reading local file for import");
                let content =
                    fs::read_to_string(path).await.with_context(|| {
                        format!(
                            "Error importing from local file `{}`",
                            path.display()
                        )
                    })?;
                Ok(content)
            }
            Self::Url(url) => {
                info!(%url, "Fetching remote file for import");
                let content = reqwest::get(url.clone())
                    .and_then(Response::text)
                    .await
                    .with_context(|| {
                        format!("Error importing from HTTP URL {url}")
                    })?;
                Ok(content)
            }
        }
    }

    /// Get the name of the input file. For a path this just grabs the
    /// file name from the path. For a URL it treats the path component of the
    /// URL as a file path and gets the file namefrom there
    fn file_name(&self) -> Option<&OsStr> {
        match self {
            Self::Stdin => None,
            Self::Path(path) => path.file_name(),
            Self::Url(url) => Path::new(url.path()).file_name(),
        }
    }
}

impl FromStr for ImportInput {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "-" {
            Ok(Self::Stdin)
        } else if let Ok(url) = s.parse::<Url>()
            // Windows paths (C:\...) parse as URLs, so we need to make sure
            // it's an HTTP URL
            && ["http", "https"].contains(&url.scheme())
        {
            Ok(Self::Url(url))
        } else {
            Ok(Self::Path(PathBuf::from(s)))
        }
    }
}
