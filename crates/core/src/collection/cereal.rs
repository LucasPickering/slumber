//! Deserialization helpers for collection types. This does *not* use serde,
//! and instead relies on [saphyr] for YAML parsing and hand-written
//! deserialization. This allows us to provide much better error messages, and
//! also enables source span tracking.
//!
//! This module only provides deserialization; serialization is still handled
//! by serde/serde_yaml, because there's no need for error messages and the
//! derive macros are sufficient to generate the corresponding YAML.

use crate::{
    collection::{
        Authentication, Collection, Folder, JsonTemplate, Profile, ProfileId,
        QueryParameterValue, Recipe, RecipeBody, RecipeId, RecipeTree,
        recipe_tree::RecipeNode,
    },
    http::HttpMethod,
};
use indexmap::IndexMap;
use saphyr::{Scalar, YamlData};
use slumber_template::Template;
use slumber_util::{
    deserialize_enum, impl_deserialize_from,
    yaml::{
        self, DeserializeYaml, Expected, Field, LocatedError, SourceMap,
        SourcedYaml, StructDeserializer, yaml_parse_panic,
    },
};

impl_deserialize_from!(ProfileId, String);
impl_deserialize_from!(RecipeId, String);

impl DeserializeYaml for Collection {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let mut deserializer = StructDeserializer::new(yaml)?;

        // Drop all fields starting with `.`
        deserializer.mapping.retain(|key, _| {
            !key.data.as_str().is_some_and(|s| s.starts_with('.'))
        });

        let collection = Self {
            name: deserializer.get(Field::new("name").opt(), source_map)?,
            profiles: deserializer
                .get::<Adopt<_>>(Field::new("profiles").opt(), source_map)?
                .0,
            // Internally we call these recipes, but extensive market research
            // shows that `requests` is more intuitive to the user
            recipes: deserializer
                .get(Field::new("requests").opt(), source_map)?,
        };
        deserializer.done()?;
        Ok(collection)
    }
}

/// Deserialize a map of profiles. This needs a custom implementation because:
/// - To call [HasId::set_id] on each value
/// - We have to enforce that at most one profile is set as default
impl DeserializeYaml for Adopt<IndexMap<ProfileId, Profile>> {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        mut yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        yaml.drop_dot_fields();

        // Enforce that only one profile can be the default
        let mut default_profile: Option<ProfileId> = None;

        yaml.try_into_mapping()?
            .into_iter()
            .map(|(k, v)| {
                let value_location = v.location;
                let key = ProfileId::deserialize(k, source_map)?;
                let mut value = Profile::deserialize(v, source_map)?;
                value.set_id(key.clone());

                // Check if another profile is already the default
                if value.default {
                    if let Some(default) = default_profile.take() {
                        return Err(LocatedError::other(
                            CerealError::MultipleDefaultProfiles {
                                first: default,
                                second: key,
                            },
                            value_location,
                        ));
                    }

                    default_profile = Some(key.clone());
                }

                Ok((key, value))
            })
            .collect::<yaml::Result<_>>()
            .map(Adopt)
    }
}

impl DeserializeYaml for Profile {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location.resolve(source_map);
        let mut deserializer = StructDeserializer::new(yaml)?;
        let profile = Self {
            id: ProfileId::default(), // Will be set by parent based on key
            location,
            name: deserializer.get(Field::new("name").opt(), source_map)?,
            default: deserializer
                .get(Field::new("default").opt(), source_map)?,
            data: deserializer.get(Field::new("data").opt(), source_map)?,
        };
        deserializer.done()?;
        Ok(profile)
    }
}

impl DeserializeYaml for RecipeTree {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location;
        let recipes: Adopt<IndexMap<RecipeId, RecipeNode>> =
            Adopt::deserialize(yaml, source_map)?;
        // Build a tree from the map
        RecipeTree::new(recipes.0)
            .map_err(|error| LocatedError::other(error, location))
    }
}

