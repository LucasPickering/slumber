mod insomnia;

use crate::template::TemplateString;
use anyhow::{anyhow, Context};
use derive_more::{Deref, Display, From};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json_path::JsonPath;
use std::{
    future::Future,
    path::{Path, PathBuf},
};
use tokio::fs;
use tracing::{info, warn};

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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestCollection<S = PathBuf> {
    /// The source of the collection, typically a path to the file it was
    /// loaded from
    #[serde(skip)]
    source: S,

    /// Unique ID for this collection. This should be unique for across all
    /// collections used on one computer.
    pub id: CollectionId,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub chains: Vec<Chain>,
    /// Internally we call these recipes, but to a user `requests` is more
    /// intuitive
    #[serde(default, rename = "requests")]
    pub recipes: Vec<RequestRecipe>,
}

/// A unique ID for a collection. This is necessary to differentiate between
/// responses from different collections in the repository.
#[derive(Clone, Debug, Display, From, Serialize, Deserialize)]
pub struct CollectionId(String);

/// Mutually exclusive hot-swappable config group
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: ProfileId,
    pub name: Option<String>,
    pub data: IndexMap<String, String>,
}

#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    Display,
    Eq,
    From,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct ProfileId(String);

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestRecipe {
    pub id: RequestRecipeId,
    pub name: Option<String>,
    /// *Not* a template string because the usefulness doesn't justify the
    /// complexity
    pub method: String,
    pub url: TemplateString,
    pub body: Option<TemplateString>,
    #[serde(default)]
    pub query: IndexMap<String, TemplateString>,
    #[serde(default)]
    pub headers: IndexMap<String, TemplateString>,
}

#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    Display,
    Eq,
    From,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct RequestRecipeId(String);

/// A chain is a means to data from one response in another request. The chain
/// is the middleman: it defines where and how to pull the value, then recipes
/// can use it in a template via `{{chains.<chain_id>}}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chain {
    pub id: String,
    pub source: ChainSource,
    /// Mask chained value in the UI
    #[serde(default)]
    pub sensitive: bool,
    /// JSONpath to extract a value from the response. For JSON data only.
    pub selector: Option<JsonPath>,
}

/// The source of data for a chain
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainSource {
    /// Load data from the most recent response of a particular request recipe
    Request(RequestRecipeId),
    /// Run an external command to get a result
    Command(Vec<String>),
    /// Load data from a file
    File(PathBuf),
    /// Prompt the user for a value, with an optional label
    Prompt(Option<String>),
}

impl RequestCollection<PathBuf> {
    /// Load config from the given file, or fall back to one of the
    /// auto-detected defaults.
    pub async fn load(
        collection_file: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        // Figure out which file we want to load from
        let path = collection_file.map_or_else(Self::detect_path, Ok)?;
        info!(?path, "Loading collection file");

        // First, parse the file to raw YAML values, so we can apply
        // anchor/alias merging. Then parse that to our config type
        let parse = async {
            let content = fs::read(&path).await?;
            let mut yaml_value =
                serde_yaml::from_slice::<serde_yaml::Value>(&content)?;
            yaml_value.apply_merge()?;
            Ok::<RequestCollection, anyhow::Error>(serde_yaml::from_value(
                yaml_value,
            )?)
        };
        let mut collection = parse.await.with_context(|| {
            format!("Error parsing config from file {path:?}")
        })?;
        collection.source = path;

        Ok(collection)
    }

    /// Reload a new collection from the same file used for this one.
    ///
    /// Returns `impl Future` to unlink the future from `&self`'s lifetime.
    pub fn reload(&self) -> impl Future<Output = anyhow::Result<Self>> {
        Self::load(Some(self.source.clone()))
    }

    /// Get the path of the file that this collection was loaded from
    pub fn path(&self) -> &Path {
        &self.source
    }

    /// Search the current directory for a config file matching one of the known
    /// file names, and return it if found
    fn detect_path() -> anyhow::Result<PathBuf> {
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
            [path] => Ok(path.to_path_buf()),
            [first, rest @ ..] => {
                // Print a warning, but don't actually fail
                warn!(
                    "Multiple config files detected. {first:?} will be used \
                    and the following will be ignored: {rest:?}"
                );
                Ok(first.to_path_buf())
            }
        }
    }
}

impl Profile {
    /// Get a presentable name for this profile
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
