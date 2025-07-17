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
        resolve::{ReferenceError, ResolveReferences},
    },
    http::HttpMethod,
};
use indexmap::IndexMap;
use itertools::Itertools;
use saphyr::{
    AnnotatedMapping, LoadableYamlNode, MarkedYaml, Marker, Scalar, ScanError,
    YamlData,
};
use slumber_template::Template;
use std::path::Path;
use thiserror::Error;

const TYPE_FIELD: &str = "type";

type Result<T> = std::result::Result<T, LocatedError<Error>>;

/// Parse and deserialize a YAML string into a [Collection]
///
/// This uses [saphyr] to parse the string into a YAML document, then uses
/// custom deserialization logic to deserialize the YAML into the collection
/// data types. We do this rather than use serde_yaml because it provides:
/// - Better error messages
/// - Source span tracking
///
/// The given path is used only for error context. The data must already be
/// loaded out of the file prior to calling.
///
/// ## Params
///
/// - `yaml_input`: YAML string to parse and deserialize
/// - `path`: File that the YAML was loaded from. `None` for tests that define
///   YAML inline
pub fn deserialize_collection(
    yaml_input: &str,
    path: Option<&Path>,
) -> anyhow::Result<Collection> {
    fn deserialize(mut yaml: MarkedYaml<'static>) -> Result<Collection> {
        // Resolve $ref keys before deserializing
        yaml.resolve_references().map_err(|error| LocatedError {
            error: Error::Reference(error.error),
            location: error.location,
        })?;
        Collection::deserialize(yaml)
    }

    let mut documents = MarkedYaml::load_from_str(yaml_input)?;

    // If the file is empty, pretend there's an empty mapping instead
    // because that's functionally equivalent
    let yaml = documents
        .pop()
        .unwrap_or(YamlData::Mapping(Default::default()).into());

    deserialize(yaml).map_err(
        |LocatedError {
             error: kind,
             location,
         }| {
            // Display the error with path and location
            anyhow::Error::from(kind).context(format!(
                "Error at {path}:{line}:{col}",
                path = path.unwrap_or(Path::new("")).display(),
                line = location.line(),
                col = location.col(),
            ))
        },
    )
}

/// Deserialize from YAML into the implementing type
trait DeserializeYaml: Sized {
    /// What kind of YAML value do we expect to see?
    fn expected() -> Expected;

    /// Deserialize the given YAML value into this type
    fn deserialize(yaml: MarkedYaml) -> Result<Self>;
}

/// Implement [DeserializeYaml] for a type `T` via type `U`, where `T: From<U>,
/// U: DeserializeYaml`
macro_rules! impl_deserialize_from {
    ($t:ty, $u:ty) => {
        impl DeserializeYaml for $t {
            fn expected() -> Expected {
                <$u as DeserializeYaml>::expected()
            }

            fn deserialize(yaml: MarkedYaml) -> Result<Self> {
                <$u as DeserializeYaml>::deserialize(yaml).map(<$t>::from)
            }
        }
    };
}

/// Deserialize a YAML value as an internally tagged enum. The `type` field will
/// contain the variant, and all other fields in the mapping will be
/// deserialized using the given function.
macro_rules! deserialize_enum {
    ($yaml:expr, $($tag:literal => $f:expr),* $(,)?) => {
            const EXPECTED: Expected =
                Expected::OneOf(&[$(&Expected::Literal($tag),)*]);

            // Find the enum variant based on the `type` field
            let span = $yaml.span;
            let mut mapping = $yaml.try_into_mapping()?;
            let kind_yaml = mapping
                .remove(&MarkedYaml::value_from_str(TYPE_FIELD))
                .ok_or(LocatedError {
                    error: Error::MissingField {
                        field: TYPE_FIELD,
                        expected: EXPECTED,
                    },
                    location: span.start,
                })?;
            let kind_location = kind_yaml.span.start;
            let kind = kind_yaml.try_into_string()?;

            // Deserialize the rest of the mapping as the specified enum variant
            let yaml = MarkedYaml {
                data: YamlData::Mapping(mapping),
                span,
            };
            match kind.as_str() {
                $($tag => $f(yaml),)*
                // Unknown tag
                _ => Err(LocatedError {
                    error: Error::Unexpected {
                        expected: EXPECTED,
                        actual: format!("{kind:?}"),
                    },
                    location: kind_location,
                }),
            }
    };
}

