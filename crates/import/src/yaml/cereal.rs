//! Serialization/deserialization helpers for various types

use crate::yaml::{
    collection::{Chain, ChainId, Profile, Recipe, RecipeBody, RecipeNode},
    template::Template,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{
    Deserialize, Deserializer,
    de::{
        self, EnumAccess, Error as _, MapAccess, SeqAccess, VariantAccess,
        Visitor,
    },
};
use slumber_core::collection::{ProfileId, RecipeId};
use slumber_util::{HasId, deserialize_id_map};

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
                .map(|profile| &profile.id)
                .format(", ")
        )));
    }

    Ok(profiles)
}

/// Deserialize query parameters from either a sequence of `key=value` or a map
/// of `key: value`
pub fn deserialize_query_parameters<'de, D>(
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
    // Constants for deserialization. Typically these are generated
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
                        let json: serde_json::Value =
                            value.newtype_variant()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use serde_test::{Token, assert_de_tokens};
    use serde_yaml::{
        Mapping,
        value::{Tag, TaggedValue},
    };
    use slumber_util::assert_err;

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
            #[allow(dead_code)]
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
        RecipeBody::Json(json!({"user": "{{user_id}}"})),
        serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("json"),
            value: mapping([("user", "{{user_id}}")])
        })),
    )]
    #[case::json_nested(
        RecipeBody::Json(json!(r#"{"warning": "NOT an object"}"#)),
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
    fn test_deserialize_recipe_body(
        #[case] body: RecipeBody,
        #[case] yaml: impl Into<serde_yaml::Value>,
    ) {
        let yaml = yaml.into();
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
    )]
    #[case::map(
        &[
            Token::Map { len: None },
            Token::Str("param"),
            Token::Str("{{value}}"),
            Token::MapEnd,
        ],
        vec![("param", "{{value}}")],
    )]
    #[case::unit(&[Token::Unit], vec![])]
    fn test_deserialize_query_parameters(
        #[case] input_tokens: &[Token],
        #[case] expected_value: Vec<(&'static str, &'static str)>,
    ) {
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(transparent)]
        struct Wrap(
            #[serde(deserialize_with = "deserialize_query_parameters")]
            Vec<(String, Template)>,
        );

        let expected_value = Wrap(
            expected_value
                .into_iter()
                .map(|(param, value)| (param.into(), value.into()))
                .collect(),
        );
        assert_de_tokens::<Wrap>(&expected_value, input_tokens);
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
