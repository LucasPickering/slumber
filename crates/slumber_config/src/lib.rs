//! App configuration. Some config fields apply to core functionality, while
//! some are interface-specific. While it's maybe not the "best" design, we
//! compile them all into one crate to give consistent behavior between the
//! CLI and TUI. Specifically, it allows the `slumber show config` command to
//! show exactly what the TUI is actually using.
//!
//! The downside of this is we have to pull in some types that are specific to
//! the TUI, because they relate to configuration. By putting this in a separate
//! crate, instead of the core crate, it at least pushes those dependencies down
//! the compile chain a bit further.

mod input;
mod theme;

pub use input::{Action, InputBinding, KeyCombination};
pub use theme::Theme;

use anyhow::Context;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use slumber_core::util::{parse_yaml, DataDirectory, ResultTraced};
use std::{fs, path::PathBuf};
use tracing::info;

const FILE: &str = "config.yml";

/// App-level configuration, which is global across all sessions and
/// collections. This is *not* meant to modifiable during a session. If changes
/// are made to the config file while a TUI session is running, they won't be
/// picked up until the app restarts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// TLS cert errors on these hostnames are ignored. Be careful!
    pub ignore_certificate_hosts: Vec<String>,
    /// Should templates be rendered inline in the UI, or should we show the
    /// raw text?
    pub preview_templates: bool,
    /// Overrides for default key bindings
    pub input_bindings: IndexMap<Action, InputBinding>,
    /// Visual configuration for the TUI (e.g. colors)
    pub theme: Theme,
}

impl Config {
    /// Path to the configuration file
    pub fn path() -> PathBuf {
        DataDirectory::get().file(FILE)
    }

    /// Load configuration from the file, if present. If not, just return a
    /// default value. This only returns an error if the file could be read, but
    /// deserialization failed. This is *not* async because it's only run during
    /// startup, when all operations are synchronous.
    ///
    /// Configuration type is dynamic so that different consumers (CLI vs TUI)
    /// can specify their own types
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path();
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore_certificate_hosts: Default::default(),
            preview_templates: true,
            input_bindings: Default::default(),
            theme: Default::default(),
        }
    }
}
