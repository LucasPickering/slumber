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
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod cereal;
mod input;
mod mime;
mod theme;

pub use input::{Action, InputBinding, InputMap, KeyCombination};
pub use theme::Theme;

use crate::mime::MimeMap;
use ::mime::Mime;
use anyhow::Context;
use editor_command::EditorBuilder;
use serde::Serialize;
use slumber_util::{
    ResultTracedAnyhow, doc_link, git_link,
    paths::{self, create_parent, expand_home},
    yaml,
};
use std::{
    env,
    error::Error,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{error, info};

const PATH_ENV_VAR: &str = "SLUMBER_CONFIG_PATH";
const FILE: &str = "config.yml";

/// App-level configuration, which is global across all sessions and
/// collections. This is *not* meant to modifiable during a session. If changes
/// are made to the config file while a TUI session is running, they won't be
/// picked up until the app restarts.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    feature = "schema",
    schemars(
        default,
        // Allow any top-level property beginning with .
        extend("patternProperties" = {
            "^\\.": { "description": "Ignore any property beginning with `.`" }
        }),
        example = Config::default(),
    )
)]
pub struct Config {
    /// Configuration for in-app query and export commands
    pub commands: CommandsConfig,
    /// Command to use for in-app editing. If provided, overrides
    /// `VISUAL`/`EDITOR` environment variables. This only supports a single
    /// command, *not* a content type map. This is because there isn't much
    /// value in it, and plumbing the content type around to support it is
    /// annoying.
    pub editor: Option<String>,
    /// Command to use to browse response bodies. If provided, overrides
    /// `PAGER` environment variable.  This could be a single command, or a map
    /// of {content_type: command} to use different commands based on response
    /// type. Aliased for backward compatibility with the old name.
    #[serde(alias = "viewer", default)]
    pub pager: MimeMap<String>,
    #[serde(flatten)]
    pub http: HttpEngineConfig,
    /// Should templates be rendered inline in the UI, or should we show the
    /// raw text?
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
    /// Enable/disable persistence for all TUI requests? The CLI ignores this
    /// in favor of the absence/presence of the `--persist` flag
    pub persist: bool,
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
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path();
        info!(?path, "Loading configuration file");

        match yaml::deserialize_file::<Config>(&path) {
            Ok(config) => Ok(config),
            Err(error) => {
                // Filesystem error shouldn't be fatal because it may be a
                // weird fs error the user can't or doesn't want tofix. Just use
                // a default config.
                if let yaml::YamlErrorKind::Io { error, .. } = &error.kind {
                    error!(
                        error = error as &dyn Error,
                        "Error opening config file {path:?}"
                    );

                    // If the file doesn't exist, try to create a placeholder.
                    // If this fails, silently move on since we don't actually
                    // need it
                    if error.kind() == io::ErrorKind::NotFound {
                        let _ = Self::create_new(&path).traced();
                    }

                    Ok(Self::default())
                } else {
                    // Error occurred during deserialization - the user probably
                    // wants to fix this
                    Err(error.into())
                }
            }
        }
    }

    /// Get a command to open the given file in the user's configured editor.
    /// Default editor is `vim`. Return an error if the command couldn't be
    /// built.
    pub fn editor_command(&self, file: &Path) -> anyhow::Result<Command> {
        EditorBuilder::new()
            // Config field takes priority over environment variables
            .source(self.editor.as_deref())
            .environment()
            .source(Some("vim"))
            .path(file)
            .build()
            .with_context(|| {
                format!(
                    "Error opening editor; see {}",
                    doc_link("user_guide/tui/editor"),
                )
            })
    }

    /// Get a command to open the given file in the user's configured file
    /// pager. Default is `less` on Unix, `more` on Windows. Return an error
    /// if the command couldn't be built.
    pub fn pager_command(
        &self,
        file: &Path,
        mime: Option<&Mime>,
    ) -> anyhow::Result<Command> {
        // Use a built-in pager
        let default = if cfg!(windows) { "more" } else { "less" };

        // Select command from the config based on content type
        let config_command = mime
            .and_then(|mime| self.pager.get(mime))
            .map(String::as_str);

        EditorBuilder::new()
            // Config field takes priority over environment variables
            .source(config_command)
            .source(env::var("PAGER").ok())
            .source(Some(default))
            .path(file)
            .build()
            .with_context(|| {
                format!(
                    "Error opening pager; see {}",
                    doc_link("user_guide/tui/editor"),
                )
            })
    }

    /// When the config file fails to open, we'll attempt to create a new one
    /// with placeholder content. Whether or not the
    // create succeeds, we're going to just log the error and use a
    // default config.
    fn create_new(path: &Path) -> anyhow::Result<()> {
        // You could do this read/create all in one operation using
        // OpenOptions::new().create(true).append(true).read(true),
        // but that requires write permission on the file even if it
        // doesn't exist, which may not be the case (e.g. NixOS)
        // https://github.com/LucasPickering/slumber/issues/504
        //
        // This two step approach does have the risk of a race
        // condition, but it's exceptionally unlikely and worst case
        // scenario we show an error and continue with the default
        // config
        create_parent(path)
            .and_then(|()| {
                let mut file = File::create_new(path)?;
                // Prepopulate with contents
                file.write_all(&Self::default_content())?;
                Ok(())
            })
            .context("Error creating config file {path:?}")
    }

    /// Pre-populated content for a new config file. Include all default values
    /// for discoverability, as well as a comment to enable LSP completion based
    /// on the schema
    fn default_content() -> Vec<u8> {
        // Write into a single byte buffer to minimize allocations
        let mut bytes: Vec<u8> = format!(
            "# yaml-language-server: $schema={schema}
# This config has been prepopulated with default values. For documentation, see:
# {doc}
",
            schema = git_link("schemas/config.json"),
            doc = doc_link("api/configuration/index"),
        )
        .into_bytes();
        serde_yaml::to_writer(&mut bytes, &Config::default()).unwrap();
        bytes
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            commands: CommandsConfig::default(),
            editor: Default::default(),
            pager: Default::default(),
            http: Default::default(),
            preview_templates: true,
            input_bindings: Default::default(),
            theme: Default::default(),
            debug: false,
            persist: true,
        }
    }
}

