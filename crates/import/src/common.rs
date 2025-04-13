//! TODO

mod cereal;
mod recipe_tree;
mod template;

pub use crate::common::{
    cereal::HasId,
    recipe_tree::{RecipeNode, RecipeTree},
    template::{Identifier, Template},
};
// Re-export anything we might need from core, so individual importers don't
// have to mix and match between this module and the core crate
pub use slumber_core::{
    collection::{ProfileId, RecipeId},
    http::{HttpMethod, content_type::ContentType},
    util::NEW_ISSUE_LINK,
};

use anyhow::Context;
use indexmap::IndexMap;
use mime::Mime;
use serde::Deserialize;
use serde_json_path::JsonPath;
use slumber_util::{ResultTraced, parse_yaml};
use std::{fs::File, path::PathBuf, time::Duration};
use tracing::info;

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
///
/// This deliberately does not implement `Clone`, because it could potentially
/// be very large. Instead, it's hidden behind an `Arc` by `CollectionFile`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
    #[serde(default, deserialize_with = "cereal::deserialize_profiles")]
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

impl Collection {
    /// Load collection from a file
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        info!(?path, "Loading collection file");

        let load = || {
            let file = File::open(path)?;
            let collection = parse_yaml(&file)?;
            Ok::<_, anyhow::Error>(collection)
        };

        load()
            .context(format!("Error loading collection from {path:?}"))
            .traced()
    }
}

/// Mutually exclusive hot-swappable config group
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: ProfileId,
    pub name: Option<String>,
    /// For the CLI, use this profile when no `--profile` flag is passed. For
    /// the TUI, select this profile by default from the list. Only one profile
    /// in the collection can be marked as default. This is enforced by a
    /// custom deserializer function.
    #[serde(default)]
    pub default: bool,
    pub data: IndexMap<String, Template>,
}

impl Profile {
    /// Get a presentable name for this profile
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    pub fn default(&self) -> bool {
        self.default
    }
}

/// A gathering of like-minded recipes and/or folders
#[derive(Debug, Deserialize)]
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

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
    #[serde(default = "cereal::persist_default")]
    pub persist: bool,
    pub name: Option<String>,
    /// *Not* a template string because the usefulness doesn't justify the
    /// complexity. This gives the user an immediate error if the method is
    /// wrong which is helpful.
    pub method: HttpMethod,
    pub url: Template,
    pub body: Option<RecipeBody>,
    pub authentication: Option<Authentication>,
    #[serde(
        default,
        deserialize_with = "cereal::deserialize_query_parameters"
    )]
    pub query: Vec<(String, Template)>,
    #[serde(default, deserialize_with = "cereal::deserialize_headers")]
    pub headers: IndexMap<String, Template>,
}

impl Recipe {
    /// Get a presentable name for this recipe
    /// TODO delete?
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// Shortcut for defining authentication method. If this is defined in addition
/// to the `Authorization` header, that header will end up being included in the
/// request twice.
///
/// Type parameter allows this to be re-used for post-render purposes (with
/// `T=String`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Authentication<T = Template> {
    /// `Authorization: Basic {username:password | base64}`
    Basic { username: T, password: Option<T> },
    /// `Authorization: Bearer {token}`
    Bearer(T),
}

/// Template for a request body. `Raw` is the "default" variant, which
/// represents a single string (parsed as a template). Other variants can be
/// used for convenience, to construct complex bodies in common formats. The
/// HTTP engine uses the variant to determine not only how to serialize the
/// body, but also other parameters of the request (e.g. the `Content-Type`
/// header).
#[derive(Debug)]
pub enum RecipeBody {
    /// Plain string/bytes body
    Raw {
        body: Template,
        /// For structured body types such as `!json`, we'll stringify during
        /// deserialization then just store the content type. This makes
        /// internal logic much simpler because we can just work with templates
        content_type: Option<ContentType>,
    },
    /// `application/x-www-form-urlencoded` fields. Values must be strings
    FormUrlencoded(IndexMap<String, Template>),
    /// `multipart/form-data` fields. Values can be binary
    FormMultipart(IndexMap<String, Template>),
}

