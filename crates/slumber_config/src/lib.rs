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
use slumber_core::{
    http::HttpEngineConfig,
    util::{expand_home, parse_yaml, DataDirectory, ResultTraced},
};
use std::{env, fs::File, path::PathBuf};
use tracing::info;

const PATH_ENV_VAR: &str = "SLUMBER_CONFIG_PATH";
const FILE: &str = "config.yml";

/// App-level configuration, which is global across all sessions and
/// collections. This is *not* meant to modifiable during a session. If changes
/// are made to the config file while a TUI session is running, they won't be
/// picked up until the app restarts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Command to use for in-app editing. If provided, overrides
    /// `VISUAL`/`EDITOR` environment variables
    pub editor: Option<String>,
    pub http: HttpEngineConfig,
    /// Should templates be rendered inline in the UI, or should we show the
    /// raw text?
    pub preview_templates: bool,
    /// Overrides for default key bindings
    pub input_bindings: IndexMap<Action, InputBinding>,
    /// Visual configuration for the TUI (e.g. colors)
    pub theme: Theme,
    /// Enable debug monitor in TUI
    pub debug: bool,
}

impl Config {
    /// Path to the configuration file
    pub fn path() -> PathBuf {
        env::var(PATH_ENV_VAR)
            .map(|path| expand_home(PathBuf::from(path)).into_owned())
            .unwrap_or_else(|_| DataDirectory::get().file(FILE))
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

        match File::open(&path) {
            Ok(file) => parse_yaml::<Self>(&file)
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
            editor: None,
            http: HttpEngineConfig::default(),
            preview_templates: true,
            input_bindings: Default::default(),
            theme: Default::default(),
            debug: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_config_path() {
        let _guard = env_lock::lock_env([(
            PATH_ENV_VAR,
            Some("~/dotfiles/slumber.yml"),
        )]);
        // Note: tilde is NOT expanded here; we expect the shell to do that
        assert_eq!(
            Config::path(),
            dirs::home_dir().unwrap().join("dotfiles/slumber.yml")
        );
    }
}
