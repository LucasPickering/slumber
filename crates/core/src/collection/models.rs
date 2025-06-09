//! The plain data types that make up a request collection

use crate::{
    collection::{
        cereal,
        recipe_tree::{RecipeNode, RecipeTree},
    },
    http::{HttpMethod, content_type::ContentType},
};
use anyhow::Context;
use derive_more::{Deref, Display, From, Into};
use indexmap::IndexMap;
use mime::Mime;
use reqwest::header;
use serde::{Deserialize, Serialize};
use slumber_template::Template;
use slumber_util::{ResultTraced, parse_yaml};
use std::{fs::File, iter, path::PathBuf};
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
impl slumber_util::Factory for Folder {
    fn factory((): ()) -> Self {
        Self {
            id: RecipeId::factory(()),
            name: None,
            children: IndexMap::new(),
        }
    }
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[expect(clippy::unsafe_derive_deserialize)]
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
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
    /// A map of key-value query parameters. Each value can either be a single
    /// value (`?foo=bar`) or multiple (`?foo=bar&foo=baz`)
    #[serde(default)]
    pub query: IndexMap<String, QueryParameterValue>,
    #[serde(default, deserialize_with = "cereal::deserialize_headers")]
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
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Authentication<T = Template> {
    /// `Authorization: Basic {username:password | base64}`
    Basic { username: T, password: Option<T> },
    /// `Authorization: Bearer {token}`
    Bearer(T),
}

/// A value for a particular query parameter key
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(untagged, deny_unknown_fields)]
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
#[derive(Debug)]
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

#[cfg(any(test, feature = "test"))]
impl From<&str> for RecipeBody {
    fn from(template: &str) -> Self {
        Self::Raw {
            body: template.into(),
            content_type: None,
        }
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
        }
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
        Some(RecipeBody::Raw {
            body: "hi!".into(),
            content_type: Some(ContentType::Json),
        }),
        Some("text/plain")
    )]
    #[case::unknown_mime(
        // Fall back to body type
        Some("bogus"),
        Some(RecipeBody::Raw {
            body: "hi!".into(),
            content_type: Some(ContentType::Json),
        }),
        Some("application/json")
    )]
    #[case::json_body(
        None,
        Some(RecipeBody::Raw {
            body: "hi!".into(),
            content_type: Some(ContentType::Json),
        }),
        Some("application/json")
    )]
    #[case::unknown_body(
        None,
        Some(RecipeBody::Raw {
            body: "hi!".into(),
            content_type: None,
        }),
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
