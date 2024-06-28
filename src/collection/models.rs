//! The plain data types that make up a request collection

use crate::{
    collection::{
        cereal,
        recipe_tree::{RecipeNode, RecipeTree},
    },
    http::{ContentType, Query},
    template::{Identifier, Template},
};
use anyhow::anyhow;
use derive_more::{Deref, Display, From, FromStr};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use strum::{EnumIter, IntoEnumIterator};

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(deny_unknown_fields)]
pub struct Collection {
    #[serde(default, deserialize_with = "cereal::deserialize_id_map")]
    pub profiles: IndexMap<ProfileId, Profile>,
    #[serde(default, deserialize_with = "cereal::deserialize_id_map")]
    pub chains: IndexMap<ChainId, Chain>,
    /// Internally we call these recipes, but to a user `requests` is more
    /// intuitive
    #[serde(default, rename = "requests")]
    pub recipes: RecipeTree,
    /// A hack-ish to allow users to add arbitrary data to their collection
    /// file without triggering a unknown field error. Ideally we could
    /// ignore anything that starts with `.` (recursively) but that
    /// requires a custom serde impl for each type, or changes to the macro
    #[serde(default, skip_serializing, rename = ".ignore")]
    pub _ignore: serde::de::IgnoredAny,
}

/// Mutually exclusive hot-swappable config group
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: ProfileId,
    pub name: Option<String>,
    pub data: IndexMap<String, Template>,
}

impl Profile {
    /// Get a presentable name for this profile
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[cfg(test)]
impl crate::test_util::Factory for Profile {
    fn factory(_: ()) -> Self {
        Self {
            id: "profile1".into(),
            name: None,
            data: IndexMap::new(),
        }
    }
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

#[cfg(test)]
impl From<&str> for ProfileId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

#[cfg(test)]
impl crate::test_util::Factory for ProfileId {
    fn factory(_: ()) -> Self {
        uuid::Uuid::new_v4().to_string().into()
    }
}

/// A gathering of like-minded recipes and/or folders
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(deny_unknown_fields)]
pub struct Folder {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
    pub name: Option<String>,
    /// RECURSION. Use `requests` in serde to match the root field.
    #[serde(
        default,
        deserialize_with = "cereal::deserialize_id_map",
        rename = "requests"
    )]
    pub children: IndexMap<RecipeId, RecipeNode>,
}

impl Folder {
    /// Get a presentable name for this folder
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[cfg(test)]
impl crate::test_util::Factory for Folder {
    fn factory(_: ()) -> Self {
        Self {
            id: "folder1".into(),
            name: None,
            children: IndexMap::new(),
        }
    }
}

impl Recipe {
    /// Get a presentable name for this recipe
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[cfg(test)]
impl crate::test_util::Factory for Recipe {
    fn factory(_: ()) -> Self {
        Self {
            id: "recipe1".into(),
            name: None,
            method: Method::Get,
            url: "http://localhost/url".into(),
            body: None,
            authentication: None,
            query: Vec::new(),
            headers: IndexMap::new(),
        }
    }
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
    pub name: Option<String>,
    /// *Not* a template string because the usefulness doesn't justify the
    /// complexity. This gives the user an immediate error if the method is
    /// wrong which is helpful.
    pub method: Method,
    pub url: Template,
    pub body: Option<RecipeBody>,
    pub authentication: Option<Authentication>,
    #[serde(
        default,
        deserialize_with = "cereal::deserialize_query_parameters"
    )]
    pub query: Vec<(String, Template)>,
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

#[cfg(test)]
impl From<&str> for RecipeId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

#[cfg(test)]
impl crate::test_util::Factory for RecipeId {
    fn factory(_: ()) -> Self {
        uuid::Uuid::new_v4().to_string().into()
    }
}

/// HTTP method. This is duplicated from reqwest's Method so we can enforce
/// the method is valid during deserialization. This is also generally more
/// ergonomic at the cost of some flexibility.
///
/// The FromStr implementation will be case-insensitive
#[derive(
    Copy, Clone, Debug, Display, EnumIter, FromStr, Serialize, Deserialize,
)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(into = "String", try_from = "String")]
pub enum Method {
    #[display("CONNECT")]
    Connect,
    #[display("DELETE")]
    Delete,
    #[display("GET")]
    Get,
    #[display("HEAD")]
    Head,
    #[display("OPTIONS")]
    Options,
    #[display("PATCH")]
    Patch,
    #[display("POST")]
    Post,
    #[display("PUT")]
    Put,
    #[display("TRACE")]
    Trace,
}