impl RecipeBody {
    /// Build a JSON body *without* parsing the internal strings as templates.
    /// Useful for importing from external formats.
    pub fn untemplated_json(value: serde_json::Value) -> Self {
        Self::Raw {
            body: Template::raw(format!("{value:#}")),
            content_type: Some(ContentType::Json),
        }
    }

    /// Get the anticipated MIME type that will appear in the `Content-Type`
    /// header of a request containing this body. This is *not* necessarily
    /// the MIME type that will _actually_ be used, as it could be overidden by
    /// an explicit header.
    pub fn mime(&self) -> Option<Mime> {
        match self {
            RecipeBody::Raw { content_type, .. } => {
                content_type.as_ref().map(ContentType::to_mime)
            }
            RecipeBody::FormUrlencoded(_) => {
                Some(mime::APPLICATION_WWW_FORM_URLENCODED)
            }
            RecipeBody::FormMultipart(_) => Some(mime::MULTIPART_FORM_DATA),
        }
    }
}

/// A chain is a means to data from one response in another request. The chain
/// is the middleman: it defines where and how to pull the value, then recipes
/// can use it in a template via `{{chains.<chain_id>}}`.
#[derive(Debug, Deserialize)]
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
    pub selector: Option<JsonPath>,
    /// Control selector behavior relative to number of query results
    #[serde(default)]
    pub selector_mode: SelectorMode,
    /// Hard-code the content type of the response. Only needed if a selector
    /// is given and the content type can't be dynamically determined
    /// correctly. This is needed if the chain source is not an HTTP
    /// response (e.g. a file) **or** if the response's `Content-Type` header
    /// is incorrect.
    pub content_type: Option<ContentType>,
    #[serde(default)]
    pub trim: ChainOutputTrim,
}

/// Unique ID for a chain, provided by the user
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Deserialize)]
#[serde(transparent)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct ChainId(Identifier);

impl From<Identifier> for ChainId {
    fn from(identifier: Identifier) -> Self {
        Self(identifier)
    }
}

/// The source of data for a chain
#[derive(Debug, Deserialize)]
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
    /// Prompt the user to select a value from a list
    Select {
        /// Descriptor to show to the user
        message: Option<Template>,
        /// List of options to choose from
        options: SelectOptions,
    },
}

/// Static or dynamic list of options for a select chain
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum SelectOptions {
    Fixed(Vec<Template>),
    /// Render a template, then parse its output as a JSON array to get options
    Dynamic(Template),
}

/// The component of the response to use as the chain source
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChainRequestSection {
    #[default]
    Body,
    /// Pull a value from a response's headers. If the given header appears
    /// multiple times, the first value will be used
    Header(Template),
}

/// Define when a recipe with a chained request should auto-execute the
/// dependency request.
#[derive(Copy, Clone, Debug, Default, Deserialize)]
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
    Expire(
        #[serde(deserialize_with = "cereal::deserialize_duration")] Duration,
    ),
    /// Trigger the request every time the dependent request is rendered
    Always,
}

/// Control how a JSONPath selector returns 0 vs 1 vs 2+ results
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectorMode {
    /// 0 - Error
    /// 1 - Single result, without wrapping quotes
    /// 2 - JSON array
    #[default]
    Auto,
    /// 0 - Error
    /// 1 - Single result, without wrapping quotes
    /// 2 - Error
    Single,
    /// 0 - JSON array
    /// 1 - JSON array
    /// 2 - JSON array
    Array,
}

/// Trim whitespace from rendered output
#[derive(Copy, Clone, Debug, Default, Deserialize)]
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

impl Collection {
    /// Get the profile marked as `default: true`, if any. At most one profile
    /// can be marked as default.
    pub fn default_profile(&self) -> Option<&Profile> {
        self.profiles.values().find(|profile| profile.default)
    }
}
