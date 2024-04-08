//! The plain data types that make up a request collection

use crate::{
    collection::cereal,
    http::{ContentType, Query},
    template::Template,
};
use derive_more::{Deref, Display, From};
use equivalent::Equivalent;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
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
#[cfg_attr(test, derive(PartialEq))]
pub struct Profile {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: ProfileId,
    pub name: Option<String>,
    pub data: IndexMap<String, Template>,
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

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Recipe {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
    pub name: Option<String>,
    /// *Not* a template string because the usefulness doesn't justify the
    /// complexity
    pub method: String,
    pub url: Template,
    pub body: Option<Template>,
    pub authentication: Option<Authentication>,
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

/// Shortcut for defining authentication method. If this is defined in addition
/// to the `Authorization` header, that header will end up being included in the
/// request twice.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum Authentication {
    /// `Authorization: Basic {username:password | base64}`
    Basic {
        username: Template,
        password: Option<Template>,
    },
    /// `Authorization: Bearer {token}`
    Bearer(Template),
}

/// A chain is a means to data from one response in another request. The chain
/// is the middleman: it defines where and how to pull the value, then recipes
/// can use it in a template via `{{chains.<chain_id>}}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Chain {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: ChainId,
    pub source: ChainSource,
    /// Mask chained value in the UI
    #[serde(default)]
    pub sensitive: bool,
    /// Selector to extract a value from the response. This uses JSONPath
    /// regardless of the content type. Non-JSON values will be converted to
    /// JSON, then converted back.
    pub selector: Option<Query>,
    /// Hard-code the content type of the response. Only needed if a selector
    /// is given and the content type can't be dynamically determined
    /// correctly. This is needed if the chain source is not an HTTP
    /// response (e.g. a file) **or** if the response's `Content-Type` header
    /// is incorrect.
    pub content_type: Option<ContentType>,
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

/// Allow looking up by ChainId<&str> in a map
impl Equivalent<ChainId> for ChainId<&str> {
    fn equivalent(&self, key: &ChainId) -> bool {
        self.0 == key.0
    }
}

/// The source of data for a chain
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum ChainSource {
    /// Load data from the most recent response of a particular request recipe
    Request {
        recipe: RecipeId,
        /// When should this request be automatically re-executed?
        #[serde(default)]
        trigger: ChainRequestTrigger,
    },
    /// Run an external command to get a result
    Command { command: Vec<String> },
    /// Load data from a file
    File { path: PathBuf },
    /// Prompt the user for a value, with an optional label
    Prompt { message: Option<String> },
}

/// Define when a recipe with a chained request should auto-execute the
/// dependency request.
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum ChainRequestTrigger {
    /// Never trigger the request. This is the default because upstream
    /// requests could be mutating, so we want the user to explicitly opt into
    /// automatic execution.
    #[default]
    Never,
    /// Trigger the request if there is none in history
    NoHistory,
    /// Trigger the request if the last response is older than some
    /// duration (or there is none in history)
    Expire(#[serde(with = "cereal::serde_duration")] Duration),
    /// Trigger the request every time the dependent request is rendered
    Always,
}

impl Profile {
    /// Get a presentable name for this profile
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

impl Recipe {
    /// Get a presentable name for this recipe
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}
