use crate::util::{parse_yaml, DataDirectory, ResultTraced};
use anyhow::Context;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tracing::info;

const FILE: &str = "config.yml";

/// App-level configuration, which is global across all sessions and
/// collections. This is *not* meant to modifiable during a session. If changes
/// are made to the config file while a session is running, they won't be
/// picked up until the app restarts.
///
/// This only contains config fields relevant to core functionality. For
/// interface-specific fields, define an extension and include this as a field
/// with `#[serde(flatten)]`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// TLS cert errors on these hostnames are ignored. Be careful!
    pub ignore_certificate_hosts: Vec<String>,
}

impl Config {
    /// Load config from the file
    pub fn load() -> anyhow::Result<Self> {
        load::<Self>()
    }
}

/// Path to the configuration file
pub fn path() -> PathBuf {
    DataDirectory::get().file(FILE)
}

/// Load configuration from the file, if present. If not, just return a
/// default value. This only returns an error if the file could be read, but
/// deserialization failed. This is *not* async because it's only run during
/// startup, when all operations are synchronous.
///
/// Configuration type is dynamic so that different consumers (CLI vs TUI) can
/// specify their own types
pub fn load<C: Default + DeserializeOwned>() -> anyhow::Result<C> {
    let path = path();
    info!(?path, "Loading configuration file");

    match fs::read(&path) {
        Ok(bytes) => parse_yaml::<C>(&bytes)
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
            Ok(C::default())
        }
    }
}