impl_deserialize_from!(ProfileId, String);
impl_deserialize_from!(RecipeId, String);

impl DeserializeYaml for Collection {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        let mut deserializer = StructDeserializer::new(yaml)?;

        // Drop all fields starting with `.`
        deserializer.mapping.retain(|key, _| {
            !key.data.as_str().is_some_and(|s| s.starts_with('.'))
        });

        let collection = Collection {
            profiles: deserializer.get(Field::new("profiles").opt())?,
            // Internally we call these recipes, but extensive market research
            // shows that `requests` is more intuitive to the user
            recipes: deserializer.get(Field::new("requests").opt())?,
        };
        deserializer.done()?;
        Ok(collection)
    }
}

/// Deserialize a map of profiles. This needs a custom implementation because:
/// - To call [HasId::set_id] on each value
/// - We have to enforce that at most one profile is set as default
impl DeserializeYaml for IndexMap<ProfileId, Profile> {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        // Enforce that only one profile can be the default
        let mut default_profile: Option<ProfileId> = None;

        yaml.try_into_mapping()?
            .into_iter()
            .map(|(k, v)| {
                let value_location = v.span.start;
                let key = ProfileId::deserialize(k)?;
                let mut value = Profile::deserialize(v)?;
                value.set_id(key.clone());

                // Check if another profile is already the default
                if value.default {
                    if let Some(default) = default_profile.take() {
                        return Err(LocatedError {
                            error: Error::MultipleDefaultProfiles {
                                first: default,
                                second: key,
                            },
                            location: value_location,
                        });
                    }

                    default_profile = Some(key.clone());
                }

                Ok((key, value))
            })
            .collect()
    }
}

impl DeserializeYaml for Profile {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        let mut deserializer = StructDeserializer::new(yaml)?;
        let profile = Self {
            id: ProfileId::default(), // Will be set by parent based on key
            name: deserializer.get(Field::new("name").opt())?,
            default: deserializer.get(Field::new("default").opt())?,
            data: deserializer.get(Field::new("data").opt())?,
        };
        deserializer.done()?;
        Ok(profile)
    }
}

impl DeserializeYaml for RecipeTree {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        let location = yaml.span.start;
        let recipes = IndexMap::deserialize(yaml)?;
        // Build a tree from the map
        RecipeTree::new(recipes)
            .map_err(|error| LocatedError::other(error, location))
    }
}

/// Deserialize a map of profiles. This needs a custom implementation to call
/// [HasId::set_id] on each value. This is used for both the root `requests`
/// field and each inner folder.
impl DeserializeYaml for IndexMap<RecipeId, RecipeNode> {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        yaml.try_into_mapping()?
            .into_iter()
            .map(|(k, v)| {
                let key = RecipeId::deserialize(k)?;
                let mut value = RecipeNode::deserialize(v)?;
                value.set_id(key.clone());
                Ok((key, value))
            })
            .collect()
    }
}

impl DeserializeYaml for RecipeNode {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        deserialize_enum! {
            yaml,
            "request" => |yaml| {
                Recipe::deserialize(yaml).map(RecipeNode::Recipe)
            },
            "folder" => |yaml| {
                Folder::deserialize(yaml).map(RecipeNode::Folder)
            },
        }
    }
}

impl DeserializeYaml for Recipe {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        let mut deserializer = StructDeserializer::new(yaml)?;
        let recipe = Recipe {
            id: RecipeId::default(), // Will be set by parent based on key
            name: deserializer.get(Field::new("name").opt())?,
            persist: deserializer.get(Field::new("persist").or(true))?,
            method: deserializer.get(Field::new("method"))?,
            url: deserializer.get(Field::new("url"))?,
            body: deserializer.get(Field::new("body").opt())?,
            authentication: deserializer
                .get(Field::new("authentication").opt())?,
            query: deserializer.get(Field::new("query").opt())?,
            // Lower-case all headers for consistency. HTTP/1.1 headers are
            // case-insensitive and HTTP/2 enforces lower casing.
            headers: deserializer
                .get(Field::<IndexMap<String, Template>>::new("headers").opt())?
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

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        let mut deserializer = StructDeserializer::new(yaml)?;
        let folder = Folder {
            id: RecipeId::default(), // Will be set by parent based on key
            name: deserializer.get(Field::new("name").opt())?,
            // `requests` matches the root field name
            children: deserializer.get(Field::new("requests").opt())?,
        };
        deserializer.done()?;
        Ok(folder)
    }
}

