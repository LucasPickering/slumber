//! The plain data types that make up a request collection

use crate::{
    collection::{
        cereal,
        json::JsonTemplate,
        recipe_tree::{RecipeNode, RecipeTree},
    },
    http::HttpMethod,
};
use derive_more::{Deref, Display, From, Into};
use indexmap::IndexMap;
use mime::Mime;
use reqwest::header;
use serde::{Deserialize, Serialize};
use slumber_template::{Template, TemplateParseError};
use slumber_util::{
    ResultTraced, doc_link,
    yaml::{self, SourceLocation, YamlError, YamlErrorKind},
};
use std::{
    fmt, io, iter,
    path::{Path, PathBuf},
};
use thiserror::Error;
use tracing::info;

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
#[derive(Debug, Default, Serialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    feature = "schema",
    schemars(
        // Allow any top-level property beginning with .
        extend("patternProperties" = {
            "^\\.": { "description": "Ignore any property beginning with `.`" }
        }),
        example = Collection::example(),
    )
)]
pub struct Collection {
    /// Descriptive name for the collection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Map of profiles, keyed by their unique IDs
    pub profiles: IndexMap<ProfileId, Profile>,
    /// Map of requests and folders, keyed by their unique IDs. Folders allow
    /// for nested maps of more requests and folders. All IDs must be unique
    /// **throughout the entire tree**, not just at their level.
    #[serde(rename = "requests")]
    pub recipes: RecipeTree,
}

impl Collection {
    /// Load collection from a file
    pub fn load(path: &Path) -> Result<Self, CollectionError> {
        info!(?path, "Loading collection file");
        yaml::deserialize_file(path)
            .map_err(CollectionError::Yaml)
            .traced()
    }

    /// Load collection from a YAML string
    pub fn parse(input: &str) -> Result<Self, CollectionError> {
        yaml::deserialize_str(input)
            .map_err(CollectionError::Yaml)
            .traced()
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

    /// Get the ID of the first profile in the list. Panic if empty. This is
    /// useful because the default collection factory includes one profile.
    pub fn first_profile_id(&self) -> &ProfileId {
        self.profiles.first().expect("Collection has no profiles").0
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Collection {
    fn factory((): ()) -> Self {
        use crate::test_util::by_id;
        // Include a body in the recipe, so body-related behavior can be tested
        let recipe = Recipe {
            body: Some(RecipeBody::Json(
                r#"{"message": "hello"}"#.parse().unwrap(),
            )),
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
#[derive(Debug, Serialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(example = Profile::example()))]
pub struct Profile {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: ProfileId,
    pub name: Option<String>,
    /// For the CLI, use this profile when no `--profile` flag is passed. For
    /// the TUI, select this profile by default from the list. Only one profile
    /// in the collection can be marked as default. This is enforced by a
    /// custom deserializer function.
    #[serde(skip_serializing_if = "cereal::is_false")] // Skip if default
    #[cfg_attr(feature = "schema", schemars(default))]
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
impl slumber_util::Factory for Profile {
    fn factory((): ()) -> Self {
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
#[derive(Debug, Serialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(example = Folder::example()))]
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
            location: SourceLocation::default(),
            name: None,
            children: IndexMap::new(),
        }
    }
}

/// A definition of how to build an HTTP request. This is also commonly called
/// "request" throughout Slumber documentation because that term is more common
/// and intuitive.
#[derive(Debug, Serialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(example = Recipe::example()))]
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
    #[serde(skip_serializing_if = "cereal::is_true")] // Skip if default
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
    /// request. This will use the raw header if it's present and a valid MIME
    /// type, otherwise it will fall back to the content type of the body, if
    /// known (e.g. JSON). Otherwise, return `None`. If the header is a
    /// dynamic template, we will *not* attempt to render it, so MIME parsing
    /// will fail.
    pub fn mime(&self) -> Option<Mime> {
        self.headers
            .get(header::CONTENT_TYPE.as_str())
            .and_then(|template| template.display().parse::<Mime>().ok())
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

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Recipe {
    fn factory((): ()) -> Self {
        Self {
            id: RecipeId::factory(()),
            location: SourceLocation::default(),
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
    Display,
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
#[derive(Clone, Debug, Serialize)]
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
#[derive(Clone, Debug, Serialize)]
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
impl From<&str> for QueryParameterValue {
    fn from(value: &str) -> Self {
        QueryParameterValue::One(value.into())
    }
}

#[cfg(any(test, feature = "test"))]
impl<const N: usize> From<[&str; N]> for QueryParameterValue {
    fn from(values: [&str; N]) -> Self {
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
#[derive(Debug, Serialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum RecipeBody {
    /// `application/json` body. Value can be any JSON value, with strings being
    /// interpreted as templates
    Json(JsonTemplate),
    /// `application/x-www-form-urlencoded` body. Value is a mapping of form
    /// field name to value. Values are templates while field names are not.
    /// Values must render to strings.
    FormUrlencoded(IndexMap<String, Template>),
    /// `multipart/form-data` body. Value is a mapping of form field
    /// name to value. Values are templates while field names are not. Values
    /// can render to any bytes.
    FormMultipart(IndexMap<String, Template>),
    /// Plain body, but the bytes will be streamed instead of being loaded
    /// into memory
    Stream(Template),
    /// Plain string/bytes body. Must be the last variant to support untagged.
    /// This captures any value that doesn't fit one of the above variants.
    #[serde(untagged)]
    Raw(Template),
}

impl RecipeBody {
    /// Build a JSON body, parsing the internal strings as templates.
    /// Useful for importing from external formats.
    pub fn json(value: serde_json::Value) -> Result<Self, TemplateParseError> {
        Ok(Self::Json(value.try_into()?))
    }

    /// Build a JSON body *without* parsing the internal strings as templates.
    /// Useful for importing from external formats.
    pub fn untemplated_json(value: serde_json::Value) -> Self {
        Self::Json(JsonTemplate::raw(value))
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
impl From<&str> for RecipeBody {
    fn from(template: &str) -> Self {
        Self::Raw(template.into())
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

    /// Error parsing/deserializing the YAML
    #[error(fmt = fmt_yaml_error)]
    Yaml(#[source] YamlError),
}

/// Display a YAML error.  If the error makes it look like the user's trying to
/// load a v3 collection in v4, give some extra help
fn fmt_yaml_error(error: &YamlError, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "{error}")?;
    if is_v3_error(error) {
        write!(
            f,
            "\nThis looks like a collection from Slumber v3. \
            Migrate to v4 or downgrade your installation to 3.x.\
            \n{}",
            doc_link("other/v4_migration")
        )?;
    }
    Ok(())
}

/// Does the deserialization error lead us to believe the collection is in
/// the v3 format? If so we'll give the user an extra message suggesting an
/// upgrade to v4
fn is_v3_error(error: &YamlError) -> bool {
    match &error.kind {
        // Look for:
        // - `chains` field
        // - `<<` merge key
        // - `!tag`
        // - `chains.` in a template
        YamlErrorKind::UnexpectedField(field) => field == "chains",
        YamlErrorKind::UnsupportedMerge => true,
        YamlErrorKind::Unexpected { actual, .. } => actual.starts_with("tag"),
        YamlErrorKind::Other(error) => error
            // Check if there's a template containing a chain reference
            .downcast_ref::<TemplateParseError>()
            .is_some_and(|error| error.to_string().contains("{{chains.")),
        _ => false,
    }
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
        #[case] header: Option<&str>,
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
}
