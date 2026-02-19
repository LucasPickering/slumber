use glob::{Pattern, PatternError};
use indexmap::{IndexMap, indexmap};
use mime::Mime;
use serde::{Deserialize, Serialize};
use slumber_util::yaml::{
    self, DeserializeYaml, Expected, LocatedError, SourceMap, SourcedYaml,
};
use std::str::FromStr;

/// A map of content type patterns to values. Use this when you need to select a
/// value based on the `Content-Type` header of a request/response. The patterns
/// use [glob] for matching, so technically it's trying to match a Unix
/// path-like string, but that happens to look the same as a MIME type.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct MimeMap<V> {
    /// Mapped MIME patterns
    ///
    /// We take the first matching pattern, so order matters. The most general
    /// patterns should go last. This could be a vec of tuples since we never
    /// actually do keyed lookup, but that makes ser/de more complicated.
    ///
    /// I'm sure there's a fancy way to do <O(n) lookup, but these maps will be
    /// less than 10 elements typically so it really does not matter.
    patterns: IndexMap<MimePattern, V>,
}

impl<V> MimeMap<V> {
    /// Get the value of the **first** pattern in the map that matches the given
    /// string
    ///
    /// Matching is only performed on the "essence" of the given mime type, i.e.
    /// the `type/subtype`. This means extensions of the type are ignored. It's
    /// very unlikely the user would want a different value based on extensions.
    ///
    /// `mime_overrides` is passed from the config object so the MIME type can
    /// be transformed according to the user's config before performing the
    /// lookup in this map.
    pub fn get(
        &self,
        mime_overrides: &MimeOverrideMap,
        mime: &Mime,
    ) -> Option<&V> {
        // Check for an override first
        let mime = mime_overrides.get(mime);
        self.get_inner(mime)
    }

    /// Get a value from the map **without** override mapping
    fn get_inner(&self, mime: &Mime) -> Option<&V> {
        let essence_str = mime.essence_str();
        self.patterns
            .iter()
            .find(|(pattern, _)| pattern.matches(essence_str))
            .map(|(_, value)| value)
    }
}

// Derive includes an unwanted type bound
impl<V> Default for MimeMap<V> {
    fn default() -> Self {
        Self {
            patterns: Default::default(),
        }
    }
}

impl<V: DeserializeYaml> DeserializeYaml for MimeMap<V> {
    fn expected() -> Expected {
        Expected::OneOf(&[&Expected::String, &Expected::Mapping])
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let patterns = if yaml.data.is_mapping() {
            // Deserialize a mapping like {"json": v1, "*/*": v2}
            IndexMap::<MimePattern, V>::deserialize(yaml, source_map)?
        } else {
            // Deserialize a single value as a map of {"*/*": value}
            let value = V::deserialize(yaml, source_map)?;
            indexmap! { MimePattern::default() => value }
        };
        Ok(Self { patterns })
    }
}

/// A map of MIME types overridden by the user
///
/// This is used to map unknown MIME types to known ones. For example:
///
/// `"text/javascript": json` will treat all `text/javascript` bodies as JSON
/// bodies for the purposes of syntax highlighting, pager selection, etc.
///
/// The keys can be any MIME pattern (including wildcards), but the values
/// **must be valid MIME types**.
#[derive(Debug, Default, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(
    feature = "schema",
    derive(schemars::JsonSchema),
    schemars(example = Self::example()),
)]
#[serde(transparent)]
pub struct MimeOverrideMap(MimeMap<MimeAdopt>);

impl MimeOverrideMap {
    /// Map a MIME type according to the override mapping
    ///
    /// If the MIME isn't in the override map, return the given MIME.
    pub fn get<'a>(&'a self, mime: &'a Mime) -> &'a Mime {
        // Overriding is *not* recursive, we only do one level of lookup
        self.0.get_inner(mime).map(|mime| &mime.0).unwrap_or(mime)
    }

    /// JSON Schema example value
    #[cfg(feature = "schema")]
    fn example() -> Self {
        Self::from_iter([("text/javascript", mime::APPLICATION_JSON)])
    }
}

// Build maps for tests
impl FromIterator<(&'static str, Mime)> for MimeOverrideMap {
    fn from_iter<T: IntoIterator<Item = (&'static str, Mime)>>(
        iter: T,
    ) -> Self {
        let patterns = iter
            .into_iter()
            .map(|(key, value)| (key.parse().unwrap(), MimeAdopt(value)))
            .collect();
        Self(MimeMap { patterns })
    }
}

