//! Serialization/deserialization helpers for various types

use crate::{
    collection::{
        Chain, ChainId, Profile, ProfileId, Recipe, RecipeBody, RecipeId,
        recipe_tree::RecipeNode,
    },
    template::Template,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, EnumAccess, Error as _, VariantAccess, Visitor},
};
use std::hash::Hash;

/// A type that has an `id` field. This is ripe for a derive macro, maybe a fun
/// project some day?
pub trait HasId {
    type Id: Clone + Eq + Hash;

    fn id(&self) -> &Self::Id;

    fn set_id(&mut self, id: Self::Id);
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

/// Default value generator for `Recipe::persist`. All recipes are persisted by
/// default
pub fn persist_default() -> bool {
    true
}

/// Deserialize a map, and update each key so its `id` field matches its key in
/// the map. Useful if you need to access the ID when you only have a value
/// available, not the full entry.
pub fn deserialize_id_map<'de, Map, V, D>(
    deserializer: D,
) -> Result<Map, D::Error>
where
    Map: Deserialize<'de>,
    for<'m> &'m mut Map: IntoIterator<Item = (&'m V::Id, &'m mut V)>,
    D: Deserializer<'de>,
    V: Deserialize<'de> + HasId,
    V::Id: Deserialize<'de>,
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
pub fn deserialize_profiles<'de, D>(
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

/// Deserialize query parameters from either a sequence of `key=value` or a map
/// of `key: value`. Serialie back to a sequence `key=value`, since that will
/// always support duplicate keys
pub mod serde_query_parameters {
    use crate::template::Template;
    use serde::{
        Deserializer, Serializer,
        de::{self, MapAccess, SeqAccess, Visitor},
        ser::SerializeSeq,
    };

