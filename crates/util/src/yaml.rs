//! Utilities for deserializing YAML. This does *not* use serde, and instead
//! relies on [saphyr] for YAML parsing and hand-written deserialization. This
//! allows us to provide much better error messages, and also enables source
//! span tracking.
//!
//! This module only provides deserialization; serialization is still handled
//! by serde/serde_yaml, because there's no need for error messages and the
//! derive macros are sufficient to generate the corresponding YAML.

mod error;
mod resolve;

pub use error::{Expected, LocatedError, YamlError, YamlErrorKind};
#[cfg(feature = "test")]
pub use test_util::*;

use indexmap::{IndexMap, IndexSet};
use saphyr::{
    AnnotatedMapping, AnnotatedNode, LoadableYamlNode, MarkedYaml, Marker,
    Scalar, YamlData,
};
use std::{
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, LocatedError<YamlErrorKind>>;

/// Load YAML from a file and deserialize it into type `T`.
///
/// This uses [saphyr] to parse the string into a YAML document, then uses
/// custom deserialization logic to deserialize the YAML into the collection
/// data types. We do this rather than use serde_yaml because it provides:
/// - Better error messages
/// - Source span tracking
pub fn deserialize_file<T>(path: &Path) -> std::result::Result<T, YamlError>
where
    T: DeserializeYaml,
{
    let mut source_map = SourceMap::default();
    // Parse YAML from the file
    SourcedYaml::load(path, source_map.add_source(path.to_owned()))
        // Resolve $ref keys before deserializing
        .and_then(|yaml| {
            yaml.resolve_references(&mut source_map).map_err(|error| {
                LocatedError {
                    error: YamlErrorKind::Reference(error.error),
                    location: error.location,
                }
            })
        })
        // Deserialize as T
        .and_then(|yaml| T::deserialize(yaml, &source_map))
        // Make the location presentable
        .map_err(|error| error.resolve(&source_map))
}

/// Parse a string into YAML, then deserialize it into `T`
pub fn deserialize_str<T>(yaml: &str) -> std::result::Result<T, YamlError>
where
    T: DeserializeYaml,
{
    deserialize(
        SourcedYaml::load_from_str(yaml, SourceId::Memory),
        SourceMap::default(),
    )
}

/// Inner helper to deserialize the result of parsing YAML
fn deserialize<T>(
    parse_result: Result<SourcedYaml>,
    mut source_map: SourceMap,
) -> std::result::Result<T, YamlError>
where
    T: DeserializeYaml,
{
    parse_result
        // Resolve $ref keys before deserializing
        .and_then(|yaml| {
            yaml.resolve_references(&mut source_map).map_err(|error| {
                LocatedError {
                    error: YamlErrorKind::Reference(error.error),
                    location: error.location,
                }
            })
        })
        // Deserialize as T
        .and_then(|yaml| T::deserialize(yaml, &source_map))
        // Make the location presentable
        .map_err(|error| error.resolve(&source_map))
}

/// Deserialize from YAML into the implementing type
pub trait DeserializeYaml: Sized {
    /// What kind of YAML value do we expect to see?
    fn expected() -> Expected;

    /// Deserialize the given YAML value into this type
    fn deserialize(yaml: SourcedYaml, source_map: &SourceMap) -> Result<Self>;
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

            fn deserialize(
                yaml: slumber_util::yaml::SourcedYaml,
                source_map: &slumber_util::yaml::SourceMap,
            ) -> slumber_util::yaml::Result<Self> {
                <$u as DeserializeYaml>::deserialize(yaml, source_map)
                    .map(<$t>::from)
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
            use $crate::yaml::{LocatedError, YamlErrorKind};

            const TYPE_FIELD: &str = "type";
            const EXPECTED: Expected =
                Expected::OneOf(&[$(&Expected::Literal($tag),)*]);

            // Find the enum variant based on the `type` field
            let location = $yaml.location;
            let mut mapping = $yaml.try_into_mapping()?;
            let kind_yaml = mapping
                .remove(&SourcedYaml::value_from_str(TYPE_FIELD))
                .ok_or(LocatedError {
                    error: YamlErrorKind::MissingField {
                        field: TYPE_FIELD,
                        expected: EXPECTED,
                    },
                    location,
                })?;
            let kind_location = kind_yaml.location;
            let kind = kind_yaml.try_into_string()?;

            // Deserialize the rest of the mapping as the specified enum variant
            let yaml = SourcedYaml {
                data: YamlData::Mapping(mapping),
                location,
            };
            match kind.as_str() {
                $($tag => $f(yaml),)*
                // Unknown tag
                _ => Err(LocatedError {
                    error: YamlErrorKind::Unexpected {
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

    fn deserialize(yaml: SourcedYaml, _source_map: &SourceMap) -> Result<Self> {
        yaml.try_into_bool()
    }
}

impl DeserializeYaml for usize {
    fn expected() -> Expected {
        Expected::Number
    }

    fn deserialize(yaml: SourcedYaml, _source_map: &SourceMap) -> Result<Self> {
        yaml.try_into_usize()
    }
}

impl DeserializeYaml for String {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(yaml: SourcedYaml, _source_map: &SourceMap) -> Result<Self> {
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

    fn deserialize(yaml: SourcedYaml, source_map: &SourceMap) -> Result<Self> {
        if yaml.data.is_null() {
            Ok(None)
        } else {
            T::deserialize(yaml, source_map).map(Some)
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

    fn deserialize(yaml: SourcedYaml, source_map: &SourceMap) -> Result<Self> {
        let sequence = yaml.try_into_sequence()?;
        sequence
            .into_iter()
            .map(|yaml| T::deserialize(yaml, source_map))
            .collect()
    }
}

/// Deserialize a plain map with string keys
impl<K, V> DeserializeYaml for IndexMap<K, V>
where
    K: Eq + Hash + DeserializeYaml,
    V: DeserializeYaml,
{
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(yaml: SourcedYaml, source_map: &SourceMap) -> Result<Self> {
        yaml.try_into_mapping()?
            .into_iter()
            .map(|(k, v)| {
                Ok((
                    K::deserialize(k, source_map)?,
                    V::deserialize(v, source_map)?,
                ))
            })
            .collect()
    }
}

/// A custom version of [saphyr::MarkedYaml] that also tracks the source *file*
/// for each node. This allows us to load values from multiple files and track
/// the original source of each individual value correctly. The source is stored
/// as a numeric ID so that the file paths don't have to be copy repeatedly.
/// [SourceMap] is used to map IDs to strings if the source path needs to be
/// displayed.
#[derive(Clone, Debug, Eq)]
pub struct SourcedYaml<'input> {
    pub location: SourceIdLocation,
    pub data: YamlData<'input, Self>,
}

impl<'input> SourcedYaml<'input> {
    /// Parse a YAML value from a file
    fn load(path: &Path, source: SourceId) -> Result<Self> {
        let content =
            fs::read_to_string(path).map_err(|error| LocatedError {
                error: YamlErrorKind::Io {
                    error,
                    source: path.display().to_string(),
                },
                location: SourceIdLocation::default(),
            })?;
        Self::load_from_str(&content, source)
    }

    /// Parse a YAML value from a string
    fn load_from_str(input: &str, source: SourceId) -> Result<Self> {
        let mut documents = MarkedYaml::load_from_str(input)
            .map_err(|error| LocatedError::scan(error, source))?;
        // If the file is empty, pretend there's an empty mapping instead
        // because that's functionally equivalent
        let yaml = documents
            .pop()
            .unwrap_or(YamlData::Mapping(Default::default()).into());

        // Convert to our own YAML format so we can track source locations for
        // multiple files
        let yaml = Self::from_marked_yaml(yaml, source);

        Ok(yaml)
    }

    /// Convert a [MarkedYaml] to [SourcedYaml] by transforming `Marker` spans
    /// to [SourceLocation]
    fn from_marked_yaml(yaml: MarkedYaml<'input>, source_id: SourceId) -> Self {
        let location =
            SourceIdLocation::from_marker(source_id, yaml.span.start);

        let data = match yaml.data {
            YamlData::Value(scalar) => YamlData::Value(scalar),
            YamlData::Sequence(sequence) => YamlData::Sequence(
                sequence
                    .into_iter()
                    .map(|item| Self::from_marked_yaml(item, source_id))
                    .collect(),
            ),
            YamlData::Mapping(mapping) => YamlData::Mapping(
                mapping
                    .into_iter()
                    .map(|(key, value)| {
                        (
                            Self::from_marked_yaml(key, source_id),
                            Self::from_marked_yaml(value, source_id),
                        )
                    })
                    .collect(),
            ),
            YamlData::Tagged(tag, value) => YamlData::Tagged(
                tag,
                Box::new(Self::from_marked_yaml(*value, source_id)),
            ),
            YamlData::Alias(alias) => YamlData::Alias(alias),
            YamlData::BadValue => YamlData::BadValue,
            YamlData::Representation(a, b, c) => {
                YamlData::Representation(a, b, c)
            }
        };

        Self { location, data }
    }

    /// Unpack the YAML as a boolean
    pub fn try_into_bool(self) -> Result<bool> {
        if let YamlData::Value(Scalar::Boolean(b)) = self.data {
            Ok(b)
        } else {
            Err(LocatedError::unexpected(Expected::Boolean, self))
        }
    }

    /// Unpack the YAML as an usize
    pub fn try_into_usize(self) -> Result<usize> {
        if let YamlData::Value(Scalar::Integer(i)) = self.data {
            i.try_into()
                .map_err(|error| LocatedError::other(error, self.location))
        } else {
            Err(LocatedError::unexpected(Expected::Number, self))
        }
    }

    /// Unpack the YAML as a string
    pub fn try_into_string(self) -> Result<String> {
        if let YamlData::Value(Scalar::String(s)) = self.data {
            Ok(s.into_owned())
        } else {
            Err(LocatedError::unexpected(Expected::String, self))
        }
    }

    /// Unpack the YAML as a sequence
    pub fn try_into_sequence(self) -> Result<Vec<Self>> {
        if let YamlData::Sequence(sequence) = self.data {
            Ok(sequence)
        } else {
            Err(LocatedError::unexpected(Expected::Sequence, self))
        }
    }

    /// Unpack the YAML as a mapping
    pub fn try_into_mapping(self) -> Result<AnnotatedMapping<'input, Self>> {
        if let YamlData::Mapping(mapping) = self.data {
            // We don't support YAML merges. Detect the key proactively to
            // provide a helpful error
            if mapping.contains_key(&SourcedYaml::value_from_str("<<")) {
                Err(LocatedError {
                    error: YamlErrorKind::UnsupportedMerge,
                    location: self.location,
                })
            } else {
                Ok(mapping)
            }
        } else {
            Err(LocatedError::unexpected(Expected::Mapping, self))
        }
    }

    /// Create a YAML string scalar from a string reference
    pub fn value_from_str(value: &'input str) -> Self {
        Self {
            data: YamlData::Value(Scalar::parse_from_cow(value.into())),
            location: SourceIdLocation::default(),
        }
    }

    /// Create a YAML string scalar from an owned string
    fn value_from_string(value: String) -> Self {
        Self {
            data: YamlData::Value(Scalar::parse_from_cow(value.into())),
            location: SourceIdLocation::default(),
        }
    }

    /// If this YAML value is a mapping, drop all entries whose keys start with
    /// the `.` character
    pub fn drop_dot_fields(&mut self) {
        if let YamlData::Mapping(mapping) = &mut self.data {
            mapping.retain(|key, _| {
                !key.data.as_str().is_some_and(|s| s.starts_with('.'))
            });
        }
    }
}

impl<'a> From<YamlData<'a, SourcedYaml<'a>>> for SourcedYaml<'a> {
    fn from(value: YamlData<'a, SourcedYaml<'a>>) -> Self {
        Self {
            data: value,
            location: SourceIdLocation::default(),
        }
    }
}

/// Ignore source location in equality. Lifetime can vary between the two
/// operands
impl<'b> PartialEq<SourcedYaml<'b>> for SourcedYaml<'_> {
    fn eq(&self, other: &SourcedYaml<'b>) -> bool {
        self.data == other.data
    }
}

// Custom Hash impl needs to match the PartialEq impl: it ignores src location
impl Hash for SourcedYaml<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.data.hash(state);
    }
}

impl AnnotatedNode for SourcedYaml<'_> {
    type HashKey<'a> = SourcedYaml<'a>;

    fn parse_representation_recursive(&mut self) -> bool {
        self.data.parse_representation_recursive()
    }
}

/// Map of all YAML files that have been loaded. For YAML documents with no
/// external references, this map will only ever hold one entry. When references
/// extend to other files, they will get included here.
///
/// The purpose of this is to centralize all the loaded paths in one place, so
/// that each individual YAML node can be given its source ID without having to
/// clone expensive paths into each one. The map also makes it easy to cache
/// sources that get referenced multiple times.
#[derive(Debug, Default)]
pub struct SourceMap {
    /// A bijection between source IDs and paths. Each path is unique and gets
    /// assigned a unique ID. The ID is an index into this set.
    sources: IndexSet<PathBuf>,
}

impl SourceMap {
    /// Add a new YAML file to the collection of sources and return its unique
    /// ID. If the path is already in the map, its existing ID will be returned.
    ///
    /// ## Panics
    ///
    /// - If the path is not absolute. Caller is responsible for ensuring the
    ///   path has been made absolute
    /// - If the source is already in the map
    fn add_source(&mut self, path: PathBuf) -> SourceId {
        // Sanity checks. Failures indicate a bug in the resolver
        assert!(
            path.is_absolute(),
            "Source path must be absolute but got {}",
            path.display()
        );
        assert!(
            !self.sources.contains(&path),
            "Source {} already in map",
            path.display()
        );

        let index = self.sources.len() as u8;
        self.sources.insert(path);
        SourceId::File(index)
    }

    /// Get a source path by ID
    fn get_path(&self, source_id: SourceId) -> Option<&Path> {
        match source_id {
            SourceId::File(index) => {
                self.sources.get_index(index as usize).map(PathBuf::as_path)
            }
            SourceId::Memory => None,
        }
    }

    /// Get a source ID by path
    fn get_source_id(&self, path: &Path) -> Option<SourceId> {
        self.sources
            .get_index_of(path)
            .map(|index| SourceId::File(index as u8))
    }
}

/// Cheap ID for a YAML document source. IDs are unique within the scope of a
/// single root document. Additional IDs are generated for additional sources
/// that are referenced within the document tree.
///
/// Use a small type here to enable better bitpacking
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
enum SourceId {
    /// YAML was loaded from a file. Value is an index into [SourceMa[]]
    File(u8),
    /// YAML was loaded from a string in memory. Used
    #[default]
    Memory,
}

/// Source location of a YAML value, where the source is a cheap ID.
///
/// To resolve this into a [SourceLocation], use [Self::resolve].
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SourceIdLocation {
    /// Source of the YAML document
    source: SourceId,
    /// 1-indexed line in the document
    line: u32,
    /// 1-indexed column in the document
    column: u32,
}

impl SourceIdLocation {
    fn from_marker(source_id: SourceId, marker: Marker) -> Self {
        Self {
            source: source_id,
            // God help us if we have a file longer than 2^32 lines
            line: marker.line() as u32,
            column: marker.col() as u32,
        }
    }

    /// Resolve this source location by mapping its source ID to the
    /// corresponding string. This makes the location ready for display, at
    /// the cost of making it no longer `Copy`.
    pub fn resolve(&self, source_map: &SourceMap) -> SourceLocation {
        let source = source_map
            .get_path(self.source)
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        SourceLocation {
            source,
            line: self.line,
            column: self.column,
        }
    }
}

/// Location in a YAML document.
///
/// The source has been rendered to a presentable string (probably a path).
#[derive(Clone, Debug, Default, derive_more::Display)]
#[display("{source}:{line}:{column}")]
pub struct SourceLocation {
    /// Display name (e.g. path) for the YAML source
    pub source: String,
    /// 1-indexed line in the file
    pub line: u32,
    /// 1-indexed column in the file
    pub column: u32,
}

// Ignore location in tests for simpler assertions
#[cfg(feature = "test")]
impl PartialEq for SourceLocation {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Utility for deserializing a struct or enum variant from a YAML mapping.
/// Initialize this struct with a YAML value, and it will:
/// - Ensure the value is a mapping
/// - Enable deserializing individual fields with [get](Self::get)
/// - Ensure no unexpected fields were present with [done](Self::done)
///     - NOTE: `done` needs to be called manually after deserialization!
pub struct StructDeserializer<'a> {
    pub mapping: AnnotatedMapping<'a, SourcedYaml<'a>>,
    pub location: SourceIdLocation,
}

impl<'a> StructDeserializer<'a> {
    pub fn new(yaml: SourcedYaml<'a>) -> Result<Self> {
        let location = yaml.location;
        let mapping = yaml.try_into_mapping()?;
        Ok(Self { mapping, location })
    }

    /// Deserialize a field from the mapping
    pub fn get<T: DeserializeYaml>(
        &mut self,
        field: Field<T>,
        source_map: &SourceMap,
    ) -> Result<T> {
        if let Some(value) = self
            .mapping
            .remove(&SourcedYaml::value_from_str(field.name))
        {
            T::deserialize(value, source_map)
        } else if let Some(default) = field.default {
            Ok(default)
        } else {
            Err(LocatedError {
                error: YamlErrorKind::MissingField {
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
            let key_location = key.location;
            // If the key isn't a string, it's reasonable to return a type error
            let key = key.try_into_string()?;
            Err(LocatedError {
                error: YamlErrorKind::UnexpectedField(key),
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

/// Test helpers
#[cfg(feature = "test")]
mod test_util {
    use super::{DeserializeYaml, Result, SourcedYaml};
    use crate::yaml::{SourceId, SourceMap};
    use std::iter;

    /// Deserialize a [serde_yaml::Value] using saphyr. Serde values are easier
    /// to construct than saphyr values
    pub fn deserialize_yaml<T: DeserializeYaml>(
        yaml: serde_yaml::Value,
    ) -> Result<T> {
        let yaml_input = serde_yaml::to_string(&yaml).unwrap();
        let yaml = SourcedYaml::load_from_str(&yaml_input, SourceId::Memory)?;
        let source_map = SourceMap::default();
        T::deserialize(yaml, &source_map)
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