/// For serialization
impl From<Method> for String {
    fn from(method: Method) -> Self {
        method.to_string()
    }
}

#[cfg(test)]
impl crate::test_util::Factory for Chain {
    fn factory(_: ()) -> Self {
        Self {
            id: "chain1".into(),
            source: ChainSource::Request {
                recipe: "recipe1".into(),
                trigger: Default::default(),
                section: Default::default(),
            },
            sensitive: false,
            selector: None,
            content_type: None,
            trim: ChainOutputTrim::default(),
        }
    }
}

/// Shortcut for defining authentication method. If this is defined in addition
/// to the `Authorization` header, that header will end up being included in the
/// request twice.
///
/// Type parameter allows this to be re-used for post-render purposes (with
/// `T=String`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Authentication<T = Template> {
    /// `Authorization: Basic {username:password | base64}`
    Basic { username: T, password: Option<T> },
    /// `Authorization: Bearer {token}`
    Bearer(T),
}

/// Template for a request body. `Raw` is the "default" variant, which repesents
/// a single string (parsed as a template). Other variants can be used for
/// convenience, to construct complex bodies in common formats. The HTTP engine
/// uses the variant to determine not only how to serialize the body, but also
/// other parameters of the request (e.g. the `Content-Type` header).
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum RecipeBody {
    /// Plain string/bytes body
    Raw(Template),
    /// Strutured JSON, which will be stringified and sent as text
    Json(JsonBody),
    /// `application/x-www-form-urlencoded` fields. Values must be strings
    FormUrlencoded(IndexMap<String, Template>),
    /// `multipart/form-data` fields. Values can be binary
    FormMultipart(IndexMap<String, Template>),
}

#[cfg(test)]
impl From<&str> for RecipeBody {
    fn from(template: &str) -> Self {
        Self::Raw(template.into())
    }
}

/// A structured JSON recipe body. This corresponds directly to
/// [serde_json::Value], but the value of the `String` variant is replaced with
/// a generic param `S`. For recipes, `S = Template`, so that strings can be
/// templatized. Other type params are used throughout the app to represent the
/// result of certain transformations.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(untagged)]
pub enum JsonBody<S = Template> {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(S),
    Array(Vec<Self>),
    Object(IndexMap<String, Self>),
}

impl<S> JsonBody<S> {
    /// Map from `JsonBody<S>` to `JsonBody<T>` by recursively applying a
    /// mapping function to each string value
    pub fn map<T>(self, mapper: impl Copy + Fn(S) -> T) -> JsonBody<T> {
        match self {
            Self::Null => JsonBody::Null,
            Self::Bool(b) => JsonBody::Bool(b),
            Self::Number(n) => JsonBody::Number(n),
            Self::String(template) => JsonBody::String(mapper(template)),
            Self::Array(values) => JsonBody::Array(
                values.into_iter().map(|value| value.map(mapper)).collect(),
            ),
            Self::Object(items) => JsonBody::Object(
                items
                    .into_iter()
                    .map(|(key, value)| (key.clone(), value.map(mapper)))
                    .collect(),
            ),
        }
    }

    /// Map from `&JsonBody<S>` to `JsonBody<T>` by recursively applying a
    /// mapping function to each string value
    pub fn map_ref<T>(&self, mapper: impl Copy + Fn(&S) -> T) -> JsonBody<T> {
        match self {
            Self::Null => JsonBody::Null,
            Self::Bool(b) => JsonBody::Bool(*b),
            Self::Number(n) => JsonBody::Number(n.clone()),
            Self::String(template) => JsonBody::String(mapper(template)),
            Self::Array(values) => JsonBody::Array(
                values.iter().map(|value| value.map_ref(mapper)).collect(),
            ),
            Self::Object(items) => JsonBody::Object(
                items
                    .iter()
                    .map(|(key, value)| (key.clone(), value.map_ref(mapper)))
                    .collect(),
            ),
        }
    }
}

