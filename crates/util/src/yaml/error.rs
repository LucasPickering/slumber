use crate::yaml::{
    SourceId, SourceIdLocation, SourceLocation, SourceMap, SourcedYaml,
    resolve::ReferenceError, yaml_parse_panic,
};
use itertools::Itertools;
use saphyr::{Scalar, ScanError, YamlData};
use std::{error::Error as StdError, io};
use thiserror::Error;

/// An error that can occur while deserializing a YAML value
#[derive(Debug, Error)]
pub enum YamlErrorKind {
    #[error("Error opening {source}")]
    Io {
        #[source]
        error: io::Error,
        source: String,
    },

    #[error("Expected field `{field}` with {expected}")]
    MissingField {
        field: &'static str,
        expected: Expected,
    },

    /// External error type
    #[error(transparent)]
    Other(Box<dyn 'static + StdError + Send + Sync>),

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

    /// Struct received an extra field
    #[error("Unexpected field `{0}`")]
    UnexpectedField(String),

    /// Special error case to identify the `<<` key. We want to report this in
    /// both static and dynamic mappings because the user almost definitely
    /// doesn't want the literal key `<<`.
    #[error("YAML merge syntax `<<` is not supported")]
    UnsupportedMerge,
}

/// An error from deserializing YAML paired with the source location in YAML
/// where the error occurred. The location has been resolved so that it contains
/// paths instead of source IDs.
#[derive(Debug, Error)]
#[error("Error at {location}")]
pub struct YamlError {
    #[source]
    pub kind: YamlErrorKind,
    pub location: SourceLocation,
}

/// An error paired with the source location in YAML where the error occurred
///
/// This doesn't implement `Error` because this isn't immediately displayable.
/// The location needs to be resolved to make this presentable.
#[derive(Debug)]
pub struct LocatedError<E> {
    /// Error that occurred
    pub error: E,
    /// Source location of the error. This is an *unresolved* location, meaning
    /// it contains a source ID instead of a source path.
    pub location: SourceIdLocation,
}

impl<E> LocatedError<E> {
    /// Move the inner error out
    pub fn into_error(self) -> E {
        self.error
    }
}

impl LocatedError<YamlErrorKind> {
    /// Create a new [Other](YamlErrorKind::Other) from any error type
    pub fn other(
        error: impl Into<Box<dyn StdError + Send + Sync>>,
        location: SourceIdLocation,
    ) -> Self {
        Self {
            error: YamlErrorKind::Other(error.into()),
            location,
        }
    }

    pub(super) fn scan(error: ScanError, source_id: SourceId) -> Self {
        let location =
            SourceIdLocation::from_marker(source_id, *error.marker());
        Self {
            error: YamlErrorKind::Scan(error),
            location,
        }
    }

    /// Resolve the source ID in the location to a path
    pub(super) fn resolve(self, source_map: &SourceMap) -> YamlError {
        YamlError {
            kind: self.error,
            location: self.location.resolve(source_map),
        }
    }

    /// Create a new [Unexpected](YamlErrorKind::Unexpected) from the expected
    /// type and actual value
    pub fn unexpected(expected: Expected, actual: SourcedYaml) -> Self {
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
            location: actual.location,
            error: YamlErrorKind::Unexpected {
                expected,
                actual: actual_string,
            },
        }
    }
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
    /// Expected an integer or float
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
    /// Expected one of a static set of types (for enum discriminants)
    #[display("one of {}", _0.iter().format(", "))]
    OneOf(&'static [&'static Self]),
}
