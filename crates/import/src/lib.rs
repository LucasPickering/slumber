//! Import from external formats into Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod common;
mod insomnia;
mod openapi;
mod rest;

use anyhow::Context;
pub use common::Collection;
pub use insomnia::from_insomnia;
pub use openapi::from_openapi;
pub use rest::from_rest;
use slumber_util::parse_yaml;
use std::{fs::File, path::Path};
use tracing::info;

/// Convert a legacy Slumber YAML collection into the common import format
pub fn from_yaml(yaml_file: impl AsRef<Path>) -> anyhow::Result<Collection> {
    let yaml_file = yaml_file.as_ref();
    info!(file = ?yaml_file, "Loading Slumber YAML collection");
    let file = File::open(yaml_file).context(format!(
        "Error opening Slumber YAML collection file {yaml_file:?}"
    ))?;
    // Since this is our own format, we're very strict about the import. If it
    // fails, that should be a fatal bug
    parse_yaml(file).context(format!(
        "Error deserializing Slumber YAML collection file {yaml_file:?}",
    ))
}