impl DeserializeYaml for HttpMethod {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        let marker = yaml.span.start;
        let s = String::deserialize(yaml)?;
        s.parse()
            .map_err(|error| LocatedError::other(error, marker))
    }
}

impl DeserializeYaml for QueryParameterValue {
    fn expected() -> Expected {
        Expected::OneOf(&[&Expected::String, &Expected::Sequence])
    }

    /// Deserialize from a single template or a list of templates
    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        if yaml.is_sequence() {
            // Deserialize vec
            DeserializeYaml::deserialize(yaml).map(Self::Many)
        } else {
            // Deserialize template
            DeserializeYaml::deserialize(yaml).map(Self::One)
        }
    }
}

impl DeserializeYaml for Authentication {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        deserialize_enum! {
            yaml,
            "basic" => |yaml: MarkedYaml| {
                let mut deserializer = StructDeserializer::new(yaml)?;
                Ok(Authentication::Basic {
                    username: deserializer.get(Field::new("username"))?,
                    password: deserializer.get(Field::new("password").opt())?,
                })
            },
            "bearer" => |yaml: MarkedYaml| {
                let mut deserializer = StructDeserializer::new(yaml)?;
                Ok(Authentication::Bearer {
                    token: deserializer.get(Field::new("token"))?,
                })
            },
        }
    }
}

impl DeserializeYaml for RecipeBody {
    fn expected() -> Expected {
        Expected::OneOf(&[&Expected::String, &Expected::Mapping])
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        // Mapping deserializes as some sort of structured body. It should have
        // a `type` and `data` field
        if yaml.is_mapping() {
            deserialize_enum! {
                yaml,
                "json" => |yaml| {
                    let mut deserializer = StructDeserializer::new(yaml)?;
                    let json = deserializer.get(Field::new("data"))?;
                    deserializer.done()?;
                    Ok(Self::Json(json))
                },
                "form_urlencoded" => |yaml| {
                    let mut deserializer = StructDeserializer::new(yaml)?;
                    let form = deserializer.get(Field::new("data"))?;
                    deserializer.done()?;
                    Ok(Self::FormUrlencoded(form))
                },
                "form_multipart" => |yaml| {
                    let mut deserializer = StructDeserializer::new(yaml)?;
                    let form = deserializer.get(Field::new("data"))?;
                    deserializer.done()?;
                    Ok(Self::FormMultipart(form))
                },
            }
        } else {
            // Otherwise it's a raw body - deserialize as a template
            Template::deserialize(yaml).map(Self::Raw)
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

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        match yaml.data {
            YamlData::Representation(_, _, _)
            | YamlData::BadValue
            | YamlData::Alias(_) => yaml_parse_panic(),
            YamlData::Value(Scalar::Null) => Ok(Self::Null),
            YamlData::Value(Scalar::Boolean(b)) => Ok(Self::Bool(b)),
            YamlData::Value(Scalar::Integer(i)) => Ok(Self::Number(i.into())),
            YamlData::Value(Scalar::FloatingPoint(f)) => {
                Ok(Self::Number(serde_json::Number::from_f64(f.0).ok_or_else(
                    || LocatedError {
                        error: Error::InvalidJsonFloat(f.0),
                        location: yaml.span.start,
                    },
                )?))
            }
            // Parse string as a template
            YamlData::Value(Scalar::String(s)) => {
                let template = s.parse::<Template>().map_err(|error| {
                    LocatedError::other(error, yaml.span.start)
                })?;
                Ok(Self::String(template))
            }
            YamlData::Sequence(sequence) => {
                let values = sequence
                    .into_iter()
                    .map(Self::deserialize)
                    .collect::<Result<_>>()?;
                Ok(Self::Array(values))
            }
            YamlData::Mapping(mapping) => {
                let fields = mapping
                    .into_iter()
                    .map(|(key, value)| {
                        let key = key.try_into_string()?;
                        let value = Self::deserialize(value)?;
                        Ok((key, value))
                    })
                    .collect::<Result<_>>()?;
                Ok(Self::Object(fields))
            }
            YamlData::Tagged(_, _) => {
                Err(LocatedError::unexpected(Self::expected(), yaml))
            }
        }
    }
}

impl DeserializeYaml for Template {
    fn expected() -> Expected {
        Expected::OneOf(&[
            &Expected::String,
            &Expected::Null,
            &Expected::Boolean,
            &Expected::Number,
        ])
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        if let YamlData::Value(scalar) = yaml.data {
            // Accept any scalar for a template. We'll treat everything as the
            // equivalent string representation
            match scalar {
                Scalar::Null => "null".parse(),
                Scalar::Boolean(b) => b.to_string().parse(),
                Scalar::Integer(i) => i.to_string().parse(),
                Scalar::FloatingPoint(f) => f.to_string().parse(),
                Scalar::String(s) => s.parse(),
            }
            .map_err(|error| LocatedError::other(error, yaml.span.start))
        } else {
            Err(LocatedError::unexpected(Expected::String, yaml))
        }
    }
}

impl DeserializeYaml for bool {
    fn expected() -> Expected {
        Expected::Boolean
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        yaml.try_into_bool()
    }
}

impl DeserializeYaml for String {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        yaml.try_into_string()
    }
}

