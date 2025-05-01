//! Serialization/deserialization helpers for various types

use crate::collection::{
    Profile, ProfileId, Recipe, RecipeBody, RecipeId, recipe_tree::RecipeNode,
};
use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use slumber_util::{HasId, deserialize_id_map};

impl<P> HasId for Profile<P> {
    type Id = ProfileId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

impl<P> HasId for RecipeNode<P> {
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

impl<P> HasId for Recipe<P> {
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

/// Deserialize a profile mapping. This also enforces that only one profile is
/// marked as default
pub fn deserialize_profiles<'de, D, P>(
    deserializer: D,
) -> Result<IndexMap<ProfileId, Profile<P>>, D::Error>
where
    D: Deserializer<'de>,
    P: Deserialize<'de>,
{
    let profiles: IndexMap<ProfileId, Profile<P>> =
        deserialize_id_map(deserializer)?;

    // Make sure at most one profile is the default
    let is_default = |profile: &&Profile<P>| profile.default;

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
pub fn deserialize_headers<'de, D, P>(
    deserializer: D,
) -> Result<IndexMap<String, P>, D::Error>
where
    D: Deserializer<'de>,
    P: Deserialize<'de>,
{
    // This involves an extra allocation, but it makes the logic a lot easier.
    // These maps should be small anyway
    let headers: IndexMap<String, P> = IndexMap::deserialize(deserializer)?;
    Ok(headers
        .into_iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
        .collect())
}

/// Custom serialize/deserialize for RecipeBody. This can't be an impl directly
/// on the type because we're just wrapping that type's impl to also support a
/// bare value as a raw body. RecipeBody is only used in one place so adding
/// this wrapper isn't a big deal.
pub mod serde_recipe_body {
    use super::*;

    pub fn serialize<S, P>(
        body: &Option<RecipeBody<P>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        P: Serialize,
    {
        match body {
            // Serialize a raw body as just the contained value
            Some(RecipeBody::Raw { data }) => data.serialize(serializer),
            // Serialize anything else as a tagged enum
            Some(body) => body.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D, P>(
        deserializer: D,
    ) -> Result<Option<RecipeBody<P>>, D::Error>
    where
        D: Deserializer<'de>,
        P: Deserialize<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged, rename_all = "camelCase")]
        enum RecipeBodyWrapper<P> {
            RecipeBody(RecipeBody<P>),
            Raw(P),
        }

        // TODO explain
        let wrapper = RecipeBodyWrapper::deserialize(deserializer)?;
        match wrapper {
            RecipeBodyWrapper::RecipeBody(body) => Ok(Some(body)),
            RecipeBodyWrapper::Raw(data) => Ok(Some(RecipeBody::Raw { data })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petitscript::Value;
    use rstest::rstest;
    use slumber_util::assert_err;

    #[rstest]
    #[case::multiple_default(
        [
            ("profile1", Value::from([
                ("default", Value::Boolean(true)),
                ("data", [("a", "1")].into())
            ])),
            ("profile2", Value::from([
                ("default", Value::Boolean(true)),
                ("data", [("a", "2")].into())
            ])),
        ],
        "Only one profile can be the default, but multiple were: \
        profile1, profile2",
    )]
    fn test_deserialize_profiles_error(
        #[case] value: impl Into<Value>,
        #[case] expected_error: &str,
    ) {
        #[derive(Debug, Deserialize)]
        #[serde(transparent)]
        struct Wrap(
            #[allow(dead_code)]
            #[serde(deserialize_with = "deserialize_profiles")]
            IndexMap<ProfileId, Profile>,
        );

        let value = value.into();
        assert_err!(
            petitscript::serde::from_value::<Wrap>(value),
            expected_error
        );
    }
}
