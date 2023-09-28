use crate::template::TemplateString;
use anyhow::{anyhow, Context};
use derive_more::{Deref, From};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};
use tokio::fs;
use tracing::{event, Level};

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
    pub chains: Vec<Chain>,
}

/// Mutually exclusive hot-swappable config group
/// TODO rename to break confusion with environment variables
#[derive(Clone, Debug, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: Option<String>,
    pub data: HashMap<String, String>,
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Clone, Debug, Deserialize)]
pub struct RequestRecipe {
    pub id: RequestRecipeId,
    pub name: Option<String>,
    pub method: TemplateString,
    pub url: TemplateString,
    pub body: Option<TemplateString>,
    #[serde(default)]
    pub query: HashMap<String, TemplateString>,
    #[serde(default)]
    pub headers: HashMap<String, TemplateString>,
}

#[derive(Clone, Debug, Deref, Default, From, Serialize, Deserialize)]
pub struct RequestRecipeId(String);

/// A chain is a means to data from one response in another request. The chain
/// is the middleman: it defines where and how to pull the value, then recipes
/// can use it in a template via `{{chains.<chain_id>}}`.
#[derive(Clone, Debug, Deserialize)]
pub struct Chain {
    pub id: String,
    pub name: Option<String>,
    pub source: RequestRecipeId,
    /// JSONpath to extract a value from the response. For JSON responses only.
    pub path: Option<String>,
}

impl RequestCollection {
    /// Load config from the given file, or fall back to one of the
    /// auto-detected defaults. Return the loaded collection as well as the
    /// path of the file it was loaded from.
    pub async fn load(
        collection_file: Option<&Path>,
    ) -> anyhow::Result<(&Path, Self)> {
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
        let collection = parse.await.with_context(|| {
            format!("Error parsing config from file {path:?}")
        })?;

        Ok((path, collection))
    }

    /// Search the current directory for a config file matching one of the known
    /// file names, and return it if found
    fn detect_path<'a>() -> anyhow::Result<&'a Path> {
        let paths: Vec<&Path> = CONFIG_FILES
            .iter()
            .map(Path::new)
            // This could be async but I'm being lazy and skipping it for now,
            // since we only do this at startup anyway (mid-process reloading
            // reuses the detected path so we don't re-detect)
            .filter(|p| p.exists())
            .collect();
        match paths.as_slice() {
            [] => Err(anyhow!(
                "No config file given and none found in current directory"
            )),
            [path] => Ok(path),
            [first, rest @ ..] => {
                // Print a warning, but don't actually fail
                event!(
                    Level::WARN,
                    "Multiple config files detected. {first:?} will be used \
                    and the following will be ignored: {rest:?}"
                );
                Ok(*first)
            }
        }
    }
}

impl Environment {
    /// Get a presentable name for this environment
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

impl RequestRecipe {
    /// Get a presentable name for this recipe
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}
