//! A request collection defines recipes, profiles, etc. that make requests
//! possible

mod cereal;
mod insomnia;

use crate::template::Template;
use anyhow::{anyhow, Context};
use derive_more::{Deref, Display, From};
use equivalent::Equivalent;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json_path::JsonPath;
use std::{
    fmt::Debug,
    future::Future,
    hash::Hash,
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RequestCollection<S = PathBuf> {
    /// The source of the collection, typically a path to the file it was
    /// loaded from
    #[serde(skip)]
    source: S,

    /// Unique ID for this collection. This should be unique for across all
    /// collections used on one computer.
    pub id: CollectionId,
    #[serde(default, deserialize_with = "cereal::deserialize_id_map")]
    pub profiles: IndexMap<ProfileId, Profile>,
    #[serde(default, deserialize_with = "cereal::deserialize_id_map")]
    pub chains: IndexMap<ChainId, Chain>,
    /// Internally we call these recipes, but to a user `requests` is more
    /// intuitive
    #[serde(
        default,
        rename = "requests",
        deserialize_with = "cereal::deserialize_id_map"
    )]
    pub recipes: IndexMap<RequestRecipeId, RequestRecipe>,
}

/// A unique ID for a collection. This is necessary to differentiate between
/// responses from different collections in the repository.
#[derive(Clone, Debug, Default, Display, From, Serialize, Deserialize)]
pub struct CollectionId(String);

/// Mutually exclusive hot-swappable config group
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: ProfileId,
    pub name: Option<String>,
    pub data: IndexMap<String, ProfileValue>,
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

/// The value type of a profile's data mapping
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum ProfileValue {
    /// A raw text string
    Raw(String),
    /// A nested template, which allows for recursion. By requiring the user to
    /// declare this up front, we can parse the template during collection
    /// deserialization. It also keeps a cap on the complexity of nested
    /// templates, which is a balance between usability and simplicity (both
    /// for the user and the code).
    Template(Template),
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestRecipe {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RequestRecipeId,
    pub name: Option<String>,
    /// *Not* a template string because the usefulness doesn't justify the
    /// complexity
    pub method: String,
    pub url: Template,
    pub body: Option<Template>,
    #[serde(default)]
    pub query: IndexMap<String, Template>,
    #[serde(default)]
    pub headers: IndexMap<String, Template>,
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
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: ChainId,
    pub source: ChainSource,
    /// Mask chained value in the UI
    #[serde(default)]
    pub sensitive: bool,
    /// JSONpath to extract a value from the response. For JSON data only.
    pub selector: Option<JsonPath>,
}

/// Unique ID for a chain. Takes a generic param so we can create these during
/// templating without having to clone the underlying string.
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
pub struct ChainId<S = String>(S);

impl From<&str> for ChainId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl From<&ChainId<&str>> for ChainId {
    fn from(value: &ChainId<&str>) -> Self {
        Self(value.0.into())
    }
}

/// Allow looking up by ChainId<&tr> in a map
impl Equivalent<ChainId> for ChainId<&str> {
    fn equivalent(&self, key: &ChainId) -> bool {
        self.0 == key.0
    }
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

impl<S> RequestCollection<S> {
    /// Replace the source value on this collection
    pub fn with_source<T>(self, source: T) -> RequestCollection<T> {
        RequestCollection {
            source,
            id: self.id,
            profiles: self.profiles,
            chains: self.chains,
            recipes: self.recipes,
        }
    }
}

impl RequestCollection<PathBuf> {
    /// Load config from the given file. The caller is responsible for using
    /// [Self::detect_path] to find the file themself. This pattern enables the
    /// TUI to start up and watch the collection file, even if it's invalid.
    pub async fn load(path: PathBuf) -> anyhow::Result<Self> {
        // Figure out which file we want to load from
        info!(?path, "Loading collection file");

        // First, parse the file to raw YAML values, so we can apply
        // anchor/alias merging. Then parse that to our config type
        let future = async {
            let content = fs::read(&path).await?;
            let mut yaml_value =
                serde_yaml::from_slice::<serde_yaml::Value>(&content)?;
            yaml_value.apply_merge()?;
            Ok::<RequestCollection, anyhow::Error>(serde_yaml::from_value(
                yaml_value,
            )?)
        };

        Ok(future
            .await
            .context(format!("Error loading collection from {path:?}"))?
            .with_source(path))
    }

    /// Reload a new collection from the same file used for this one.
    ///
    /// Returns `impl Future` to unlink the future from `&self`'s lifetime.
    pub fn reload(&self) -> impl Future<Output = anyhow::Result<Self>> {
        Self::load(self.source.clone())
    }

    /// Get the path of the file that this collection was loaded from
    pub fn path(&self) -> &Path {
        &self.source
    }

    /// Get the path to the collection file, returning an error if none is
    /// available. This will use the override if given, otherwise it will fall
    /// back to searching the current directory for a collection.
    pub fn try_path(override_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
        override_path
            .or_else(RequestCollection::detect_path)
            .ok_or(anyhow!(
                "No collection file given and none found in current directory"
            ))
    }

    /// Search the current directory for a config file matching one of the known
    /// file names, and return it if found
    fn detect_path() -> Option<PathBuf> {
        let paths: Vec<&Path> = CONFIG_FILES
            .iter()
            .map(Path::new)
            // This could be async but I'm being lazy and skipping it for now,
            // since we only do this at startup anyway (mid-process reloading
            // reuses the detected path so we don't re-detect)
            .filter(|p| p.exists())
            .collect();
        match paths.as_slice() {
            [] => None,
            [path] => Some(path.to_path_buf()),
            [first, rest @ ..] => {
                // Print a warning, but don't actually fail
                warn!(
                    "Multiple config files detected. {first:?} will be used \
                    and the following will be ignored: {rest:?}"
                );
                Some(first.to_path_buf())
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

impl From<String> for ProfileValue {
    fn from(value: String) -> Self {
        Self::Raw(value)
    }
}

impl From<&str> for ProfileValue {
    fn from(value: &str) -> Self {
        Self::Raw(value.into())
    }
}

impl RequestRecipe {
    /// Get a presentable name for this recipe
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}
