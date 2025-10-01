//! v3 collection format models, copied from the old code with minor revisions.
//!
//! Some of these types are common with v4 but we copy them because:
//! - v4 doesn't use serde for deserialization
//! - v4 types may change over time
//!
//! The only exception is ID types since they're so simple and stable.

use crate::v3::template::{Identifier, Template};
use derive_more::{From, FromStr};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{
    Deserialize, Deserializer,
    de::{
        self, EnumAccess, Error as _, MapAccess, SeqAccess, VariantAccess,
        Visitor,
    },
};
use serde_json_path::JsonPath;
use slumber_core::{
    collection::{HasId, ProfileId, RecipeId},
    http::HttpMethod,
};
use std::time::Duration;
use winnow::{ModalResult, Parser, ascii::digit1, token::take_while};

/// A collection of profiles, requests, etc. This is the primary Slumber unit
/// of configuration.
///
/// This deliberately does not implement `Clone`, because it could potentially
/// be very large. Instead, it's hidden behind an `Arc` by `CollectionFile`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Collection {
    /// Descriptive name for the collection
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_profiles")]
    pub profiles: IndexMap<ProfileId, Profile>,
    #[serde(default, deserialize_with = "deserialize_id_map")]
    pub chains: IndexMap<ChainId, Chain>,
    /// Instead of using the full RecipeTree type, we simplify this here and
    /// just use the flat map. If there are duplicate IDs in the tree, we'll
    /// catch it during the conversion to v4 instead of v3 deserialization.
    #[serde(
        default,
        rename = "requests",
        deserialize_with = "deserialize_id_map"
    )]
    pub recipes: IndexMap<RecipeId, RecipeNode>,
}

/// Mutually exclusive hot-swappable config group
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
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

/// A node in the recipe tree, either a folder or recipe
#[derive(Debug, From, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RecipeNode {
    Folder(Folder),
    /// Rename this variant to match the `requests` field in the root and
    /// folders
    #[serde(rename = "request")]
    Recipe(Recipe),
}

/// A gathering of like-minded recipes and/or folders
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Folder {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
    pub name: Option<String>,
    /// RECURSION. Use `requests` in serde to match the root field.
    #[serde(
        default,
        deserialize_with = "deserialize_id_map",
        rename = "requests"
    )]
    pub children: IndexMap<RecipeId, RecipeNode>,
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
    #[serde(skip)] // This will be auto-populated from the map key
    pub id: RecipeId,
    #[serde(default = "persist_default")]
    pub persist: bool,
    pub name: Option<String>,
    /// *Not* a template string because the usefulness doesn't justify the
    /// complexity. This gives the user an immediate error if the method is
    /// wrong which is helpful.
    pub method: HttpMethod,
    pub url: Template,
    pub body: Option<RecipeBody>,
    pub authentication: Option<Authentication>,
    #[serde(default, deserialize_with = "deserialize_query_parameters")]
    pub query: Vec<(String, Template)>,
    #[serde(default, deserialize_with = "deserialize_headers")]
    pub headers: IndexMap<String, Template>,
}

