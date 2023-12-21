use crate::util::{parse_yaml, Directory, ResultExt};
use anyhow::Context;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::info;

/// App-level configuration, which is global across all sessions and
/// collections. This is *not* meant to modifiable during a session. If changes
/// are made to the config file while a session is running, they won't be
/// picked up until the app restarts.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The path that the config was loaded from, or tried to be loaded from if
    /// the file didn't exist
    #[serde(skip)]
    path: PathBuf,
    /// Should templates be rendered inline in the UI, or should we show the
    /// raw text?
    pub preview_templates: bool,
}

impl Config {
    const FILE: &'static str = "config.yml";

    /// Load configuration from the file, if present. If not, just return a
    /// default value. This only returns an error if the file could be read, but
    /// deserialization failed. This is *not* async because it's only run during
    /// startup, when all operations are synchronous.
    pub fn load() -> anyhow::Result<Self> {
        let path = Directory::root().create()?.join(Self::FILE);
        info!(?path, "Loading configuration file");

        let mut config = match fs::read(&path) {
            Ok(bytes) => parse_yaml::<Self>(&bytes)
                .context(format!("Error loading configuration from {path:?}"))
                .traced(),
            // An error here is probably just the file missing, so don't make
            // a big stink about it
            Err(error) => {
                info!(
                    ?path,
                    error = &error as &dyn std::error::Error,
                    "Error reading configuration file"
                );
                Ok(Self::default())
            }
        }?;

        config.path = path;
        Ok(config)
    }

    /// The path where configuration is stored
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: PathBuf::default(),
            preview_templates: true,
        }
    }
}
