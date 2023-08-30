use crate::template::TemplateString;
use anyhow::{anyhow, Context};
use log::warn;
use serde::Deserialize;
use std::{collections::HashMap, path::Path};
use tokio::fs;

/// The support file names to be automatically loaded as a config. We only
/// support loading from one file at a time, so if more than one of these is
/// defined, we'll take the earliest and print a warning.
pub const CONFIG_FILES: &[&str] = &[
    "slumber.yml",
    "slumber.yaml",
    ".slumber.yml",
    ".slumber.yaml",
];

/// A collection of requests
#[derive(Clone, Debug, Deserialize)]
pub struct RequestCollection {
    #[serde(default)]
    pub environments: Vec<Environment>,
    #[serde(default)]
    pub requests: Vec<RequestRecipe>,
}

/// Mutually exclusive hot-swappable config group
#[derive(Clone, Debug, Deserialize)]
pub struct Environment {
    pub name: String,
    pub data: HashMap<String, String>,
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Clone, Debug, Deserialize)]
pub struct RequestRecipe {
    pub id: String,
    /// No reason for this to be a template, because it only appears in the UI
    pub name: String,
    pub method: TemplateString,
    pub url: TemplateString,
    pub body: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, TemplateString>,
}

impl RequestCollection {
    /// Load config from the given file, or fall back to one of the
    /// auto-detected defaults
    pub async fn load(collection_file: Option<&Path>) -> anyhow::Result<Self> {
        // Figure out which file we want to load from
        let path = collection_file.map_or_else(Self::detect_path, Ok)?;

        // First, parse the file to raw YAML values, so we can apply
        // anchor/alias merging. Then parse that to our config type
        let parse = async {
            let content = fs::read(path).await?;
            let mut yaml_value =
                serde_yaml::from_slice::<serde_yaml::Value>(&content)?;
            yaml_value.apply_merge()?;
            Ok::<RequestCollection, anyhow::Error>(serde_yaml::from_value(
                yaml_value,
            )?)
        };
        parse
            .await
            .with_context(|| format!("Error parsing config from file {path:?}"))
    }

    /// Search the current directory for a config file matching one of the known
    /// file names, and return it if found
    fn detect_path<'a>() -> anyhow::Result<&'a Path> {
        let paths: Vec<&Path> = CONFIG_FILES
            .iter()
            .map(Path::new)
            // This could be async but I'm being lazy and skipping it for now,
            // since we only do this at startup anyway
            .filter(|p| p.exists())
            .collect();
        match paths.as_slice() {
            [] => Err(anyhow!(
                "No config file given and none found in current directory"
            )),
            [path] => Ok(path),
            [first, rest @ ..] => {
                // Print a warning, but don't actually fail
                warn!(
                    "Multiple config files detected. {first:?} will be used \
                    and the following will be ignored: {rest:?}"
                );
                Ok(*first)
            }
        }
    }
}
