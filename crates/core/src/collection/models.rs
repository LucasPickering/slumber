//! The plain data types that make up a request collection

use crate::{
    collection::{
        HasId,
        recipe_tree::{RecipeNode, RecipeTree},
    },
    http::HttpMethod,
    render::TemplateContext,
};
use derive_more::{Deref, From, Into};
use indexmap::IndexMap;
use mime::Mime;
use petitscript::{
    Engine, Exports, NativeModuleName, Source, Value, error::ValueError,
};
use reqwest::header;
use serde::{Deserialize, Serialize};
use slumber_util::yaml::SourceLocation;
use std::{
    io, iter,
    path::{Path, PathBuf},
    sync::LazyLock,
};
use thiserror::Error;
use tracing::info;

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
#[derive(Debug, Default)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(
    feature = "schema",
    derive(schemars::JsonSchema),
    schemars(
        // Allow any top-level property beginning with .
        extend("patternProperties" = {
            "^\\.": { "description": "Ignore any property beginning with `.`" }
        }),
        example = Collection::example(),
    ),
)]
pub struct Collection {
    /// Descriptive name for the collection
    pub name: Option<String>,
    /// Map of profiles, keyed by their unique IDs
    pub profiles: IndexMap<ProfileId, Profile>,
    /// Map of requests and folders, keyed by their unique IDs. Folders allow
    /// for nested maps of more requests and folders. All IDs must be unique
    /// **throughout the entire tree**, not just at their level.
    pub recipes: RecipeTree,
}

impl Collection {
    /// Load collection from a file
    pub fn load(path: &Path) -> Result<Self, CollectionError> {
        info!(?path, "Loading collection file");
        todo!()
    }

    /// TODO
    pub fn load_ps(path: &Path) -> Result<Self, petitscript::Error> {
        static ENGINE: LazyLock<Engine> = LazyLock::new(init_engine);

        fn init_engine() -> Engine {
            // TODO module name is clunky, shouldn't have to parse it
            let module_name: NativeModuleName = "slumber".parse().unwrap();
            let mut module = Exports::default();
            module.export_fn("command", |_, args: Vec<Value>| dbg!(args));
            Engine::builder()
                .with_stdlib()
                .with_module(module_name, module)
                .build()
        }

        info!(?path, "Loading collection file");
        let source = Source::file(path)
            .expect("TODO make convertible to petitscript::Error");
        let process = ENGINE.compile(source)?;

        let exports = process.execute()?;

        // Can't serialize directly to a collection because each field is
        // declared/exported separately
        let name = exports.get_deserialize("name")?;
        let profiles = exports.get_deserialize("profiles")?;
        let recipes = exports.get_deserialize("recipes")?;

        Ok(Self {
            name,
            profiles,
            recipes,
        })
    }

    /// Load collection from a YAML string
    pub fn parse(input: &str) -> Result<Self, CollectionError> {
        todo!()
    }
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

    /// Get the first **recipe** (not recipe node) in the list. Panic if empty.
    /// This is useful because the default collection factory includes one
    /// recipe.
    pub fn first_recipe(&self) -> &Recipe {
        let id = self.first_recipe_id();
        self.recipes.get_recipe(id).unwrap()
    }

    /// Get the ID of the first profile in the list. Panic if empty. This is
    /// useful because the default collection factory includes one profile.
    pub fn first_profile_id(&self) -> &ProfileId {
        self.profiles.first().expect("Collection has no profiles").0
    }

