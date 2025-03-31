//! The plain data types that make up a request collection

use crate::{
    collection::{
        cereal,
        recipe_tree::{RecipeNode, RecipeTree},
    },
    http::HttpMethod,
    template::Template,
};
use derive_more::{Deref, Display, From, Into};
use indexmap::IndexMap;
use mime::Mime;
use petitscript::{FromPs, error::ValueError};
use reqwest::header;
use serde::{Deserialize, Serialize};
use std::iter;

// TODO search for "chain" everywhere and rewrite comments

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
impl slumber_util::Factory for Folder {
    fn factory(_: ()) -> Self {
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
    #[serde(default, with = "cereal::serde_recipe_body")]
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
            .and_then(|template| template.to_string().parse::<Mime>().ok())
            .or_else(|| self.body.as_ref()?.mime())
    }

    /// Get a _flattened_ iterator over this recipe's query parameters. Any
    /// parameter with multiple values will be flattened so the key appears
    /// multiple times, once with each value. This will respect all ordering
    /// from the original map.
    pub fn query_iter(&self) -> impl Iterator<Item = (&str, &Template)> {
        self.query
            .iter()
            .flat_map(|(k, v)| v.iter().map(move |v| (k.as_str(), v)))
    }
}

#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory for Recipe {
    fn factory(_: ()) -> Self {
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

impl FromPs for RecipeId {
    fn from_ps(value: petitscript::Value) -> Result<Self, ValueError> {
        let string = value.into_todo::<String>()?;
        Ok(Self(string))
    }
}

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
    fn factory(_: ()) -> Self {
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
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Authentication<T = Template> {
    /// `Authorization: Basic {username:password | base64}`
    Basic { username: T, password: Option<T> },
    /// `Authorization: Bearer {token}`
    Bearer { token: T },
}

/// A value for a particular query parameter key
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "test"), derive(PartialEq))]
#[serde(untagged, deny_unknown_fields)]
pub enum QueryParameterValue {
    /// Multiple values for the same parameter. This will be represented by
    /// repeating the parameter key: `?foo=bar&foo=baz`
    /// Note: This variant has to be first for deserialization. Because the
    /// single case accepts any PS value, it will also accept an array.
    Many(Vec<Template>),
    /// The common case: `?foo=bar`
    Single(Template),
}

impl QueryParameterValue {
    /// Get an iterator over the contained values. For a single value this is
    /// just one element; for many is all the values in the list.
    pub fn iter(&self) -> Box<dyn Iterator<Item = &Template> + '_> {
        match self {
            Self::Many(values) => Box::new(values.iter()),
            Self::Single(value) => Box::new(iter::once(value)),
        }
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
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum RecipeBody {
    /// Plain string/bytes body
    Raw {
        data: Template,
    },
    Json {
        data: Template,
    },
    /// `application/x-www-form-urlencoded` fields. Values must be strings
    FormUrlencoded {
        data: IndexMap<String, Template>,
    },
    /// `multipart/form-data` fields. Values can be binary
    FormMultipart {
        data: IndexMap<String, Template>,
    },
}

impl RecipeBody {
    /// Build a JSON body *without* parsing the internal strings as templates.
    /// Useful for importing from external formats.
    pub fn untemplated_json(_value: serde_json::Value) -> Self {
        // TODO
        Self::Json {
            data: Default::default(),
        }
    }

    /// Get the anticipated MIME type that will appear in the `Content-Type`
    /// header of a request containing this body. This is *not* necessarily
    /// the MIME type that will _actually_ be used, as it could be overidden by
    /// an explicit header.
    pub fn mime(&self) -> Option<Mime> {
        match self {
            RecipeBody::Raw { .. } => None,
            RecipeBody::Json { .. } => Some(mime::APPLICATION_JSON),
            RecipeBody::FormUrlencoded { .. } => {
                Some(mime::APPLICATION_WWW_FORM_URLENCODED)
            }
            RecipeBody::FormMultipart { .. } => Some(mime::MULTIPART_FORM_DATA),
        }
    }
}

#[cfg(any(test, feature = "test"))]
impl From<&str> for RecipeBody {
    fn from(template: &str) -> Self {
        Self::Raw {
            data: template.into(),
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
    fn factory(_: ()) -> Self {
        use crate::test_util::by_id;
        use petitscript::Object;
        // Include a body in the recipe, so body-related behavior can be tested
        let recipe = Recipe {
            body: Some(RecipeBody::Json {
                data: Template::new(Object::new().insert("message", "hello")),
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
            headers,
            body,
            ..Recipe::factory(())
        };
        let expected = expected.and_then(|value| value.parse::<Mime>().ok());
        assert_eq!(recipe.mime(), expected);
    }
}