/// Shortcut for defining authentication method. If this is defined in addition
/// to the `Authorization` header, that header will end up being included in the
/// request twice.
///
/// Type parameter allows this to be re-used for post-render purposes (with
/// `T=String`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Authentication<T = Template> {
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
pub(super) enum RecipeBody {
    /// Plain string/bytes body
    Raw(Template),
    /// `application/json` body
    Json(JsonTemplate),
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
pub(super) struct Chain {
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
    /// v3 has a content_type field, but the only possible value for it is
    /// `json` so it's pretty pointless. We've dropped that in v4 and
    /// content type is assumed contextually (e.g. jsonpath assumes it's
    /// JSON). This is here just to prevent errors during deserialization.
    #[expect(unused)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub trim: ChainOutputTrim,
}

/// Unique ID for a chain, provided by the user
#[derive(Clone, Debug, Default, Eq, FromStr, Hash, PartialEq, Deserialize)]
#[serde(transparent)]
pub(super) struct ChainId(pub Identifier);

/// The source of data for a chain
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ChainSource {
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
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum SelectOptions {
    Fixed(Vec<Template>),
    /// Render a template, then parse its output as a JSON array to get options
    Dynamic(Template),
}

/// The component of the response to use as the chain source
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ChainRequestSection {
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
pub(super) enum ChainRequestTrigger {
    /// Never trigger the request. This is the default because upstream
    /// requests could be mutating, so we want the user to explicitly opt into
    /// automatic execution.
    #[default]
    Never,
    /// Trigger the request if there is none in history
    NoHistory,
    /// Trigger the request if the last response is older than some
    /// duration (or there is none in history)
    Expire(#[serde(deserialize_with = "deserialize_duration")] Duration),
    /// Trigger the request every time the dependent request is rendered
    Always,
}

/// Control how a JSONPath selector returns 0 vs 1 vs 2+ results
#[derive(Copy, Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum SelectorMode {
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
pub(super) enum ChainOutputTrim {
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

/// A JSON value like [serde_json::Value], but all strings are templates
#[derive(Clone, Debug)]
pub(super) enum JsonTemplate {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(Template),
    Array(Vec<Self>),
    Object(IndexMap<String, Self>),
}

impl TryFrom<serde_json::Value> for JsonTemplate {
    type Error = String;

    /// Convert static JSON to templated JSON, parsing each string as a template
    fn try_from(json: serde_json::Value) -> Result<Self, Self::Error> {
        let mapped = match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Number(number) => Self::Number(number),
            serde_json::Value::String(s) => Self::String(s.parse()?),
            serde_json::Value::Array(values) => Self::Array(
                values
                    .into_iter()
                    .map(JsonTemplate::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            serde_json::Value::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| {
                        let value = value.try_into()?;
                        Ok::<_, String>((key, value))
                    })
                    .collect::<Result<_, _>>()?,
            ),
        };
        Ok(mapped)
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

impl HasId for RecipeNode {
    type Id = RecipeId;

    fn id(&self) -> &Self::Id {
        match self {
            Self::Folder(folder) => &folder.id,
            Self::Recipe(recipe) => &recipe.id,
        }
    }

    fn set_id(&mut self, id: Self::Id) {
        match self {
            Self::Folder(folder) => folder.id = id,
            Self::Recipe(recipe) => recipe.id = id,
        }
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

impl HasId for Chain {
    type Id = ChainId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

/// Default value generator for Recipe::persist
fn persist_default() -> bool {
    true
}

/// Deserialize a map, and update each key so its `id` field matches its key in
/// the map. Useful if you need to access the ID when you only have a value
/// available, not the full entry.
fn deserialize_id_map<'de, Map, V, D>(deserializer: D) -> Result<Map, D::Error>
where
    Map: Deserialize<'de>,
    for<'m> &'m mut Map: IntoIterator<Item = (&'m V::Id, &'m mut V)>,
    D: Deserializer<'de>,
    V: Deserialize<'de> + HasId,
    V::Id: Clone + Deserialize<'de>,
{
    let mut map: Map = Map::deserialize(deserializer)?;
    // Update the ID on each value to match the key
    for (k, v) in &mut map {
        v.set_id(k.clone());
    }
    Ok(map)
}

/// Deserialize a profile mapping. This also enforces that only one profile is
/// marked as default
fn deserialize_profiles<'de, D>(
    deserializer: D,
) -> Result<IndexMap<ProfileId, Profile>, D::Error>
where
    D: Deserializer<'de>,
{
    let profiles: IndexMap<ProfileId, Profile> =
        deserialize_id_map(deserializer)?;

    // Make sure at most one profile is the default
    let is_default = |profile: &&Profile| profile.default;

    if profiles.values().filter(is_default).count() > 1 {
        return Err(de::Error::custom(format!(
            "Only one profile can be the default, but multiple were: {}",
            profiles
                .values()
                .filter(is_default)
                .map(Profile::id)
                .format(", ")
        )));
    }

    Ok(profiles)
}

/// Deserialize a header map, lowercasing all header names. Headers are
/// case-insensitive (and must be lowercase in HTTP/2+), so forcing the case
/// makes lookups on the map easier.
pub fn deserialize_headers<'de, D>(
    deserializer: D,
) -> Result<IndexMap<String, Template>, D::Error>
where
    D: Deserializer<'de>,
{
    // This involves an extra allocation, but it makes the logic a lot easier.
    // These maps should be small anyway
    let headers: IndexMap<String, Template> =
        IndexMap::deserialize(deserializer)?;
    Ok(headers
        .into_iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
        .collect())
}

/// Deserialize query parameters from either a sequence of `key=value` or a map
/// of `key: value`
fn deserialize_query_parameters<'de, D>(
    deserializer: D,
) -> Result<Vec<(String, Template)>, D::Error>
where
    D: Deserializer<'de>,
{
    struct QueryParametersVisitor;

    impl<'de> Visitor<'de> for QueryParametersVisitor {
        type Value = Vec<(String, Template)>;

        fn expecting(
            &self,
            formatter: &mut std::fmt::Formatter,
        ) -> std::fmt::Result {
            formatter.write_str("sequence of \"<param>=<value>\" or map")
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Vec::new())
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut query: Vec<(String, Template)> =
                Vec::with_capacity(seq.size_hint().unwrap_or(5));
            while let Some(value) = seq.next_element::<String>()? {
                let (param, value) =
                    value.split_once('=').ok_or_else(|| {
                        de::Error::custom(
                            "Query parameters must be in the form \
                                `\"<param>=<value>\"`",
                        )
                    })?;

                if param.is_empty() {
                    return Err(de::Error::custom(
                        "Query parameter name cannot be empty",
                    ));
                }

                let key = param.to_string();
                let value = value.parse().map_err(de::Error::custom)?;

                query.push((key, value));
            }
            Ok(query)
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut query: Vec<(String, Template)> =
                Vec::with_capacity(map.size_hint().unwrap_or(5));
            while let Some((key, value)) = map.next_entry()? {
                query.push((key, value));
            }
            Ok(query)
        }
    }

    deserializer.deserialize_any(QueryParametersVisitor)
}

/// Deserialize a duration with unit shorthand. This does *not* handle
/// subsecond precision. Supported units are:
/// - s
/// - m
/// - h
/// - d
///
/// Examples: `30s`, `5m`, `12h`, `3d`
///
/// Unlike v4, this does *not* support composite durations, just
/// `<number><unit>`
fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Debug)]
    enum Unit {
        Second,
        Minute,
        Hour,
        Day,
    }

    impl FromStr for Unit {
        type Err = String;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            match s {
                "s" => Ok(Self::Second),
                "m" => Ok(Self::Minute),
                "h" => Ok(Self::Hour),
                "d" => Ok(Self::Day),
                _ => Err(format!(
                    "Unknown duration unit `{s}`; must be one of \
                    `s`, `m`, `h`, or `d`"
                )),
            }
        }
    }

    fn quantity(input: &mut &str) -> ModalResult<u64> {
        digit1.parse_to().parse_next(input)
    }

    fn unit<'a>(input: &mut &'a str) -> ModalResult<&'a str> {
        take_while(1.., char::is_alphabetic).parse_next(input)
    }

    let input = String::deserialize(deserializer)?;
    let (quantity, unit) = (quantity, unit)
        .parse(&input)
        // The format is so simple there isn't much value in spitting out a
        // specific parsing error, just use a canned one
        .map_err(|_| {
            D::Error::custom(
                "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)",
            )
        })?;

    let unit = unit.parse().map_err(D::Error::custom)?;
    let seconds = match unit {
        Unit::Second => quantity,
        Unit::Minute => quantity * 60,
        Unit::Hour => quantity * 60 * 60,
        Unit::Day => quantity * 60 * 60 * 24,
    };
    Ok(Duration::from_secs(seconds))
}

