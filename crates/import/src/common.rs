//! Components and logic common to all import formats. Each format converts to a
//! common [Collection] struct, then we use that to generate a PetitScript AST.
//! TODO explain what this is and why it's all duplicated

mod cereal;
mod generate;
mod recipe_tree;
mod template;

pub(crate) use crate::common::{
    cereal::HasId,
    recipe_tree::{DuplicateRecipeIdError, RecipeNode, RecipeTree},
    template::{Identifier, Template},
};
// Re-export anything we might need from core, so individual importers don't
// have to mix and match between this module and the core crate
// TODO break this dependency
pub use slumber_core::{
    collection::{ProfileId, RecipeId},
    http::{HttpMethod, content_type::ContentType},
    util::NEW_ISSUE_LINK,
};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json_path::JsonPath;
use slumber_util::Duration;

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
///
/// This deliberately does not implement `Clone`, because it could potentially
/// be very large. Instead, it's hidden behind an `Arc` by `CollectionFile`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
    #[serde(default, deserialize_with = "cereal::deserialize_profiles")]
    pub(crate) profiles: IndexMap<ProfileId, Profile>,
    #[serde(default, deserialize_with = "cereal::deserialize_id_map")]
    pub(crate) chains: IndexMap<ChainId, Chain>,
    /// Internally we call these recipes, but to a user `requests` is more
    /// intuitive
    #[serde(default, rename = "requests")]
    pub(crate) recipes: RecipeTree,
    /// A hack-ish to allow users to add arbitrary data to their collection
    /// file without triggering a unknown field error. Ideally we could
    /// ignore anything that starts with `.` (recursively) but that
    /// requires a custom serde impl for each type, or changes to the macro
    #[serde(default, rename = ".ignore")]
    pub(crate) _ignore: serde::de::IgnoredAny,
}

/// Mutually exclusive hot-swappable config group
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    #[serde(skip)] // This will be auto-populated from the map key
    pub(crate) id: ProfileId,
    pub(crate) name: Option<String>,
    /// For the CLI, use this profile when no `--profile` flag is passed. For
    /// the TUI, select this profile by default from the list. Only one profile
    /// in the collection can be marked as default. This is enforced by a
    /// custom deserializer function.
    #[serde(default)]
    pub(crate) default: bool,
    pub(crate) data: IndexMap<String, Template>,
}

/// A gathering of like-minded recipes and/or folders
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Folder {
    #[serde(skip)] // This will be auto-populated from the map key
    pub(crate) id: RecipeId,
    pub(crate) name: Option<String>,
    /// RECURSION. Use `requests` in serde to match the root field.
    #[serde(
        default,
        deserialize_with = "cereal::deserialize_id_map",
        rename = "requests"
    )]
    pub(crate) children: IndexMap<RecipeId, RecipeNode>,
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipe {
    #[serde(skip)] // This will be auto-populated from the map key
    pub(crate) id: RecipeId,
    #[serde(default = "cereal::persist_default")]
    pub(crate) persist: bool,
    pub(crate) name: Option<String>,
    /// *Not* a template string because the usefulness doesn't justify the
    /// complexity. This gives the user an immediate error if the method is
    /// wrong which is helpful.
    pub(crate) method: HttpMethod,
    pub(crate) url: Template,
    pub(crate) body: Option<RecipeBody>,
    pub(crate) authentication: Option<Authentication>,
    #[serde(
        default,
        deserialize_with = "cereal::deserialize_query_parameters"
    )]
    pub(crate) query: Vec<(String, Template)>,
    #[serde(default, deserialize_with = "cereal::deserialize_headers")]
    pub(crate) headers: IndexMap<String, Template>,
}

/// Shortcut for defining authentication method. If this is defined in addition
/// to the `Authorization` header, that header will end up being included in the
/// request twice.
///
/// Type parameter allows this to be re-used for post-render purposes (with
/// `T=String`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Authentication {
    /// `Authorization: Basic {username:password | base64}`
    Basic {
        username: Template,
        password: Option<Template>,
    },
    /// `Authorization: Bearer {token}`
    Bearer(Template),
}

/// Template for a request body. `Raw` is the "default" variant, which
/// represents a single string (parsed as a template). Other variants can be
/// used for convenience, to construct complex bodies in common formats. The
/// HTTP engine uses the variant to determine not only how to serialize the
/// body, but also other parameters of the request (e.g. the `Content-Type`
/// header).
#[derive(Debug)]
pub(crate) enum RecipeBody {
    /// Plain string/bytes body
    Raw(Template),
    /// `application/json` body
    Json(serde_json::Value),
    /// `application/x-www-form-urlencoded` fields. Values must be strings
    FormUrlencoded(IndexMap<String, Template>),
    /// `multipart/form-data` fields. Values can be binary
    FormMultipart(IndexMap<String, Template>),
}

/// A chain is a means to data from one response in another request. The chain
/// is the middleman: it defines where and how to pull the value, then recipes
/// can use it in a template via `{{chains.<chain_id>}}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Chain {
    #[serde(skip)] // This will be auto-populated from the map key
    pub(crate) id: ChainId,
    pub(crate) source: ChainSource,
    /// Mask chained value in the UI
    #[serde(default)]
    pub(crate) sensitive: bool,
    /// Selector to extract a value from the response. This uses JSONPath
    /// regardless of the content type. Non-JSON values will be converted to
    /// JSON, then converted back.
    pub(crate) selector: Option<JsonPath>,
    /// Control selector behavior relative to number of query results
    #[serde(default)]
    pub(crate) selector_mode: SelectorMode,
    /// Hard-code the content type of the response. Only needed if a selector
    /// is given and the content type can't be dynamically determined
    /// correctly. This is needed if the chain source is not an HTTP
    /// response (e.g. a file) **or** if the response's `Content-Type` header
    /// is incorrect.
    pub(crate) content_type: Option<ContentType>,
    #[serde(default)]
    pub(crate) trim: ChainOutputTrim,
}

/// Unique ID for a chain, provided by the user
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Deserialize)]
#[serde(transparent)]
pub(crate) struct ChainId(Identifier);

impl From<Identifier> for ChainId {
    fn from(identifier: Identifier) -> Self {
        Self(identifier)
    }
}

/// The source of data for a chain
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ChainSource {
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
pub(crate) enum SelectOptions {
    Fixed(Vec<Template>),
    /// Render a template, then parse its output as a JSON array to get options
    Dynamic(Template),
}

/// The component of the response to use as the chain source
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ChainRequestSection {
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
pub(crate) enum ChainRequestTrigger {
    /// Never trigger the request. This is the default because upstream
    /// requests could be mutating, so we want the user to explicitly opt into
    /// automatic execution.
    #[default]
    Never,
    /// Trigger the request if there is none in history
    NoHistory,
    /// Trigger the request if the last response is older than some
    /// duration (or there is none in history)
    Expire(Duration),
    /// Trigger the request every time the dependent request is rendered
    Always,
}

/// Control how a JSONPath selector returns 0 vs 1 vs 2+ results
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SelectorMode {
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
pub(crate) enum ChainOutputTrim {
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