impl<T: DeserializeYaml> DeserializeYaml for Option<T> {
    fn expected() -> Expected {
        // Techinically we should include `null` here too, but generally
        // optional fields should just be omitted instead of being set to null.
        // It also makes the lifetimes and type signatures on Expected much more
        // complicated to dynamically build one that's not 'static
        T::expected()
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        if yaml.data.is_null() {
            Ok(None)
        } else {
            T::deserialize(yaml).map(Some)
        }
    }
}

impl<T> DeserializeYaml for Vec<T>
where
    T: DeserializeYaml,
{
    fn expected() -> Expected {
        Expected::Sequence
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        let sequence = yaml.try_into_sequence()?;
        sequence.into_iter().map(T::deserialize).collect()
    }
}

/// Deserialize a plain map with string keys
impl<V> DeserializeYaml for IndexMap<String, V>
where
    V: DeserializeYaml,
{
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: MarkedYaml) -> Result<Self> {
        yaml.try_into_mapping()?
            .into_iter()
            .map(|(k, v)| Ok((k.try_into_string()?, V::deserialize(v)?)))
            .collect()
    }
}

/// Extension trait to add fallible conversion methods to [MarkedYaml]
trait MarkedYamlExt<'a>: Sized {
    /// Unpack the YAML as a boolean
    fn try_into_bool(self) -> Result<bool>;

    /// Unpack the YAML as a string
    fn try_into_string(self) -> Result<String>;

    /// Unpack the YAML as a sequence
    fn try_into_sequence(self) -> Result<Vec<Self>>;

    /// Unpack the YAML as a mapping
    fn try_into_mapping(self) -> Result<AnnotatedMapping<'a, Self>>;
}

impl<'a> MarkedYamlExt<'a> for MarkedYaml<'a> {
    fn try_into_bool(self) -> Result<bool> {
        if let YamlData::Value(Scalar::Boolean(b)) = self.data {
            Ok(b)
        } else {
            Err(LocatedError::unexpected(Expected::Boolean, self))
        }
    }

    fn try_into_string(self) -> Result<String> {
        if let YamlData::Value(Scalar::String(s)) = self.data {
            Ok(s.into_owned())
        } else {
            Err(LocatedError::unexpected(Expected::String, self))
        }
    }

    fn try_into_sequence(self) -> Result<Vec<Self>> {
        if let YamlData::Sequence(sequence) = self.data {
            Ok(sequence)
        } else {
            Err(LocatedError::unexpected(Expected::Sequence, self))
        }
    }

    fn try_into_mapping(self) -> Result<AnnotatedMapping<'a, Self>> {
        if let YamlData::Mapping(mapping) = self.data {
            Ok(mapping)
        } else {
            Err(LocatedError::unexpected(Expected::Mapping, self))
        }
    }
}

/// An error paired with the source location in YAML where the error occurred
///
/// This type is internal to this module because there's no external application
/// for it beyond stringification.
#[derive(Debug, derive_more::Display)]
#[display("{error}")]
pub struct LocatedError<E> {
    /// Error that occurred
    pub error: E,
    /// Location of the error within the YAML file
    pub location: Marker,
}

