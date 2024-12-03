//! The plain data types that make up a request collection

use crate::{
    collection::{
        cereal,
        recipe_tree::{RecipeNode, RecipeTree},
    },
    http::{content_type::ContentType, query::Query},
    template::{Identifier, Template},
    util::{parse_yaml, ResultTraced},
};
use anyhow::{anyhow, Context};
use derive_more::{Deref, Display, From, FromStr};
use hcl::{
    expr::{Traversal, TraversalOperator},
    Attribute, Body, Expression, Structure,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::PathBuf,
    time::Duration,
};
use strum::{EnumIter, IntoEnumIterator};
use tracing::info;

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
///
/// This deliberately does not implement `Clone`, because it could potentially
/// be very large. Instead, it's hidden behind an `Arc` by `CollectionFile`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(deny_unknown_fields)]
pub struct Collection {
    #[serde(default, deserialize_with = "cereal::deserialize_profiles")]
    pub profiles: IndexMap<ProfileId, Profile>,
    /// Internally we call these recipes, but to a user `requests` is more
    /// intuitive
    #[serde(default, rename = "requests")]
    pub recipes: RecipeTree,
    /// TODO
    pub locals: IndexMap<String, Expression>,
}

impl Collection {
    /// Load collection from a file
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        info!(?path, "Loading collection file");

        let load = || {
            let file = File::open(path)?;
            let collection = if path.extension().unwrap_or_default() == "hcl" {
                let content = fs::read_to_string(path)?;
                let deserializer = hcl::de::Deserializer::from_str(&content)?;
                let half_done: HalfDone =
                    serde_path_to_error::deserialize(deserializer)?;
                half_done.try_into_collection()?
            } else {
                parse_yaml(&file)?
            };
            Ok::<_, anyhow::Error>(collection)
        };

        load()
            .context(format!("Error loading data from {path:?}"))
            .traced()
    }
}

/// Mutually exclusive hot-swappable config group
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for Profile {
    fn factory(_: ()) -> Self {
        Self {
            id: ProfileId::factory(()),
            name: None,
            default: false,
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
#[serde(transparent)]
pub struct ProfileId(String);

#[cfg(any(test, feature = "test"))]
impl From<&str> for ProfileId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for ProfileId {
    fn factory(_: ()) -> Self {
        uuid::Uuid::new_v4().to_string().into()
    }
}

/// A gathering of like-minded recipes and/or folders
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for Folder {
    fn factory(_: ()) -> Self {
        Self {
            id: RecipeId::factory(()),
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

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for Recipe {
    fn factory(_: ()) -> Self {
        Self {
            id: RecipeId::factory(()),
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

/// Create recipe with a fixed ID
#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory<&str> for Recipe {
    fn factory(id: &str) -> Self {
        Self {
            id: id.into(),
            ..Self::factory(())
        }
    }
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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
    #[serde(default, with = "cereal::serde_query_parameters")]
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
#[serde(transparent)]
pub struct RecipeId(String);

#[cfg(any(test, feature = "test"))]
impl From<&str> for RecipeId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

#[cfg(any(test, feature = "test"))]
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
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for Chain {
    fn factory(_: ()) -> Self {
        Self {
            id: "chain1".into(),
            source: ChainSource::Request {
                recipe: RecipeId::factory(()),
                trigger: Default::default(),
                section: Default::default(),
            },
            sensitive: false,
            selector: None,
            selector_mode: SelectorMode::default(),
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
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
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
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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
}

#[cfg(any(test, feature = "test"))]
impl From<&str> for RecipeBody {
    fn from(template: &str) -> Self {
        Self::Raw {
            body: template.into(),
            content_type: None,
        }
    }
}

/// Define when a recipe with a chained request should auto-execute the
/// dependency request.
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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

/// Control how a JSONPath selector returns 0 vs 1 vs 2+ results
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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
#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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

/// Test-only helpers
#[cfg(any(test, feature = "test"))]
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

#[cfg(any(test, feature = "test"))]
impl crate::test_util::Factory for Collection {
    fn factory(_: ()) -> Self {
        use crate::test_util::by_id;
        // Include a body in the recipe, so body-related behavior can be tested
        let recipe = Recipe {
            body: Some(RecipeBody::Raw {
                body: r#"{"message": "hello"}"#.into(),
                content_type: Some(ContentType::Json),
            }),
            ..Recipe::factory(())
        };
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

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct HalfDone {
    body: Body,
}

impl HalfDone {
    const LOCALS: &'static str = "locals";

    fn try_into_collection(mut self) -> anyhow::Result<Collection> {
        self.resolve()?;
        let deserializer = hcl::de::Deserializer::from_body(self.body)?;
        serde_path_to_error::deserialize(deserializer).context("TODO")
    }

    fn resolve(&mut self) -> anyhow::Result<()> {
        self.resolve_includes()?;
        self.resolve_locals()?;
        Ok(())
    }

    fn resolve_includes(&mut self) -> anyhow::Result<()> {
        // TODO
        Ok(())
    }

    /// TODO
    fn resolve_locals(&mut self) -> anyhow::Result<()> {
        let locals = self.load_locals()?;
        self.body.resolve_locals(&locals)
    }

    /// TODO
    fn load_locals(
        &mut self,
        // TODO import hcl::Identifier
    ) -> anyhow::Result<IndexMap<hcl::Identifier, Expression>> {
        // TODO error if "locals" is an attribute
        let Some(locals) = self
            .body
            .blocks()
            .find(|block| block.identifier() == Self::LOCALS)
        else {
            return Ok(IndexMap::new());
        };
        // TODO assert labels empty
        // TODO assert body.blocks empty
        let map = locals
            .body
            .attributes()
            .map(|attr| (attr.key.clone(), attr.expr.clone()))
            .collect();
        Ok(map)
    }
}

/// TODO
trait Resolve {
    fn resolve_locals(
        &mut self,
        locals: &IndexMap<hcl::Identifier, Expression>,
    ) -> anyhow::Result<()>;
}

impl Resolve for Body {
    fn resolve_locals(
        &mut self,
        locals: &IndexMap<hcl::Identifier, Expression>,
    ) -> anyhow::Result<()> {
        for structure in self {
            structure.resolve_locals(locals)?;
        }
        Ok(())
    }
}

impl Resolve for Structure {
    fn resolve_locals(
        &mut self,
        locals: &IndexMap<hcl::Identifier, Expression>,
    ) -> anyhow::Result<()> {
        match self {
            Structure::Attribute(Attribute { ref mut expr, .. }) => {
                expr.resolve_locals(locals)
            }
            Structure::Block(block) => block.body.resolve_locals(locals),
        }
    }
}

impl Resolve for Expression {
    fn resolve_locals(
        &mut self,
        locals: &IndexMap<hcl::Identifier, Expression>,
    ) -> anyhow::Result<()> {
        // TODO handle other expression types
        let Expression::Traversal(traversal) = self else {
            return Ok(());
        };
        let Traversal {
            expr: Expression::Variable(variable),
            operators,
        } = &mut **traversal
        else {
            // TODO handle other expression types
            return Ok(());
        };
        if variable.as_str() != HalfDone::LOCALS {
            // TODO: should be error instead?
            return Ok(());
        }
        let [TraversalOperator::GetAttr(field)] = operators.as_slice() else {
            todo!("return error")
        };
        // Replace this alias with the assigned expression
        *self = locals.get(field).ok_or_else(|| anyhow!("TODO"))?.clone();
        Ok(())
    }
}
