//! Serialization/deserialization helpers for various types

use crate::{
    collection::{
        Profile, ProfileId, Recipe, RecipeBody, RecipeId,
        recipe_tree::RecipeNode,
    },
    http::content_type::ContentType,
};
use anyhow::Context;
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, EnumAccess, Error as _, VariantAccess, Visitor},
    ser::Error as _,
};
use slumber_template::Template;
use std::{hash::Hash, str::FromStr};

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
            RecipeBody::Raw {
                body,
                content_type: None,
            } => body.serialize(serializer),
            RecipeBody::Raw {
                body,
                content_type: Some(ContentType::Json),
            } => {
                // Reparse the body as JSON. Since it came from JSOn originally,
                // it *shouldn't* fail to reparse
                let json = serde_json::Value::from_str(&body.display())
                    .context("Failed to reparse body as JSON")
                    .map_err(S::Error::custom)?;
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
                    Ok(RecipeBody::Raw {
                        body: template,
                        content_type: None,
                    })
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
                        // Pretty print the JSON now and parse it as a template.
                        // This is inefficient because we stringify the value
                        // then immediately clone it during the parse :(
                        let json: serde_json::Value =
                            value.newtype_variant()?;
                        let body = format!("{json:#}")
                            .parse()
                            .map_err(A::Error::custom)?;
                        Ok(RecipeBody::Raw {
                            body,
                            content_type: Some(ContentType::Json),
                        })
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
        RecipeBody::Raw { body: "{{ user_id }}".into(), content_type: None },
        "{{ user_id }}"
    )]
    #[case::json(
        RecipeBody::Raw {
            body: json!({"user": "{{ user_id }}"}).into(),
            content_type: Some(ContentType::Json),
        },
        serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("json"),
            value: mapping([("user", "{{ user_id }}")])
        })),
    )]
    #[case::json_nested(
        RecipeBody::Raw {
            body: json!(r#"{"warning": "NOT an object"}"#).into(),
            content_type: Some(ContentType::Json),
        },
        serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("json"),
            value: r#"{"warning": "NOT an object"}"#.into()
        })),
    )]
    #[case::form_urlencoded(
        RecipeBody::FormUrlencoded(indexmap! {
            "username".into() => "{{ username }}".into(),
            "password".into() => "{{ prompt('Password', sensitive=true) }}".into(),
        }),
        serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("form_urlencoded"),
            value: mapping([
                ("username", "{{ username }}"),
                ("password", "{{ prompt('Password', sensitive=true) }}"),
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
            value: "{{ user_id }}".into()
        })),
        "unknown variant `raw`, expected one of \
        `json`, `form_urlencoded`, `form_multipart`",
    )]
    #[case::form_urlencoded_wrong_type(
        serde_yaml::Value::Tagged(Box::new(TaggedValue{
            tag: Tag::new("form_urlencoded"),
            value: "{{ user_id }}".into()
        })),
        "invalid type: string \"{{ user_id }}\", expected a map"
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