impl RecipeBody {
    // Constants for serialize/deserialization. Typically these are generated
    // by macros, but we need custom implementation
    const VARIANT_JSON: &'static str = "json";
    const VARIANT_FORM_URLENCODED: &'static str = "form_urlencoded";
    const VARIANT_FORM_MULTIPART: &'static str = "form_multipart";
    const ALL_VARIANTS: &'static [&'static str] = &[
        Self::VARIANT_JSON,
        Self::VARIANT_FORM_URLENCODED,
        Self::VARIANT_FORM_MULTIPART,
    ];
}

// Custom deserialization for RecipeBody, to support raw template or structured
// body with a tag
impl<'de> Deserialize<'de> for RecipeBody {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RecipeBodyVisitor;

        /// For all primitives, parse it as a template and create a raw body
        macro_rules! visit_primitive {
            ($func:ident, $type:ty) => {
                fn $func<E>(self, v: $type) -> Result<Self::Value, E>
                where
                    E: de::Error,
                {
                    let template = v.to_string().parse().map_err(E::custom)?;
                    Ok(RecipeBody::Raw(template))
                }
            };
        }

        impl<'de> Visitor<'de> for RecipeBodyVisitor {
            type Value = RecipeBody;

            fn expecting(
                &self,
                formatter: &mut std::fmt::Formatter,
            ) -> std::fmt::Result {
                // "!<type>" is a little wonky, but tags aren't a common YAML
                // syntax so we should provide a hint to the user about what it
                // means. Once they provide a tag they'll get a different error
                // message if it's an unsupported tag
                formatter.write_str("string, boolean, number, or tag !<type>")
            }

            visit_primitive!(visit_bool, bool);
            visit_primitive!(visit_u64, u64);
            visit_primitive!(visit_u128, u128);
            visit_primitive!(visit_i64, i64);
            visit_primitive!(visit_i128, i128);
            visit_primitive!(visit_f64, f64);
            visit_primitive!(visit_str, &str);

            fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
            where
                A: EnumAccess<'de>,
            {
                let (tag, value) = data.variant::<String>()?;
                match tag.as_str() {
                    RecipeBody::VARIANT_JSON => {
                        // Deserialize to regular JSON
                        let json: serde_json::Value =
                            value.newtype_variant()?;
                        // Parse strings as templates
                        let json = json.try_into().map_err(A::Error::custom)?;
                        Ok(RecipeBody::Json(json))
                    }
                    RecipeBody::VARIANT_FORM_URLENCODED => {
                        Ok(RecipeBody::FormUrlencoded(value.newtype_variant()?))
                    }
                    RecipeBody::VARIANT_FORM_MULTIPART => {
                        Ok(RecipeBody::FormMultipart(value.newtype_variant()?))
                    }
                    other => Err(A::Error::unknown_variant(
                        other,
                        RecipeBody::ALL_VARIANTS,
                    )),
                }
            }
        }

        deserializer.deserialize_any(RecipeBodyVisitor)
    }
}