impl LocatedError<Error> {
    /// Create a new [Other](Self::Other) from any error type
    fn other(
        error: impl 'static + std::error::Error + Send + Sync,
        location: Marker,
    ) -> Self {
        Self {
            error: Error::Other(Box::new(error)),
            location,
        }
    }

    /// Create a new [UnexpectedType](Self::UnexpectedType) from the expected
    /// type and actual value
    fn unexpected(expected: Expected, actual: MarkedYaml) -> Self {
        // Find a useful representation of the received value
        let actual_string = match actual.data {
            // Scalars are unlikely to be big so we can include the actual value
            YamlData::Value(Scalar::Null) => "null".into(),
            YamlData::Value(Scalar::Boolean(b)) => format!("`{b}`"),
            YamlData::Value(Scalar::Integer(i)) => format!("`{i}`"),
            YamlData::Value(Scalar::FloatingPoint(f)) => format!("`{f}`"),
            // Use debug format to get wrapping quotes
            YamlData::Value(Scalar::String(s)) => format!("{s:?}"),
            YamlData::Tagged(tag, _) => format!("tag `{tag}`"),
            // Collections could be large so just include the type
            YamlData::Sequence(_) => "sequence".into(),
            YamlData::Mapping(_) => "mapping".into(),
            YamlData::Representation(_, _, _)
            | YamlData::Alias(_)
            | YamlData::BadValue => yaml_parse_panic(),
        };
        Self {
            location: actual.span.start,
            error: Error::Unexpected {
                expected,
                actual: actual_string,
            },
        }
    }
}

impl From<ScanError> for LocatedError<Error> {
    fn from(error: ScanError) -> Self {
        Self {
            location: *error.marker(),
            error: Error::Scan(error),
        }
    }
}

/// An error that can occur while deserializing a YAML value
#[derive(Debug, Error)]
enum Error {
    /// JSON body contained a float value that isn't representable in JSON
    #[error("Invalid float `{0}`; JSON does not support NaN or Infinity")]
    InvalidJsonFloat(f64),

    #[error("Expected field `{field}` with {expected}")]
    MissingField {
        field: &'static str,
        expected: Expected,
    },

    #[error(
        "Cannot set profile `{second}` as default; `{first}` is already default"
    )]
    MultipleDefaultProfiles { first: ProfileId, second: ProfileId },

    /// External error type
    #[error(transparent)]
    Other(Box<dyn 'static + std::error::Error + Send + Sync>),

    /// Error parsing or resolving a reference under a `$ref` tag
    #[error(transparent)]
    Reference(ReferenceError),

    /// Error parsing YAML
    #[error(transparent)]
    Scan(saphyr::ScanError),

    /// Expected a particular type or value, but received something else
    #[error("Expected {expected}, received {actual}")]
    Unexpected {
        expected: Expected,
        /// Pre-formatted "actual" value. Getting an owned YAML value from
        /// is complicated so it's easier to store it as the presentation
        /// string
        actual: String,
    },

    #[error("Unexpected field `{0}`")]
    UnexpectedField(String),
}

/// When a value is expected but is either incorrect or missing, this type
/// allows the caller to declare what they expected to find
#[derive(Debug, derive_more::Display)]
enum Expected {
    /// Expected null
    #[display("null")]
    Null,
    /// Expected a string
    #[display("string")]
    String,
    /// Expected a boolean
    #[display("boolean")]
    Boolean,
    /// Expected a number
    #[display("number")]
    Number,
    /// Expected a sequence
    #[display("sequence")]
    Sequence,
    /// Expected a mapping
    #[display("mapping")]
    Mapping,
    /// Expected a string literal
    #[display("{_0:?}")]
    Literal(&'static str),
    /// Expected one of a static set of strings (for enum discriminants)
    #[display("one of {}", _0.iter().format(", "))]
    OneOf(&'static [&'static Self]),
}

/// Utility for deserializing a struct or enum variant from a YAML mapping.
/// Initialize this struct with a YAML value, and it will:
/// - Ensure the value is a mapping
/// - Enable deserializing individual fields with [get](Self::get)
/// - Ensure no unexpected fields were present with [done](Self::done)
///     - NOTE: `done` needs to be called manually after deserialization!
struct StructDeserializer<'a> {
    mapping: AnnotatedMapping<'a, MarkedYaml<'a>>,
    location: Marker,
}

