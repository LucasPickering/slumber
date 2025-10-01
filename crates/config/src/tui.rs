//! TUI-specific configuration

mod input;
mod mime;
mod theme;

pub use input::{Action, InputBinding, InputMap, KeyCombination};
pub use theme::Theme;

use crate::tui::mime::MimeMap;
use serde::Serialize;

/// Configuration specific to the TUI
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(default))]
pub struct TuiConfig {
    /// Configuration for in-app query and export commands
    pub commands: CommandsConfig,

    /// Command to use for in-app editing. If provided, overrides
    /// `VISUAL`/`EDITOR` environment variables. This only supports a
    /// single command, *not* a content type map. This is because
    /// there isn't much value in it, and plumbing the content type
    /// around to support it is annoying.
    pub editor: Option<String>,

    /// Command to use to browse response bodies. If provided, overrides
    /// `PAGER` environment variable.  This could be a single command, or a
    /// map of {content_type: command} to use different commands
    /// based on response type. Aliased for backward compatibility
    /// with the old name.
    #[serde(alias = "viewer", default)]
    pub pager: MimeMap<String>,

    /// Should templates be rendered inline in the UI, or should we show
    /// the raw text?
    pub preview_templates: bool,

    /// Overrides for default key bindings
    pub input_bindings: InputMap,

    /// Visual configuration for the TUI (e.g. colors)
    pub theme: Theme,

    /// Enable debug monitor in TUI
    ///
    /// Mainly meant for development so don't expose it
    #[serde(skip_serializing)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub debug: bool,

    /// Enable/disable persistence for all TUI requests? The CLI ignores
    /// this in favor of the absence/presence of the `--persist`
    /// flag
    pub persist: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            commands: CommandsConfig::default(),
            editor: Default::default(),
            pager: Default::default(),
            preview_templates: true,
            input_bindings: Default::default(),
            theme: Default::default(),
            debug: false,
            persist: true,
        }
    }
}

/// Configuration for in-app query and export commands
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(default))]
pub struct CommandsConfig {
    /// Wrapping shell to parse and execute commands
    /// If empty, commands will be parsed with shell-words and run natievly
    pub shell: Vec<String>,
    /// Default query command for responses
    #[serde(default)]
    pub default_query: MimeMap<String>,
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
            shell: default_shell.iter().map(ToString::to_string).collect(),
            default_query: MimeMap::default(),
        }
    }
}
