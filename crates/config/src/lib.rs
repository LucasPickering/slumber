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
    util::{
        parse_yaml,
        paths::{self, create_parent, expand_home},
        ResultTraced,
    },
};
use std::{env, fs::OpenOptions, path::PathBuf};
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
    /// Configuration for in-app query and side effect commands
    pub commands: CommandsConfig,
    /// Command to use for in-app editing. If provided, overrides
    /// `VISUAL`/`EDITOR` environment variables
    pub editor: Option<String>,
    /// Command to use to browse response bodies. Aliased for backward
    /// compatibility with the old name
    #[serde(alias = "viewer")]
    pub pager: Option<String>,
    #[serde(flatten)]
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
    /// Path to the configuration file, in this precedence:
    /// - Value of `$SLUMBER_CONFIG_PATH`
    /// - `$DATA_DIR/slumber/config.yml` **if the file exists**, where
    ///   `$DATA_DIR` is defined by [dirs::data_dir]. This is a legacy location,
    ///   supported for backward compatibility only. See this issue for more:
    ///   https://github.com/LucasPickering/slumber/issues/371
    /// - `$CONFIG_DIR/slumber/config.yml`, where `$CONFIG_DIR` is defined by
    ///   [dirs::config_dir]
    pub fn path() -> PathBuf {
        if let Ok(path) = env::var(PATH_ENV_VAR) {
            return expand_home(PathBuf::from(path)).into_owned();
        }

        let legacy_path = paths::data_directory().join(FILE);
        if legacy_path.is_file() {
            return legacy_path;
        }

        paths::config_directory().join(FILE)
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
        create_parent(&path)?;

        info!(?path, "Loading configuration file");

        // Open the config file, creating it if it doesn't exist. This will
        // never create the legacy file, because the file must already exist in
        // order for the legacy location to be used.
        (|| {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .read(true)
                .open(&path)?;
            let config = parse_yaml::<Self>(&file)?;
            Ok::<_, anyhow::Error>(config)
        })()
        .context(format!("Error loading configuration from {path:?}"))
        .traced()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            commands: CommandsConfig::default(),
            editor: None,
            pager: None,
            http: HttpEngineConfig::default(),
            preview_templates: true,
            input_bindings: Default::default(),
            theme: Default::default(),
            debug: false,
        }
    }
}

/// Configuration for in-app query and side effect commands
#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandsConfig {
    /// Wrapping shell to parse and execute commands
    /// If empty, commands will be parsed with shell-words and run natievly
    pub shell: Vec<String>,
    /// Default query command for responses
    pub query_default: Option<String>,
}

impl Default for CommandsConfig {
    fn default() -> Self {
        // We use the defaults from docker, because it's well tested and
        // reasonably intuitive
        // https://docs.docker.com/reference/dockerfile/#shell
        let default_shell: &[&str] = if cfg!(windows) {
            &["cmd", "/S", "/C"]
        } else {
            &["/bin/sh", "-c"]
        };

        Self {
            shell: default_shell.iter().map(|s| s.to_string()).collect(),
            query_default: None,
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