/// Deserialize a map of profiles. This needs a custom implementation to call
/// [HasId::set_id] on each value. This is used for both the root `requests`
/// field and each inner folder.
impl DeserializeYaml for Adopt<IndexMap<RecipeId, RecipeNode>> {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        yaml.try_into_mapping()?
            .into_iter()
            .map(|(k, v)| {
                let key = RecipeId::deserialize(k, source_map)?;
                let mut value = RecipeNode::deserialize(v, source_map)?;
                value.set_id(key.clone());
                Ok((key, value))
            })
            .collect::<yaml::Result<_>>()
            .map(Adopt)
    }
}

impl DeserializeYaml for RecipeNode {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        // Recipe nodes are untagged enums. They're written very frequently,
        // have distinct required fields that we can key on, and there's minimal
        // risk that we'll need to add new variants. Forcing users to require a
        // tag on every node is annoying so we can just omit it.

        // Get a reference to the mapping without moving it
        let YamlData::Mapping(mapping) = &yaml.data else {
            return Err(LocatedError::unexpected(Expected::Mapping, yaml));
        };

        let has = |key| mapping.contains_key(&SourcedYaml::value_from_str(key));

        // Do a little heuristicking to guess what the variant is. This gives
        // slightly better error messages
        if has("method") || has("url") {
            Recipe::deserialize(yaml, source_map).map(RecipeNode::Recipe)
        } else if has("requests") {
            Folder::deserialize(yaml, source_map).map(RecipeNode::Folder)
        } else {
            Err(LocatedError::other(
                CerealError::UnknownRecipeNodeVariant,
                yaml.location,
            ))
        }
    }
}

impl DeserializeYaml for Recipe {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location.resolve(source_map);
        let mut deserializer = StructDeserializer::new(yaml)?;
        let recipe = Recipe {
            id: RecipeId::default(), // Will be set by parent based on key
            location,
            name: deserializer.get(Field::new("name").opt(), source_map)?,
            persist: deserializer
                .get(Field::new("persist").or(true), source_map)?,
            method: deserializer.get(Field::new("method"), source_map)?,
            url: deserializer.get(Field::new("url"), source_map)?,
            body: deserializer.get(Field::new("body").opt(), source_map)?,
            authentication: deserializer
                .get(Field::new("authentication").opt(), source_map)?,
            query: deserializer.get(Field::new("query").opt(), source_map)?,
            // Lower-case all headers for consistency. HTTP/1.1 headers are
            // case-insensitive and HTTP/2 enforces lower casing.
            headers: deserializer
                .get::<IndexMap<String, Template>>(
                    Field::new("headers").opt(),
                    source_map,
                )?
                .into_iter()
                .map(|(k, v)| (k.to_lowercase(), v))
                .collect(),
        };
        deserializer.done()?;
        Ok(recipe)
    }
}

impl DeserializeYaml for Folder {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location.resolve(source_map);
        let mut deserializer = StructDeserializer::new(yaml)?;
        let folder = Folder {
            id: RecipeId::default(), // Will be set by parent based on key
            location,
            name: deserializer.get(Field::new("name").opt(), source_map)?,
            // `requests` matches the root field name
            children: deserializer
                .get::<Adopt<_>>(Field::new("requests").opt(), source_map)?
                .0,
        };
        deserializer.done()?;
        Ok(folder)
    }
}

impl DeserializeYaml for HttpMethod {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location;
        let s = String::deserialize(yaml, source_map)?;
        s.parse()
            .map_err(|error| LocatedError::other(error, location))
    }
}

impl DeserializeYaml for QueryParameterValue {
    fn expected() -> Expected {
        Expected::OneOf(&[&Expected::String, &Expected::Sequence])
    }

    /// Deserialize from a single template or a list of templates
    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        if yaml.data.is_sequence() {
            // Deserialize vec
            DeserializeYaml::deserialize(yaml, source_map).map(Self::Many)
        } else {
            // Deserialize template
            DeserializeYaml::deserialize(yaml, source_map).map(Self::One)
        }
    }
}