    pub fn serialize<S>(
        query_parameters: &Vec<(String, Template)>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(query_parameters.len()))?;
        for (param, value) in query_parameters {
            seq.serialize_element(&format!("{param}={}", value.display()))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D>(
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

impl RecipeBody {
    // Constants for serialize/deserialization. Typically these are generated
    // by macros, but we need custom implementation
    const STRUCT_NAME: &'static str = "RecipeBody";
    const VARIANT_JSON: &'static str = "json";
    const VARIANT_FORM_URLENCODED: &'static str = "form_urlencoded";
    const VARIANT_FORM_MULTIPART: &'static str = "form_multipart";
    const ALL_VARIANTS: &'static [&'static str] = &[
        Self::VARIANT_JSON,
        Self::VARIANT_FORM_URLENCODED,
        Self::VARIANT_FORM_MULTIPART,
    ];
}

/// Custom serialization for RecipeBody, so the `Raw` variant serializes as a
/// scalar without a tag
impl Serialize for RecipeBody {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // This involves a lot of duplication, but any abstraction will probably
        // just make it worse
        match self {
            RecipeBody::Raw(body) => body.serialize(serializer),
            RecipeBody::Json(json_template) => {
                // Convert the JSON template back to regular JSON
                let json: serde_json::Value = json_template.into();
                serializer.serialize_newtype_variant(
                    Self::STRUCT_NAME,
                    1,
                    Self::VARIANT_JSON,
                    &json,
                )
            }
            RecipeBody::FormUrlencoded(value) => serializer
                .serialize_newtype_variant(
                    Self::STRUCT_NAME,
                    2,
                    Self::VARIANT_FORM_URLENCODED,
                    value,
                ),
            RecipeBody::FormMultipart(value) => serializer
                .serialize_newtype_variant(
                    Self::STRUCT_NAME,
                    3,
                    Self::VARIANT_FORM_MULTIPART,
                    value,
                ),
        }
    }
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
                        RecipeBody::json(json).map_err(A::Error::custom)
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

/// Serialize/deserialize a duration with unit shorthand. This does *not* handle
/// subsecond precision. Supported units are:
/// - s
/// - m
/// - h
/// - d
///
/// Examples: `30s`, `5m`, `12h`, `3d`
pub mod serde_duration {
    use derive_more::Display;
    use itertools::Itertools;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    use std::time::Duration;
    use strum::{EnumIter, EnumString, IntoEnumIterator};
    use winnow::{ModalResult, Parser, ascii::digit1, token::take_while};

    #[derive(Debug, Display, EnumIter, EnumString)]
    enum Unit {
        #[display("s")]
        #[strum(serialize = "s")]
        Second,
        #[display("m")]
        #[strum(serialize = "m")]
        Minute,
        #[display("h")]
        #[strum(serialize = "h")]
        Hour,
        #[display("d")]
        #[strum(serialize = "d")]
        Day,
    }

    pub fn serialize<S>(
        duration: &Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Always serialize as seconds, because it's easiest. Sub-second
        // precision is lost
        S::serialize_str(serializer, &format!("{}s", duration.as_secs()))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
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

        let unit = unit.parse().map_err(|_| {
            D::Error::custom(format!(
                "Unknown duration unit `{unit}`; must be one of {}",
                Unit::iter()
                    .format_with(", ", |unit, f| f(&format_args!("`{unit}`")))
            ))
        })?;
        let seconds = match unit {
            Unit::Second => quantity,
            Unit::Minute => quantity * 60,
            Unit::Hour => quantity * 60 * 60,
            Unit::Day => quantity * 60 * 60 * 24,
        };
        Ok(Duration::from_secs(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde::Serialize;
    use serde_json::json;
    use serde_test::{
        Token, assert_de_tokens, assert_de_tokens_error, assert_ser_tokens,
    };
    use serde_yaml::{
        Mapping,
        value::{Tag, TaggedValue},
    };
    use slumber_util::assert_err;
    use std::time::Duration;

    #[rstest]
    #[case::multiple_default(
        mapping([
            ("profile1", mapping([
                ("default", serde_yaml::Value::Bool(true)),
                ("data", mapping([("a", "1")]))
            ])),
            ("profile2", mapping([
                ("default", serde_yaml::Value::Bool(true)),
                ("data", mapping([("a", "2")]))
            ])),
        ]),
        "Only one profile can be the default, but multiple were: \
        profile1, profile2",
    )]
    fn test_deserialize_profiles_error(
        #[case] yaml: impl Into<serde_yaml::Value>,
        #[case] expected_error: &str,
    ) {
        #[derive(Debug, Deserialize)]
        #[serde(transparent)]
        struct Wrap(
            #[expect(dead_code)]
            #[serde(deserialize_with = "deserialize_profiles")]
            IndexMap<ProfileId, Profile>,
        );

        let yaml = yaml.into();
        assert_err!(serde_yaml::from_value::<Wrap>(yaml), expected_error);
    }

    /// Test serializing and deserializing recipe bodies. Round trips should all
    /// be no-ops. We use serde_yaml instead of serde_test because the handling
    /// of enums is a bit different, and we specifically only care about YAML.
    #[rstest]
    #[case::raw(
        RecipeBody::Raw("{{user_id}}".into()),
        "{{user_id}}"
    )]
    #[case::json(
        RecipeBody::json(json!({"user": "{{user_id}}"})).unwrap(),
        serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("json"),
            value: mapping([("user", "{{user_id}}")])
        })),
    )]
    #[case::json_nested(
        RecipeBody::json(json!(r#"{"warning": "NOT an object"}"#)).unwrap(),
        serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("json"),
            value: r#"{"warning": "NOT an object"}"#.into()
        })),
    )]
    #[case::form_urlencoded(
        RecipeBody::FormUrlencoded(indexmap! {
            "username".into() => "{{username}}".into(),
            "password".into() => "{{chains.password}}".into(),
        }),
        serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("form_urlencoded"),
            value: mapping([
                ("username", "{{username}}"),
                ("password", "{{chains.password}}"),
            ])
        }))
    )]
    fn test_serde_recipe_body(
        #[case] body: RecipeBody,
        #[case] yaml: impl Into<serde_yaml::Value>,
    ) {
        let yaml = yaml.into();
        assert_eq!(
            serde_yaml::to_value(&body).unwrap(),
            yaml,
            "Serialization mismatch"
        );
        assert_eq!(
            serde_yaml::from_value::<RecipeBody>(yaml).unwrap(),
            body,
            "Deserialization mismatch"
        );
    }

    /// Test various errors when deserializing a recipe body. We use serde_yaml
    /// instead of serde_test because the handling of enums is a bit different,
    /// and we specifically only care about YAML.
    #[rstest]
    #[case::array(
        Vec::<i32>::new(),
        "invalid type: sequence, expected string, boolean, number, or tag !<type>"
    )]
    #[case::map(
        Mapping::default(),
        "invalid type: map, expected string, boolean, number, or tag !<type>"
    )]
    // `Raw` variant is *not* accessible by tag
    #[case::raw_tag(
        serde_yaml::Value::Tagged(Box::new(TaggedValue{
            tag: Tag::new("raw"),
            value: "{{user_id}}".into()
        })),
        "unknown variant `raw`, expected one of \
        `json`, `form_urlencoded`, `form_multipart`",
    )]
    #[case::form_urlencoded_wrong_type(
        serde_yaml::Value::Tagged(Box::new(TaggedValue{
            tag: Tag::new("form_urlencoded"),
            value: "{{user_id}}".into()
        })),
        "invalid type: string \"{{user_id}}\", expected a map"
    )]
    fn test_deserialize_recipe_error(
        #[case] yaml: impl Into<serde_yaml::Value>,
        #[case] expected_error: &str,
    ) {
        assert_err!(
            serde_yaml::from_value::<RecipeBody>(yaml.into()),
            expected_error
        );
    }

    /// Test serializing/deserializing query parameters. There are multiple
    /// input formats but only one output format, so we have to test
    /// serialization and deserialization separately here
    #[rstest]
    #[case::list(
        &[
            Token::Seq { len: None },
            Token::Str("param={{value}}"),
            Token::Str("param=value"),
            Token::SeqEnd,
        ],
        vec![("param", "{{value}}"), ("param", "value")],
        &[
            Token::Seq { len: Some(2) },
            Token::Str("param={{value}}"),
            Token::Str("param=value"),
            Token::SeqEnd,
        ],
    )]
    #[case::map(
        &[
            Token::Map { len: None },
            Token::Str("param"),
            Token::Str("{{value}}"),
            Token::MapEnd,
        ],
        vec![("param", "{{value}}")],
        &[
            Token::Seq { len: Some(1) },
            Token::Str("param={{value}}"),
            Token::SeqEnd,
        ],
    )]
    #[case::unit(
        &[Token::Unit],
        vec![],
        &[Token::Seq { len: Some(0) }, Token::SeqEnd],
    )]
    fn test_deserialize_query_parameters(
        #[case] input_tokens: &[Token],
        #[case] expected_value: Vec<(&str, &str)>,
        #[case] expected_tokens: &[Token],
    ) {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        #[serde(transparent)]
        struct Wrap(
            #[serde(with = "serde_query_parameters")] Vec<(String, Template)>,
        );

        let expected_value = Wrap(
            expected_value
                .into_iter()
                .map(|(param, value)| (param.into(), value.into()))
                .collect(),
        );
        assert_de_tokens::<Wrap>(&expected_value, input_tokens);
        assert_ser_tokens(&expected_value, expected_tokens);
    }

    /// A wrapper that forces serde_test to use our custom serialize/deserialize
    /// functions
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(transparent)]
    struct WrapDuration(#[serde(with = "super::serde_duration")] Duration);

    #[rstest]
    #[case::seconds_short(Duration::from_secs(3), "3s")]
    #[case::seconds_long(Duration::from_secs(3000), "3000s")]
    // Subsecond precision is lost
    #[case::seconds_subsecond_lost(Duration::from_millis(400), "0s")]
    #[case::seconds_subsecond_round_down(Duration::from_millis(1999), "1s")]
    fn test_serialize_duration(
        #[case] duration: Duration,
        #[case] expected: &'static str,
    ) {
        assert_ser_tokens(&WrapDuration(duration), &[Token::String(expected)]);
    }

    #[rstest]
    #[case::seconds_zero("0s", Duration::from_secs(0))]
    #[case::seconds_short("1s", Duration::from_secs(1))]
    #[case::seconds_longer("100s", Duration::from_secs(100))]
    #[case::minutes("3m", Duration::from_secs(180))]
    #[case::hours("3h", Duration::from_secs(10800))]
    #[case::days("2d", Duration::from_secs(172_800))]
    fn test_deserialize_duration(
        #[case] s: &'static str,
        #[case] expected: Duration,
    ) {
        assert_de_tokens(&WrapDuration(expected), &[Token::Str(s)]);
    }

    #[rstest]
    #[case::negative(
        "-1s",
        "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)"
    )]
    #[case::whitespace(
        " 1s ",
        "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)"
    )]
    #[case::trailing_whitespace(
        "1s ",
        "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)"
    )]
    #[case::decimal(
        "3.5s",
        "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)"
    )]
    #[case::invalid_unit(
        "3hr",
        "Unknown duration unit `hr`; must be one of `s`, `m`, `h`, `d`"
    )]
    fn test_deserialize_duration_error(
        #[case] s: &'static str,
        #[case] error: &str,
    ) {
        assert_de_tokens_error::<WrapDuration>(&[Token::Str(s)], error);
    }

    /// Build a YAML mapping
    fn mapping(
        fields: impl IntoIterator<
            Item = (&'static str, impl Into<serde_yaml::Value>),
        >,
    ) -> serde_yaml::Value {
        fields
            .into_iter()
            .map(|(k, v)| (serde_yaml::Value::from(k), v.into()))
            .collect::<Mapping>()
            .into()
    }
}
