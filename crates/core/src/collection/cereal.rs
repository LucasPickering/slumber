//! Serialization/deserialization helpers for various types

use crate::collection::{
    recipe_tree::RecipeNode, Profile, ProfileId, Recipe, RecipeId,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{de, Deserialize, Deserializer};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_err;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde::Serialize;
    use serde_json::json;
    use serde_test::{
        assert_de_tokens, assert_de_tokens_error, assert_ser_tokens, Token,
    };
    use serde_yaml::{
        value::{Tag, TaggedValue},
        Mapping,
    };
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
        RecipeBody::Raw { body: "{{user_id}}".into(), content_type: None },
        "{{user_id}}"
    )]
    #[case::json(
        RecipeBody::Raw {
            body: serde_json::to_string_pretty(&json!({"user": "{{user_id}}"}))
                .unwrap()
                .into(),
            content_type: Some(ContentType::Json),
        },
        serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("json"),
            value: mapping([("user", "{{user_id}}")])
        })),
    )]
    #[case::json_nested(
        RecipeBody::Raw {
            body: serde_json::to_string_pretty(
                &json!(r#"{"warning": "NOT an object"}"#)
            ).unwrap().into(),
            content_type: Some(ContentType::Json),
        },
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
    #[case::days("2d", Duration::from_secs(172800))]
    fn test_deserialize_duration(
        #[case] s: &'static str,
        #[case] expected: Duration,
    ) {
        assert_de_tokens(&WrapDuration(expected), &[Token::Str(s)])
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
        assert_de_tokens_error::<WrapDuration>(&[Token::Str(s)], error)
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
