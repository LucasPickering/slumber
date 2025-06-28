//! Serialization/deserialization helpers for various types

use crate::collection::{
    Profile, ProfileId, Recipe, RecipeId, recipe_tree::RecipeNode,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Deserialize, Deserializer, de};
use slumber_template::Template;
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

/*
 * TODO delete this
/// Custom serialize/deserialize for RecipeBody. This can't be an impl directly
/// on the type because we're just wrapping that type's impl to also support a
/// bare value as a raw body. RecipeBody is only used in one place so adding
/// this wrapper isn't a big deal.
pub mod serde_recipe_body {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use slumber_template::Template;

    use crate::collection::RecipeBody;

    #[expect(clippy::ref_option)]
    pub fn serialize<S>(
        body: &Option<RecipeBody>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match body {
            // Serialize a raw body as just the contained value
            Some(RecipeBody::Raw(data)) => data.serialize(serializer),
            // Serialize anything else as a tagged enum
            Some(body) => body.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<RecipeBody>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged, rename_all = "snake_case")]
        enum RecipeBodyWrapper {
            // The body variant must come first to give it higher priority,
            // because the raw variant will accept any expression
            RecipeBody(RecipeBody),
            Raw(Template),
        }

        // Support internally tagged enums for any body type. Also support bare
        // expressions for a raw body
        let wrapper = RecipeBodyWrapper::deserialize(deserializer)?;
        match wrapper {
            RecipeBodyWrapper::RecipeBody(body) => Ok(Some(body)),
            RecipeBodyWrapper::Raw(data) => Ok(Some(RecipeBody::Raw(data))),
        }
    }
}
*/

#[cfg(test)]
mod tests {
    use crate::collection::RecipeBody;

    use super::*;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use serde_yaml::Mapping;
    use slumber_util::assert_err;
    use std::iter;

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
        RecipeBody::Raw("{{ user_id }}".into()),
        "{{ user_id }}"
    )]
    #[case::json(
        RecipeBody::json(json!({"user": "{{ user_id }}"})).unwrap(),
        yaml_enum("json", [("data", mapping([("user", "{{ user_id }}")]))]),
    )]
    #[case::json_nested(
        RecipeBody::json(json!(r#"{"warning": "NOT an object"}"#)).unwrap(),
        yaml_enum("json", [("data", r#"{"warning": "NOT an object"}"#)]),
    )]
    #[case::form_urlencoded(
        RecipeBody::FormUrlencoded(indexmap! {
            "username".into() => "{{ username }}".into(),
            "password".into() => "{{ prompt('Password', sensitive=true) }}".into(),
        }),
        yaml_enum("form_urlencoded", [("data", mapping([
            ("username", "{{ username }}"),
            ("password", "{{ prompt('Password', sensitive=true) }}"),
        ]))]),
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
        "invalid type: sequence, expected string, boolean, or number"
    )]
    #[case::map(
        Mapping::default(),
        "invalid type: map, expected string, boolean, or number"
    )]
    // `Raw` variant is *not* accessible by tag
    #[case::raw_tag(
        yaml_enum("raw", [("data", "data")]),
        "unknown variant `raw`, expected one of \
        `json`, `form_urlencoded`, `form_multipart`",
    )]
    #[case::form_urlencoded_missing_data(
        yaml_enum("form_urlencoded", [] as [(_, serde_yaml::Value); 0]),
        "TODO"
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

    /// Build a YAML mapping with a `type` field
    fn yaml_enum(
        type_: &'static str,
        fields: impl IntoIterator<
            Item = (&'static str, impl Into<serde_yaml::Value>),
        >,
    ) -> serde_yaml::Value {
        mapping(
            iter::once(("type", serde_yaml::Value::from(type_)))
                .chain(fields.into_iter().map(|(k, v)| (k, v.into()))),
        )
    }
}
