//! Utilities for deserializing YAML. This does *not* use serde, and instead
//! relies on [saphyr] for YAML parsing and hand-written deserialization. This
//! allows us to provide much better error messages, and also enables source
//! span tracking.
//!
//! This module only provides deserialization; serialization is still handled
//! by serde/serde_yaml, because there's no need for error messages and the
//! derive macros are sufficient to generate the corresponding YAML.

mod resolve;

pub use resolve::ResolveReferences;

use crate::yaml::resolve::ReferenceError;
use indexmap::IndexMap;
use itertools::Itertools;
use saphyr::{
    AnnotatedMapping, MarkedYaml, Marker, Scalar, ScanError, YamlData,
};
use thiserror::Error;

type Result<T> = std::result::Result<T, LocatedError<Error>>;

/// Deserialize from YAML into the implementing type
pub trait DeserializeYaml: Sized {
    /// What kind of YAML value do we expect to see?
    fn expected() -> Expected;

    /// Deserialize the given YAML value into this type
    fn deserialize(yaml: MarkedYaml) -> Result<Self>;
}

/// Implement [DeserializeYaml] for a type `T` via type `U`, where `T: From<U>,
/// U: DeserializeYaml`
#[macro_export]
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
#[macro_export]
macro_rules! deserialize_enum {
    ($yaml:expr, $($tag:literal => $f:expr),* $(,)?) => {
            const TYPE_FIELD: &str = "type";
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
pub trait MarkedYamlExt<'a>: Sized {
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
    pub fn other(
        error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
        location: Marker,
    ) -> Self {
        Self {
            error: Error::Other(error.into()),
            location,
        }
    }

    /// Create a new [UnexpectedType](Self::UnexpectedType) from the expected
    /// type and actual value
    pub fn unexpected(expected: Expected, actual: MarkedYaml) -> Self {
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
pub enum Error {
    #[error("Expected field `{field}` with {expected}")]
    MissingField {
        field: &'static str,
        expected: Expected,
    },

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
pub enum Expected {
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
pub struct StructDeserializer<'a> {
    pub mapping: AnnotatedMapping<'a, MarkedYaml<'a>>,
    pub location: Marker,
}

impl<'a> StructDeserializer<'a> {
    pub fn new(yaml: MarkedYaml<'a>) -> Result<Self> {
        let location = yaml.span.start;
        let mapping = yaml.try_into_mapping()?;
        Ok(Self { mapping, location })
    }

    /// Deserialize a field from the mapping
    pub fn get<T: DeserializeYaml>(&mut self, field: Field<T>) -> Result<T> {
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
    pub fn done(mut self) -> Result<()> {
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
pub struct Field<T> {
    name: &'static str,
    default: Option<T>,
}

impl<T> Field<T> {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            default: None,
        }
    }

    /// Pre-populate this field with `T`'s default value. If the field is not
    /// deserialized, the default value will be used instead.
    #[must_use]
    pub fn opt(mut self) -> Self
    where
        T: Default,
    {
        self.default = Some(T::default());
        self
    }

    /// Pre-populate this field with the given default value. If the field is
    /// not deserialized, the default value will be used instead.
    #[must_use]
    pub fn or(mut self, value: T) -> Self {
        self.default = Some(value);
        self
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

#[cfg(feature = "test")]
pub use test_util::*;

/// Test helpers
#[cfg(feature = "test")]
mod test_util {
    use super::{DeserializeYaml, MarkedYaml, Result};
    use saphyr::LoadableYamlNode;
    use std::iter;

    /// Deserialize a [serde_yaml::Value] using saphyr. Serde values are easier
    /// to construct than saphyr values
    pub fn deserialize_yaml<T: DeserializeYaml>(
        yaml: serde_yaml::Value,
    ) -> Result<T> {
        let yaml_input = serde_yaml::to_string(&yaml).unwrap();
        let mut documents = MarkedYaml::load_from_str(&yaml_input)?;
        let yaml = documents.pop().unwrap();
        T::deserialize(yaml)
    }

    /// Build a YAML mapping
    pub fn yaml_mapping(
        fields: impl IntoIterator<
            Item = (&'static str, impl Into<serde_yaml::Value>),
        >,
    ) -> serde_yaml::Value {
        fields
            .into_iter()
            .map(|(k, v)| (serde_yaml::Value::from(k), v.into()))
            .collect::<serde_yaml::Mapping>()
            .into()
    }

    /// Build a YAML mapping with a `type` field
    pub fn yaml_enum(
        type_: &'static str,
        fields: impl IntoIterator<
            Item = (&'static str, impl Into<serde_yaml::Value>),
        >,
    ) -> serde_yaml::Value {
        yaml_mapping(
            iter::once(("type", serde_yaml::Value::from(type_)))
                .chain(fields.into_iter().map(|(k, v)| (k, v.into()))),
        )
    }
}