impl DeserializeYaml for Authentication {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        deserialize_enum! {
            yaml,
            "basic" => |yaml: SourcedYaml| {
                let mut deserializer = StructDeserializer::new(yaml)?;
                Ok(Authentication::Basic {
                    username: deserializer.get(Field::new("username"), source_map)?,
                    password: deserializer.get(Field::new("password").opt(), source_map)?,
                })
            },
            "bearer" => |yaml: SourcedYaml| {
                let mut deserializer = StructDeserializer::new(yaml)?;
                Ok(Authentication::Bearer {
                    token: deserializer.get(Field::new("token"), source_map)?,
                })
            },
        }
    }
}

impl DeserializeYaml for RecipeBody {
    fn expected() -> Expected {
        Expected::OneOf(&[&Expected::String, &Expected::Mapping])
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        /// Deserialize a struct with a single "data" field
        fn deserialize_data<T: DeserializeYaml>(
            yaml: SourcedYaml<'_>,
            source_map: &SourceMap,
        ) -> yaml::Result<T> {
            let mut deserializer = StructDeserializer::new(yaml)?;
            let data = deserializer.get(Field::new("data"), source_map)?;
            deserializer.done()?;
            Ok(data)
        }

        // Mapping deserializes as some sort of structured body. It should have
        // a `type` and `data` field
        if yaml.data.is_mapping() {
            deserialize_enum! {
                yaml,
                "json" => |yaml| {
                    Ok(Self::Json(deserialize_data(yaml, source_map)?))
                },
                "form_urlencoded" => |yaml| {
                    Ok(Self::FormUrlencoded(deserialize_data(yaml, source_map)?))
                },
                "form_multipart" => |yaml| {
                    Ok(Self::FormMultipart(deserialize_data(yaml, source_map)?))
                },
                "stream" => |yaml| {
                    Ok(Self::Stream(deserialize_data(yaml, source_map)?))
                },
            }
        } else {
            // Otherwise it's a raw body - deserialize as a template
            Template::deserialize(yaml, source_map).map(Self::Raw)
        }
    }
}

impl DeserializeYaml for JsonTemplate {
    fn expected() -> Expected {
        Expected::OneOf(&[
            &Expected::Null,
            &Expected::Boolean,
            &Expected::Number,
            &Expected::String,
            &Expected::Sequence,
            &Expected::Mapping,
        ])
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        match yaml.data {
            YamlData::Representation(_, _, _)
            | YamlData::BadValue
            | YamlData::Alias(_) => yaml_parse_panic(),
            YamlData::Value(Scalar::Null) => Ok(Self::Null),
            YamlData::Value(Scalar::Boolean(b)) => Ok(Self::Bool(b)),
            YamlData::Value(Scalar::Integer(i)) => Ok(Self::Number(i.into())),
            YamlData::Value(Scalar::FloatingPoint(f)) => Ok(Self::Number(
                serde_json::Number::from_f64(f.0).ok_or_else(|| {
                    LocatedError::other(
                        CerealError::InvalidJsonFloat(f.0),
                        yaml.location,
                    )
                })?,
            )),
            // Parse string as a template
            YamlData::Value(Scalar::String(s)) => {
                let template = s.parse::<Template>().map_err(|error| {
                    LocatedError::other(error, yaml.location)
                })?;
                Ok(Self::String(template))
            }
            YamlData::Sequence(sequence) => {
                let values = sequence
                    .into_iter()
                    .map(|yaml| Self::deserialize(yaml, source_map))
                    .collect::<yaml::Result<_>>()?;
                Ok(Self::Array(values))
            }
            YamlData::Mapping(mapping) => {
                let fields = mapping
                    .into_iter()
                    .map(|(key, value)| {
                        let key = Template::deserialize(key, source_map)?;
                        let value = Self::deserialize(value, source_map)?;
                        Ok((key, value))
                    })
                    .collect::<yaml::Result<_>>()?;
                Ok(Self::Object(fields))
            }
            YamlData::Tagged(_, _) => {
                Err(LocatedError::unexpected(Self::expected(), yaml))
            }
        }
    }
}

/// A Slumber-specific error that can occur while deserializing a YAML value.
/// Generic YAML errors are defined in [slumber_util::yaml::YamlError]. This
/// only holds errors specific to collection deserialization.
#[derive(Debug, thiserror::Error)]
enum CerealError {
    /// JSON body contained a float value that isn't representable in JSON
    #[error("Invalid float `{0}`; JSON does not support NaN or Infinity")]
    InvalidJsonFloat(f64),