/// If we're holding plain strings, we can easily convert to serde_json
impl From<JsonBody<String>> for serde_json::Value {
    fn from(value: JsonBody<String>) -> Self {
        match value {
            JsonBody::Null => serde_json::Value::Null,
            JsonBody::Bool(b) => serde_json::Value::Bool(b),
            JsonBody::Number(n) => serde_json::Value::Number(n.clone()),
            JsonBody::String(s) => serde_json::Value::String(s),
            JsonBody::Array(values) => serde_json::Value::Array(
                values.into_iter().map(Self::from).collect(),
            ),
            JsonBody::Object(items) => serde_json::Value::Object(
                items
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            ),
        }
    }
}

/// Convert parsed JSON
impl From<serde_json::Value> for JsonBody<String> {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Number(n) => Self::Number(n),
            serde_json::Value::String(s) => Self::String(s),
            serde_json::Value::Array(values) => {
                Self::Array(values.into_iter().map(Self::from).collect())
            }
            serde_json::Value::Object(items) => Self::Object(
                items
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            ),
        }
    }
}

/// Make it easier to construct JSON bodies in tests
#[cfg(test)]
impl From<serde_json::Value> for JsonBody<Template> {
    fn from(value: serde_json::Value) -> Self {
        let halfway: JsonBody<String> = value.into();
        halfway.map(|s| s.parse().unwrap())
    }
}

/// A chain is a means to data from one response in another request. The chain
/// is the middleman: it defines where and how to pull the value, then recipes
/// can use it in a template via `{{chains.<chain_id>}}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    pub trim: ChainOutputTrim,
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
    FromStr,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct ChainId(#[deref(forward)] Identifier);

impl<T: Into<Identifier>> From<T> for ChainId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

/// The source of data for a chain
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChainSource {
    /// Run an external command to get a result
    Command {
        command: Vec<Template>,
        stdin: Option<Template>,
    },
    /// Load from an environment variable
    #[serde(rename = "env")]
    Environment { variable: Template },
    /// Load data from a file
    File { path: Template },
    /// Prompt the user for a value
    Prompt {
        /// Descriptor to show to the user
        message: Option<Template>,
        /// Default value for the shown textbox
        default: Option<Template>,
    },
    /// Load data from the most recent response of a particular request recipe
    Request {
        recipe: RecipeId,
        /// When should this request be automatically re-executed?
        #[serde(default)]
        trigger: ChainRequestTrigger,
        #[serde(default)]
        section: ChainRequestSection,
    },
}

/// Test-only helpers
#[cfg(test)]
impl ChainSource {
    /// Build a new [Self::Command] variant from [command, ...args]
    pub fn command<const N: usize>(cmd: [&str; N]) -> ChainSource {
        ChainSource::Command {
            command: cmd.into_iter().map(Template::from).collect(),
            stdin: None,
        }
    }
}

/// The component of the response to use as the chain source
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChainRequestSection {
    #[default]
    Body,
    Header(String),
}

/// Define when a recipe with a chained request should auto-execute the
/// dependency request.
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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

/// Trim whitespace from rendered output
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChainOutputTrim {
    /// Do not trim the output
    #[default]
    None,
    /// Trim the start of the output
    Start,
    /// Trim the end of the output
    End,
    /// Trim the start and end of the output
    Both,
}

/// Test-only helpers
#[cfg(test)]
impl Collection {
    /// Get the ID of the first **recipe** (not recipe node) in the list. Panic
    /// if empty. This is useful because the default collection factory includes
    /// one recipe.
    pub fn first_recipe_id(&self) -> &RecipeId {
        self.recipes
            .recipe_ids()
            .next()
            .expect("Collection has no recipes")
    }

    /// Get the ID of the first profile in the list. Panic if empty. This is
    /// useful because the default collection factory includes one profile.
    pub fn first_profile_id(&self) -> &ProfileId {
        self.profiles.first().expect("Collection has no profiles").0
    }
}

#[cfg(test)]
impl crate::test_util::Factory for Collection {
    fn factory(_: ()) -> Self {
        use crate::test_util::by_id;
        let recipe = Recipe::factory(());
        let profile = Profile::factory(());
        Collection {
            recipes: by_id([recipe]).into(),
            profiles: by_id([profile]),
            ..Collection::default()
        }
    }
}

/// For deserialization
impl TryFrom<String> for Method {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        // Provide a better error than what's generated
        value.parse().map_err(|_| {
            anyhow!(
                "Invalid HTTP method `{value}`. Must be one of: {}",
                Method::iter().map(|method| method.to_string()).format(", ")
            )
        })
    }
}
