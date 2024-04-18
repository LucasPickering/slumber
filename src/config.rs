use crate::{
    tui::input::{Action, InputBinding},
    util::{
        parse_yaml,
        paths::{DataDirectory, FileGuard},
        ResultExt,
    },
};
use anyhow::Context;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fs;
use tracing::info;

/// App-level configuration, which is global across all sessions and
/// collections. This is *not* meant to modifiable during a session. If changes
/// are made to the config file while a session is running, they won't be
/// picked up until the app restarts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// TLS cert errors on these hostnames are ignored. Be careful!
    #[serde(default)]
    pub ignore_certificate_hosts: Vec<String>,
    /// Should templates be rendered inline in the UI, or should we show the
    /// raw text?
    pub preview_templates: bool,

    /// Overrides for default key bindings
    pub input_bindings: IndexMap<Action, InputBinding>,
}

impl Config {
    const FILE: &'static str = "config.yml";

    /// Load configuration from the file, if present. If not, just return a
    /// default value. This only returns an error if the file could be read, but
    /// deserialization failed. This is *not* async because it's only run during
    /// startup, when all operations are synchronous.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path().create_parent()?;
        info!(?path, "Loading configuration file");

        match fs::read(&path) {
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
        }
    }

    /// Path to the configuration file
    pub fn path() -> FileGuard {
        DataDirectory::root().file(Self::FILE)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore_certificate_hosts: Vec::new(),
            preview_templates: true,
            input_bindings: IndexMap::default(),
        }
    }
}