    #[error(
        "Cannot set profile `{second}` as default; `{first}` is already default"
    )]
    MultipleDefaultProfiles { first: ProfileId, second: ProfileId },

    /// We couldn't guess the variant of a recipe node based on its fields
    #[error(
        "Requests must have a `method` and `url` field; \
        folders must have a `requests` field"
    )]
    UnknownRecipeNodeVariant,
}

/// A type that has an `id` field. This is ripe for a derive macro, maybe a fun
/// project some day?
pub trait HasId {
    type Id;

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

/// Workaround for the orphan rule
#[derive(Debug, Default)]
struct Adopt<T>(T);

/// Predicate for skip_serializing_if
#[expect(clippy::trivially_copy_pass_by_ref)]
pub fn is_true(b: &bool) -> bool {
    *b
}

/// Predicate for skip_serializing_if
#[expect(clippy::trivially_copy_pass_by_ref)]
pub fn is_false(b: &bool) -> bool {
    !b
}

/// Expose this for RecipeTree's tests
#[cfg(test)]
pub use tests::deserialize_recipe_tree;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::RecipeBody;
    use indexmap::indexmap;
    use rstest::rstest;
    use serde_json::json;
    use serde_yaml::Mapping;
    use slumber_util::{
        assert_err,
        yaml::{YamlErrorKind, deserialize_yaml, yaml_enum, yaml_mapping},
    };

    /// Test error cases for deserializing a profile map
    #[rstest]
    #[case::multiple_default(
        yaml_mapping([
            ("profile1", yaml_mapping([
                ("default", serde_yaml::Value::Bool(true)),
                ("data", yaml_mapping([("a", "1")]))
            ])),
            ("profile2", yaml_mapping([
                ("default", serde_yaml::Value::Bool(true)),
                ("data", yaml_mapping([("a", "2")]))
            ])),
        ]),
        "Cannot set profile `profile2` as default; `profile1` is already default",
    )]
    fn test_deserialize_profiles_error(
        #[case] yaml: impl Into<serde_yaml::Value>,
        #[case] expected_error: &str,
    ) {
        assert_err!(
            deserialize_yaml::<Adopt<IndexMap<ProfileId, Profile>>>(
                yaml.into()
            )
            .map_err(LocatedError::into_error),
            expected_error
        );
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
        yaml_enum("json", [("data", yaml_mapping([("user", "{{ user_id }}")]))]),
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
        yaml_enum("form_urlencoded", [("data", yaml_mapping([
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
            deserialize_yaml::<RecipeBody>(yaml).unwrap(),
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
        "Expected string, received sequence"
    )]
    #[case::map(
        Mapping::default(),
        "Expected field `type` with one of \
        \"json\", \"form_urlencoded\", \"form_multipart\""
    )]
    // `Raw` variant is *not* accessible by tag
    #[case::raw_tag(
        yaml_enum("raw", [("data", "data")]),
        "Expected one of \"json\", \"form_urlencoded\", \"form_multipart\", \
        \"stream\", received \"raw\"",
    )]
    #[case::form_urlencoded_missing_data(
        yaml_enum("form_urlencoded", [] as [(_, serde_yaml::Value); 0]),
        "Expected field `data` with mapping"
    )]
    fn test_deserialize_recipe_body_error(
        #[case] yaml: impl Into<serde_yaml::Value>,
        #[case] expected_error: &str,
    ) {
        assert_err!(
            deserialize_yaml::<RecipeBody>(yaml.into())
                .map_err(LocatedError::into_error),
            expected_error
        );
    }

    /// Test deserializing an empty file. It should return an empty collection
    #[test]
    fn test_deserialize_empty() {
        assert_eq!(Collection::parse("").unwrap(), Collection::default());
    }

    /// Helper for deserializing in RecipeTree's tests. We export this
    pub fn deserialize_recipe_tree(
        yaml: serde_yaml::Value,
    ) -> Result<RecipeTree, YamlErrorKind> {
        deserialize_yaml(yaml).map_err(|error| error.error)
    }
}