    /// Get the first profile in the list. Panic if empty. This is useful
    /// because the default collection factory includes one profile.
    pub fn first_profile(&self) -> &Profile {
        let id = self.first_profile_id();
        self.profiles.get(id).unwrap()
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Collection {
    fn factory((): ()) -> Self {
        use crate::test_util::by_id;
        use serde_json::json;

        // Include a body in the recipe, so body-related behavior can be tested
        let recipe = Recipe {
            body: Some(RecipeBody::json(json!({"message": "hello"})).unwrap()),
            ..Recipe::factory(())
        };
        let profile = Profile::factory(());
        Collection {
            name: None,
            recipes: by_id([recipe]).into(),
            profiles: by_id([profile]),
        }
    }
}

/// Mutually exclusive hot-swappable config group
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(
    feature = "schema",
    derive(schemars::JsonSchema),
    schemars(example = Profile::example()),
)]
pub struct Profile {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: ProfileId,
    /// Location where this profile is defined in YAML
    #[serde(skip)]
    pub location: SourceLocation,
    pub name: Option<String>,
    /// For the CLI, use this profile when no `--profile` flag is passed. For
    /// the TUI, select this profile by default from the list. Only one profile
    /// in the collection can be marked as default. This is enforced by a
    /// custom deserializer function.
    #[serde(default, skip_serializing_if = "is_false")] // Skip if default
    #[cfg_attr(feature = "schema", schemars(default))]
    pub default: bool,
    /// Arbitrary data that can be used in templates
    pub data: IndexMap<String, serde_json::Value>,
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

impl HasId for Profile {
    type Id = ProfileId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Profile {
    fn factory((): ()) -> Self {
        Self {
            id: ProfileId::factory(()),

            location: SourceLocation {
                source: "memory".into(),
                line: 5,
                column: 4,
            },
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
    derive_more::Display,
    Eq,
    From,
    Hash,
    Into,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[deref(forward)]
#[serde(transparent)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProfileId(String);

#[cfg(any(test, feature = "test"))]
impl From<&str> for ProfileId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for ProfileId {
    fn factory((): ()) -> Self {
        uuid::Uuid::new_v4().to_string().into()
    }
}

/// A gathering of like-minded recipes and/or folders
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(
    feature = "schema",
    derive(schemars::JsonSchema),
    schemars(example = Folder::example()),
)]
pub struct Folder {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
    /// Location where this folder is defined in YAML
    #[serde(skip)]
    pub location: SourceLocation,
    /// Display name
    pub name: Option<String>,
    /// Child requests of this folder
    // Use `requests` in serde to match the root collection field
    #[serde(rename = "requests")]
    pub children: IndexMap<RecipeId, RecipeNode>,
}

impl Folder {
    /// Get a presentable name for this folder
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Folder {
    fn factory((): ()) -> Self {
        Self {
            id: RecipeId::factory(()),
            location: SourceLocation {
                source: "memory".into(),
                line: 10,
                column: 4,
            },
            name: None,
            children: IndexMap::new(),
        }
    }
}

/// A definition of how to build an HTTP request. This is also commonly called
/// "request" throughout Slumber documentation because that term is more common
/// and intuitive.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(
    feature = "schema",
    derive(schemars::JsonSchema),
    schemars(example = Recipe::example()),
)]
pub struct Recipe {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
    /// Location where this recipe is defined in YAML
    #[serde(skip)]
    pub location: SourceLocation,
    /// Should requests and responses of this recipe be persisted in the local
    /// Slumber database?
    /// [See docs](https://slumber.lucaspickering.me/book/user_guide/database.html)
    /// for more info
    #[serde(skip_serializing_if = "is_true")] // Skip if default
    #[cfg_attr(feature = "schema", schemars(default = "persist_default"))]
    pub persist: bool,
    /// Display name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// [HTTP request method](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Methods)
    // *Not* a template string because the usefulness doesn't justify the
    // complexity. This gives the user an immediate error if the method is
    // wrong which is helpful.
    pub method: HttpMethod,
    /// [HTTP request URL](https://developer.mozilla.org/en-US/docs/Learn_web_development/Howto/Web_mechanics/What_is_a_URL)
    ///
    /// Query parameters *can* be included here, but typically it's easier to
    /// use the `query` field instead.
    pub url: Template,
    /// HTTP request body
    ///
    /// - `type: json`: `application/json` body
    /// - `type: form_urlencoded`: `application/x-www-form-urlencoded` body
    /// - `type: form_multipart`: `multipart/form-data` body
    /// - Any template can be given to define the literal request body
    ///   text/bytes. In this case, consider including a
    ///   [`Content-Type` header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Type)
    ///   to tell the server what type of content you're sending.
    ///
    /// See individual variants for more details on usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<RecipeBody>,
    /// HTTP authentication scheme
    ///
    /// - `type: basic`: [Basic authentication](https://swagger.io/docs/specification/v3_0/authentication/basic-authentication/)
    /// - `type: bearer`: [Bearer authentication](https://swagger.io/docs/specification/v3_0/authentication/bearer-authentication/)
    ///
    /// See individual variants for more details on usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<Authentication>,
    /// A map of [URL query parameters](https://developer.mozilla.org/en-US/docs/Learn_web_development/Howto/Web_mechanics/What_is_a_URL#parameters).
    /// Each value can either be a single value (`?foo=bar`) or multiple
    /// (`?foo=bar&foo=baz`)
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    #[cfg_attr(feature = "schema", schemars(default))]
    pub query: IndexMap<String, QueryParameterValue>,
    /// A map of [HTTP request headers](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers)
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    #[cfg_attr(feature = "schema", schemars(default))]
    pub headers: IndexMap<String, Template>,
}

impl Recipe {
    /// Get a presentable name for this recipe
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// Guess the value that the `Content-Type` header will have for a generated
    /// request
    ///
    /// This will use the raw header if it's present and a valid MIME type,
    /// otherwise it will fall back to the content type of the body, if
    /// known (e.g. JSON). Otherwise, return `None`. If the header is a
    /// dynamic template, we will *not* attempt to render it, so MIME parsing
    /// will fail.
    pub fn mime(&self) -> Option<Mime> {
        self.headers
            .get(header::CONTENT_TYPE.as_str())
            .and_then(|template| template.to_string().parse::<Mime>().ok())
            .or_else(|| self.body.as_ref()?.mime())
    }

    /// Get a _flattened_ iterator over this recipe's query parameters. Any
    /// parameter with multiple values will be flattened so the parameter name
    /// appears multiple times, once with each value. Each tuple will include
    /// the index of each value to distinguish repeated parameters. The index
    /// is unique to each parameter; it resets to 0 for each new parameter. This
    /// will respect all ordering from the original map.
    pub fn query_iter(&self) -> impl Iterator<Item = (&str, usize, &Template)> {
        self.query.iter().flat_map(|(k, v)| {
            let iter: Box<dyn Iterator<Item = _>> = match v {
                QueryParameterValue::One(value) => Box::new(iter::once(value)),
                QueryParameterValue::Many(values) => Box::new(values.iter()),
            };
            iter.enumerate().map(move |(i, v)| (k.as_str(), i, v))
        })
    }
}

impl HasId for Recipe {
    type Id = RecipeId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Recipe {
    fn factory((): ()) -> Self {
        Self {
            id: RecipeId::factory(()),
            location: SourceLocation {
                source: "memory".into(),
                line: 20,
                column: 4,
            },
            persist: true,
            name: None,
            method: HttpMethod::Get,
            url: "http://localhost/url".into(),
            body: None,
            authentication: None,
            query: IndexMap::new(),
            headers: IndexMap::new(),
        }
    }
}

/// Create recipe with a fixed ID
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<&str> for Recipe {
    fn factory(id: &str) -> Self {
        Self {
            id: id.into(),
            ..Self::factory(())
        }
    }
}

/// Default value for `Recipe::persist`
#[cfg(feature = "schema")]
fn persist_default() -> bool {
    true
}

#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    derive_more::Display,
    Eq,
    From,
    Hash,
    Into,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[deref(forward)]
#[serde(transparent)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecipeId(String);

#[cfg(any(test, feature = "test"))]
impl From<&str> for RecipeId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

/// For rstest magic conversions
#[cfg(any(test, feature = "test"))]
impl std::str::FromStr for RecipeId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok::<_, ()>(s.to_owned().into())
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for RecipeId {
    fn factory((): ()) -> Self {
        uuid::Uuid::new_v4().to_string().into()
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
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Authentication<T = Template> {
    /// `Authorization: Basic {username:password | base64}`
    Basic { username: T, password: Option<T> },
    /// `Authorization: Bearer {token}`
    Bearer { token: T },
}

/// A value for a particular query parameter key
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum QueryParameterValue {
    /// The common case: `?foo=bar`
    One(Template),
    /// Multiple values for the same parameter. This will be represented by
    /// repeating the parameter key: `?foo=bar&foo=baz`
    Many(Vec<Template>),
}

#[cfg(any(test, feature = "test"))]
impl From<&'static str> for QueryParameterValue {
    fn from(value: &'static str) -> Self {
        QueryParameterValue::One(value.into())
    }
}

#[cfg(any(test, feature = "test"))]
impl<const N: usize> From<[&'static str; N]> for QueryParameterValue {
    fn from(values: [&'static str; N]) -> Self {
        QueryParameterValue::Many(
            values.into_iter().map(Template::from).collect(),
        )
    }
}

/// Template for a request body. `Raw` is the "default" variant, which
/// represents a single string (parsed as a template). Other variants can be
/// used for convenience, to construct complex bodies in common formats. The
/// HTTP engine uses the variant to determine not only how to serialize the
/// body, but also other parameters of the request (e.g. the `Content-Type`
/// header).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum RecipeBody {
    /// `application/json` body. Value can be any JSON value, with strings being
    /// interpreted as templates
    Json(Value),
    /// `application/x-www-form-urlencoded` body. Value is a mapping of form
    /// field name to value. Values are templates while field names are not.
    /// Values must render to strings.
    FormUrlencoded(IndexMap<String, Value>),
    /// `multipart/form-data` body. Value is a mapping of form field
    /// name to value. Values are templates while field names are not. Values
    /// can render to any bytes.
    FormMultipart(IndexMap<String, Value>),
    /// Plain body, but the bytes will be streamed instead of being loaded
    /// into memory
    Stream(Value),
    /// Plain string/bytes body. Must be the last variant to support untagged.
    /// This captures any value that doesn't fit one of the above variants.
    #[serde(untagged)]
    Raw(Value),
}

impl RecipeBody {
    /// Build a JSON body, parsing the internal strings as templates.
    /// Useful for importing from external formats.
    pub fn json(value: serde_json::Value) -> Result<Self, ValueError> {
        todo!()
        // Ok(Self::Json(value.try_into()?))
    }

    /// Build a JSON body *without* parsing the internal strings as templates.
    /// Useful for importing from external formats.
    pub fn untemplated_json(value: serde_json::Value) -> Self {
        todo!()
        // Self::Json(ValueTemplate::from_raw_json(value))
    }

    /// Get the anticipated MIME type that will appear in the `Content-Type`
    /// header of a request containing this body. This is *not* necessarily
    /// the MIME type that will _actually_ be used, as it could be overidden by
    /// an explicit header.
    pub fn mime(&self) -> Option<Mime> {
        match self {
            RecipeBody::Raw(_) | RecipeBody::Stream(_) => None,
            RecipeBody::Json(_) => Some(mime::APPLICATION_JSON),
            RecipeBody::FormUrlencoded(_) => {
                Some(mime::APPLICATION_WWW_FORM_URLENCODED)
            }
            RecipeBody::FormMultipart(_) => Some(mime::MULTIPART_FORM_DATA),
        }
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&'static str> for RecipeBody {
    fn from(template: &'static str) -> Self {
        Self::Raw(template.into())
    }
}

// TODO move this and make it its own enum
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Template(Value);

impl Template {
    /// TODo
    pub fn render(
        &self,
        context: &TemplateContext,
    ) -> Result<Value, petitscript::Error> {
        todo!()
    }
}

/// Error that can occur while loading a collection
#[derive(Debug, Error)]
pub enum CollectionError {
    /// Error getting the current working directory
    #[error(transparent)]
    CurrentDir(io::Error),

    /// File system error
    #[error("Error loading `{}`", path.display())]
    Io {
        path: PathBuf,
        #[source]
        error: io::Error,
    },

    /// No collection file exists that the given path or in the given directory
    #[error("No collection file found in `{}` or its ancestors", path.display())]
    NoFile { path: PathBuf },

    /// Error parsing/executing the PetitScript
    #[error(transparent)]
    PetitScript(petitscript::Error),
}

impl CollectionError {
    /// Get the source location of the error
    ///
    /// `None` if the error occurred while opening the file rather than parsing
    /// or deserializing.
    pub fn location(&self) -> Option<&SourceLocation> {
        // TODO this would be great!!
        None
    }
}

/// Predicate for skip_serializing_if
#[expect(clippy::trivially_copy_pass_by_ref)]
fn is_true(b: &bool) -> bool {
    *b
}

/// Predicate for skip_serializing_if
#[expect(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !b
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use itertools::Itertools;
    use rstest::rstest;
    use slumber_util::Factory;

    #[rstest]
    #[case::none(None, None, None)]
    #[case::header(
        // Header takes precedence over body
        Some("text/plain"),
        Some(RecipeBody::untemplated_json("hi!".into())),
        Some("text/plain")
    )]
    #[case::unknown_mime(
        // Fall back to body type
        Some("bogus"),
        Some(RecipeBody::untemplated_json("hi!".into())),
        Some("application/json")
    )]
    #[case::json_body(
        None,
        Some(RecipeBody::untemplated_json("hi!".into())),
        Some("application/json")
    )]
    #[case::unknown_body(
        None,
        Some(RecipeBody::Raw("hi!".into())),
        None,
    )]
    #[case::form_urlencoded_body(
        None,
        Some(RecipeBody::FormUrlencoded(indexmap! {})),
        Some("application/x-www-form-urlencoded")
    )]
    #[case::form_multipart_body(
        None,
        Some(RecipeBody::FormMultipart(indexmap! {})),
        Some("multipart/form-data")
    )]
    fn test_recipe_mime(
        #[case] header: Option<&'static str>,
        #[case] body: Option<RecipeBody>,
        #[case] expected: Option<&str>,
    ) {
        let mut headers = IndexMap::new();
        if let Some(header) = header {
            headers.insert("content-type".into(), header.into());
        }
        let recipe = Recipe {
            body,
            headers,
            ..Recipe::factory(())
        };
        let expected = expected.and_then(|value| value.parse::<Mime>().ok());
        assert_eq!(recipe.mime(), expected);
    }

    #[test]
    fn test_query_iter() {
        let recipe = Recipe {
            query: indexmap! {
                "param1".into() => ["value1.1", "value1.2"].into(),
                "param2".into() => "value2.1".into(),
            },
            ..Recipe::factory(())
        };
        assert_eq!(
            recipe.query_iter().collect_vec().as_slice(),
            &[
                ("param1", 0, &"value1.1".into()),
                ("param1", 1, &"value1.2".into()),
                ("param2", 0, &"value2.1".into()),
            ]
        );
    }

    /// TODO
    #[test]
    fn test_js() {
        let collection =
            Collection::load_ps(Path::new("../../slumber.ts")).unwrap();
        assert_eq!(
            collection,
            Collection {
                name: Some("Example".into()),
                profiles: IndexMap::new(),
                recipes: RecipeTree::default(),
            }
        );
    }
}
