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
    // We take the first matching pattern, so order matters. The most general
    // patterns should go last. This could be a vec of tuples since we never
    // actually do keyed lookup, but that makes ser/de more complicated.
    patterns: IndexMap<MimePattern, V>,
}

impl<V> MimeMap<V> {
    /// Get the value of the **first** pattern in the map that matches the given
    /// string. Matching is only performed on the "essence" of the given mime
    /// type, i.e. the `type/subtype`. This means extensions of the type are
    /// ignored. It's very unlikely the user would want a different value based
    /// on extensions.
    pub fn get(&self, mime: &Mime) -> Option<&V> {
        self.patterns
            .iter()
            .find(|(pattern, _)| pattern.matches(mime.essence_str()))
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

impl DeserializeYaml for MimeMap<String> {
    fn expected() -> Expected {
        Expected::OneOf(&[&Expected::String, &Expected::Mapping])
    }

    fn deserialize(
        yaml: SourcedYaml,
        source_map: &SourceMap,
    ) -> yaml::Result<Self> {
        let patterns: IndexMap<MimePattern, String> = if yaml.data.is_mapping()
        {
            DeserializeYaml::deserialize(yaml, source_map)?
        } else {
            // Deserialize a single value as a map of {"*/*": value}
            let s = yaml.try_into_string()?;
            indexmap! { MimePattern::default() => s }
        };
        Ok(Self { patterns })
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
    fn test_match_mimes(#[case] mime: Mime, #[case] expected: String) {
        let map = map(&[
            ("text/csv", "csv"),
            ("text/*", "text"),
            ("json", "json"),
            ("*/*", "default"),
            // This should never get hit because it's after the default case
            ("audio/*", "audio"),
        ]);
        let actual = map.get(&mime).unwrap();
        assert_eq!(actual, &expected);
    }
}