impl DeserializeYaml for MimeOverrideMap {
    fn expected() -> Expected {
        Expected::Mapping
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        // We can't reuse MimeMap's deserialization because we *don't* support
        // the single-value case here. That would effectively override all MIMEs
        // to a single type, which doesn't really make sense.
        let patterns =
            IndexMap::<MimePattern, MimeAdopt>::deserialize(yaml, source_map)?;
        Ok(Self(MimeMap { patterns }))
    }
}

/// Workaround for the orphan rule
#[derive(Debug, PartialEq)]
struct MimeAdopt(Mime);

impl Serialize for MimeAdopt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as string
        self.0.as_ref().serialize(serializer)
    }
}

impl DeserializeYaml for MimeAdopt {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(
        yaml: SourcedYaml,
        _source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location;
        let s = yaml.try_into_string()?;
        let mime: Mime = s
            .parse()
            .map_err(|error| LocatedError::other(error, location))?;
        Ok(Self(mime))
    }
}

// Use a string for the schema
#[cfg(feature = "schema")]
impl schemars::JsonSchema for MimeAdopt {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        String::schema_name()
    }

    fn json_schema(
        generator: &mut schemars::SchemaGenerator,
    ) -> schemars::Schema {
        String::json_schema(generator)
    }
}

/// Newtype for [glob::Pattern] so we can define ser/de for it
#[derive(
    Clone,
    Debug,
    derive_more::Display,
    derive_more::Deref,
    Serialize,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "String", into = "String")]
struct MimePattern(Pattern);

impl Default for MimePattern {
    fn default() -> Self {
        Self(Pattern::from_str("*/*").unwrap())
    }
}

impl From<MimePattern> for String {
    fn from(value: MimePattern) -> Self {
        value.to_string()
    }
}

impl FromStr for MimePattern {
    type Err = PatternError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check some known aliases
        let dealiased = match s {
            "default" => "*/*",
            "image" => "image/*",
            // Make sure to capture JSON extensions too
            "json" => "application/*json",
            other => other,
        };
        Ok(Self(dealiased.parse()?))
    }
}

impl TryFrom<String> for MimePattern {
    type Error = PatternError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

/// Deserialize via FromStr
impl DeserializeYaml for MimePattern {
    fn expected() -> Expected {
        Expected::String
    }

    fn deserialize(
        yaml: SourcedYaml,
        _source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let location = yaml.location;
        let s = yaml.try_into_string()?;
        s.parse()
            .map_err(|error| LocatedError::other(error, location))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mime::APPLICATION_JSON;
    use rstest::rstest;
    use serde_yaml::Mapping;
    use slumber_util::yaml::deserialize_yaml;

    fn map(entries: &[(&str, &str)]) -> MimeMap<String> {
        MimeMap {
            patterns: entries
                .iter()
                .map(|(k, v)| {
                    (k.parse::<MimePattern>().unwrap(), (*v).to_owned())
                })
                .collect(),
        }
    }

    #[rstest]
    #[case::string("test", map(&[("*/*", "test")]))]
    #[case::empty_map(serde_yaml::Value::Mapping(Mapping::default()), map(&[]))]
    #[case::aliases(serde_yaml::Value::Mapping([
            ("json".into(), "json-value".into()),
            ("image".into(), "image-value".into()),
            ("default".into(), "default-value".into()),
        ].into_iter().collect()),
        map(&[
            ("application/*json", "json-value"),
            ("image/*","image-value"),
            ("*/*", "default-value"),
        ]),
    )]
    fn test_deserialize(
        #[case] yaml: impl Into<serde_yaml::Value>,
        #[case] expected: MimeMap<String>,
    ) {
        assert_eq!(
            deserialize_yaml::<MimeMap<String>>(yaml.into()).unwrap(),
            expected
        );
    }

    #[rstest]
    #[case::wildcard("text/plain", "text")]
    #[case::default("image/png", "default")]
    #[case::priority("audio/ogg", "default")]
    #[case::json_plain("application/json", "json")]
    #[case::json_extension("application/ld+json", "json")]
    #[case::essence("text/csv; charset=utf-8", "csv")]
    #[case::override_mime("text/override; charset=utf-8", "json")]
    fn test_match_mimes(#[case] mime: Mime, #[case] expected: String) {
        let map = map(&[
            ("text/csv", "csv"),
            ("text/*", "text"),
            ("json", "json"),
            ("*/*", "default"),
            // This should never get hit because it's after the default case
            ("audio/*", "audio"),
        ]);
        let overrides =
            MimeOverrideMap::from_iter([("text/override", APPLICATION_JSON)]);
        let actual = map.get(&overrides, &mime).unwrap();
        assert_eq!(actual, &expected);
    }
}