/// Configuration for the engine that handles HTTP requests
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(default))]
pub struct HttpEngineConfig {
    /// TLS cert errors on these hostnames are ignored. Be careful!
    pub ignore_certificate_hosts: Vec<String>,
    /// Request/response bodies over this size are treated differently, for
    /// performance reasons
    pub large_body_size: usize,
    /// Follow 3xx redirects automatically. Enabled by default
    pub follow_redirects: bool,
}

impl HttpEngineConfig {
    /// Is the given size (e.g. request or response body size) larger than the
    /// configured "large" body size? Large bodies are treated differently, for
    /// performance reasons.
    pub fn is_large(&self, size: usize) -> bool {
        size > self.large_body_size
    }
}

impl Default for HttpEngineConfig {
    fn default() -> Self {
        Self {
            ignore_certificate_hosts: Default::default(),
            large_body_size: 1000 * 1000, // 1MB
            follow_redirects: true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use env_lock::EnvGuard;
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use slumber_util::{TempDir, assert_err, temp_dir};
    use std::fs;

    struct ConfigPath {
        path: PathBuf,
        dir: TempDir,
        /// Guard on [PATH_ENV_VAR], so multiple tests can't modify it at once
        _guard: EnvGuard<'static>,
    }

    /// Create a temp dir, get a path to a config file from it, and set
    /// [PATH_ENV_VAR] to point to that file
    #[fixture]
    fn config_path(temp_dir: TempDir) -> ConfigPath {
        let path = temp_dir.join("config.yml");
        let guard =
            env_lock::lock_env([(PATH_ENV_VAR, Some(path.to_str().unwrap()))]);
        ConfigPath {
            path,
            dir: temp_dir,
            _guard: guard,
        }
    }

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

    /// File exists but it's empty. The default deserialized value should match
    /// `Config::default()`
    #[rstest]
    fn test_load_file_empty(config_path: ConfigPath) {
        fs::write(&config_path.path, "").unwrap();

        let config = Config::load().unwrap();
        assert_eq!(config, Config::default());
    }

    /// We can load the config when the config file already exists but is
    /// readonly
    #[rstest]
    fn test_load_file_exists_readonly(config_path: ConfigPath) {
        fs::write(&config_path.path, "debug: true\n").unwrap();
        let mut permissions =
            fs::metadata(&config_path.path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&config_path.path, permissions).unwrap();

        let config = Config::load().unwrap();
        assert_eq!(
            config,
            Config {
                debug: true,
                ..Config::default()
            }
        );
    }

    /// If the config file doesn't already exist, we'll create it
    #[rstest]
    fn test_load_file_does_not_exist_can_create(config_path: ConfigPath) {
        // Ensure file does not exist
        assert!(!config_path.path.exists());

        // Should be default values
        let config = Config::load().unwrap();
        assert_eq!(config, Config::default());

        // File should now exist
        assert!(config_path.path.exists());

        // Should contain default values
        let config = Config::load().unwrap();
        assert_eq!(config, Config::default());
    }

    /// If the config file doesn't already exist, we'll attempt to create it.
    /// If we don't have permission to create it, use the default
    #[rstest]
    // Directory permissions are funky in windows and I don't feel like figuring
    // it out
    #[cfg(unix)]
    fn test_load_file_does_not_exist_cannot_create(config_path: ConfigPath) {
        let mut permissions =
            fs::metadata(&*config_path.dir).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&*config_path.dir, permissions).unwrap();

        // Should be default values
        let config = Config::load().unwrap();
        assert_eq!(config, Config::default());

        // File still does not exist
        assert!(!config_path.path.exists());
    }

    /// Loading a config file with contents that don't deserialize correctly
    /// returns an error
    #[rstest]
    fn test_load_file_invalid(config_path: ConfigPath) {
        fs::write(&config_path.path, "fake_field: true\n").unwrap();
        assert_err!(Config::load(), "Unexpected field `fake_field`");
    }
}
