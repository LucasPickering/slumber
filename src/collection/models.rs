//! The plain data types that make up a request collection

use crate::{collection::cereal, template::Template};
use derive_more::{Deref, Display, From};
use equivalent::Equivalent;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json_path::JsonPath;
use std::path::PathBuf;

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Collection {
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
    pub recipes: IndexMap<RecipeId, Recipe>,
}

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

/// Needed for persistence loading
impl PartialEq<Profile> for ProfileId {
    fn eq(&self, other: &Profile) -> bool {
        self == &other.id
    }
}

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
pub struct Recipe {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
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
pub struct RecipeId(String);

/// Needed for persistence loading
impl PartialEq<Recipe> for RecipeId {
    fn eq(&self, other: &Recipe) -> bool {
        self == &other.id
    }
}

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
    Request(RecipeId),
    /// Run an external command to get a result
    Command(Vec<String>),
    /// Load data from a file
    File(PathBuf),
    /// Prompt the user for a value, with an optional label
    Prompt(Option<String>),
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

impl Recipe {
    /// Get a presentable name for this recipe
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}
