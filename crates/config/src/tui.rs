//! TUI-specific configuration

mod cereal;
mod input;
mod mime;
mod theme;

pub use cereal::deserialize_tui_config;
pub use input::{Action, InputBinding, InputMap, KeyCombination};
pub use mime::MimeOverrideMap;
pub use theme::{Color, Syntax, Theme};

use crate::{Config, EditorError, tui::mime::MimeMap};
use ::mime::Mime;
use editor_command::Editor;
use serde::Serialize;
use std::env;

/// Configuration specific to the TUI
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(default))]
pub struct TuiConfig {
    /// Configuration for in-app query and export commands
    pub commands: CommandsConfig,

    /// Override mapping for MIME types
    ///
    /// This mapping is applied before any other MIME-based operations. It
    /// allows you to dynamically replace a response's reported `Content-Type`.
    /// It's useful when the server uses the wrong MIME.
    pub mime_overrides: MimeOverrideMap,

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
            mime_overrides: Default::default(),
            commands: CommandsConfig::default(),
            pager: Default::default(),
            preview_templates: true,
            input_bindings: Default::default(),
            theme: Default::default(),
            debug: false,
            persist: true,
        }
    }
}

// Extension for the root config for TUI-specific methods
impl Config {
    /// Get an [Editor] to open the given file in the user's configured file
    /// pager. Default is `less` on Unix, `more` on Windows. Return an error
    /// if the command couldn't be built.
    pub fn pager(&self, mime: Option<&Mime>) -> Result<Editor, EditorError> {
        // Use a built-in pager
        let default = if cfg!(windows) { "more" } else { "less" };

        // Select command from the config based on content type
        let config_command = mime
            .and_then(|mime| self.tui.pager.get(&self.tui.mime_overrides, mime))
            .map(String::as_str);

        editor_command::EditorBuilder::new()
            // Config field takes priority over environment variables
            .string(config_command)
            .string(env::var("PAGER").ok())
            .string(Some(default))
            .build()
            .map_err(EditorError)
    }

    /// Get the default query command for a response body based on its MIME type
    pub fn default_query(&self, mime: Option<&Mime>) -> Option<&str> {
        mime.and_then(|mime| {
            self.tui
                .commands
                .default_query
                .get(&self.tui.mime_overrides, mime)
        })
        .map(String::as_str)
    }

    /// Get the MIME override map
    pub fn mime_overrides(&self) -> &MimeOverrideMap {
        &self.tui.mime_overrides
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