impl<'a> StructDeserializer<'a> {
    fn new(yaml: MarkedYaml<'a>) -> Result<Self> {
        let location = yaml.span.start;
        let mapping = yaml.try_into_mapping()?;
        Ok(Self { mapping, location })
    }

    /// Deserialize a field from the mapping
    fn get<T: DeserializeYaml>(&mut self, field: Field<T>) -> Result<T> {
        if let Some(value) =
            self.mapping.remove(&MarkedYaml::value_from_str(field.name))
        {
            T::deserialize(value)
        } else if let Some(default) = field.default {
            Ok(default)
        } else {
            Err(LocatedError {
                error: Error::MissingField {
                    field: field.name,
                    expected: T::expected(),
                },
                location: self.location,
            })
        }
    }

    /// Check that no fields were unused
    fn done(mut self) -> Result<()> {
        if let Some((key, _)) = self.mapping.pop_front() {
            let key_location = key.span.start;
            // If the key isn't a string, it's reasonable to return a type error
            let key = key.try_into_string()?;
            Err(LocatedError {
                error: Error::UnexpectedField(key),
                location: key_location,
            })
        } else {
            Ok(())
        }
    }
}

/// A single deserializable field in a struct or enum variant. The field has a
/// static name, which corresponds to the name of the field *in the YAML*.
/// Generally this matches the internal field name, but not always. Fields are
/// required by default, but can be made optional with [opt](Self::opt) or
/// [or](Self::or).
struct Field<T> {
    name: &'static str,
    default: Option<T>,
}

impl<T> Field<T> {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            default: None,
        }
    }

    /// Pre-populate this field with `T`'s default value. If the field is not
    /// deserialized, the default value will be used instead.
    fn opt(mut self) -> Self
    where
        T: Default,
    {
        self.default = Some(T::default());
        self
    }

    /// Pre-populate this field with the given default value. If the field is
    /// not deserialized, the default value will be used instead.
    fn or(mut self, value: T) -> Self {
        self.default = Some(value);
        self
    }
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

/// There are a few variants of [YamlData] that are not possible to encounter
/// with the way we use the parser. They represent partially parsed data, while
/// we do full parsing before starting deserialization. Call this function in
/// `match` statements for these variants
#[track_caller]
pub fn yaml_parse_panic() -> ! {
    unreachable!("Invalid or incomplete YAML data")
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
    use slumber_util::assert_err;
    use std::iter;

    /// Test error cases for deserializing a profile map
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
        "Cannot set profile `profile2` as default; `profile1` is already default",
    )]
    fn test_deserialize_profiles_error(
        #[case] yaml: impl Into<serde_yaml::Value>,
        #[case] expected_error: &str,
    ) {
        assert_err!(
            deserialize_yaml::<IndexMap<ProfileId, Profile>>(yaml.into()),
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
        received \"raw\"",
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
            deserialize_yaml::<RecipeBody>(yaml.into()),
            expected_error
        );
    }

    /// Test deserializing an empty file. It should return an empty collection
    #[test]
    fn test_deserialize_empty() {
        assert_eq!(
            deserialize_collection("", None).unwrap(),
            Collection::default()
        );
    }

    /// Deserialize a [serde_yaml::Value] using saphyr. Serde values are easier
    /// to construct than saphyr values
    fn deserialize_yaml<T: DeserializeYaml>(
        yaml: serde_yaml::Value,
    ) -> Result<T> {
        let yaml_input = serde_yaml::to_string(&yaml).unwrap();
        let mut documents = MarkedYaml::load_from_str(&yaml_input)?;
        let yaml = documents.pop().unwrap();
        T::deserialize(yaml)
    }

    /// Helper for deserializing in RecipeTree's tests. We export this
    pub fn deserialize_recipe_tree(
        yaml: serde_yaml::Value,
    ) -> anyhow::Result<RecipeTree> {
        deserialize_yaml(yaml).map_err(|error| error.error.into())
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
            iter::once((TYPE_FIELD, serde_yaml::Value::from(type_)))
                .chain(fields.into_iter().map(|(k, v)| (k, v.into()))),
        )
    }
}
