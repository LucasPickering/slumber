//! Common utilities that aren't specific to one other subcrate and are unlikely
//! to change frequently. The main purpose of this is to pull logic out of the
//! core crate, because that one changes a lot and requires constant
//! recompilation.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

pub mod paths;
#[cfg(feature = "test")]
mod test_util;

#[cfg(feature = "test")]
pub use test_util::*;

use serde::de::DeserializeOwned;
use std::{fmt::Debug, io::Read, ops::Deref};
use tracing::error;

/// Link to the GitHub New Issue form
pub const NEW_ISSUE_LINK: &str =
    "https://github.com/LucasPickering/slumber/issues/new/choose";

/// A static mapping between values (of type `T`) and labels (strings). Used to
/// both stringify from and parse to `T`.
pub struct Mapping<'a, T: Copy>(&'a [(T, &'a [&'a str])]);

impl<'a, T: Copy> Mapping<'a, T> {
    /// Construct a new mapping
    pub const fn new(mapping: &'a [(T, &'a [&'a str])]) -> Self {
        Self(mapping)
    }

    /// Get a value by one of its labels
    pub fn get(&self, s: &str) -> Option<T> {
        for (value, strs) in self.0 {
            for other_string in *strs {
                if *other_string == s {
                    return Some(*value);
                }
            }
        }
        None
    }

    /// Get the label mapped to a value. If it has multiple labels, use the
    /// first. Panic if the value has no mapped labels
    pub fn get_label(&self, value: T) -> &str
    where
        T: Debug + PartialEq,
    {
        let (_, strings) = self
            .0
            .iter()
            .find(|(v, _)| v == &value)
            .unwrap_or_else(|| panic!("Unknown value {value:?}"));
        strings
            .first()
            .unwrap_or_else(|| panic!("No mapped strings for value {value:?}"))
    }

    /// Get all available mapped strings
    pub fn all_strings(&self) -> impl Iterator<Item = &str> {
        self.0
            .iter()
            .flat_map(|(_, strings)| strings.iter().copied())
    }
}
/// Extension trait for [Result]
pub trait ResultTraced<T, E>: Sized {
    /// If this is an error, trace it. Return the same result.
    #[must_use]
    fn traced(self) -> Self;
}

impl<T> ResultTraced<T, anyhow::Error> for anyhow::Result<T> {
    fn traced(self) -> Self {
        self.inspect_err(|err| error!(error = err.deref()))
    }
}

/// Get a link to a page on the doc website. This will append the doc prefix,
/// as well as the suffix.
///
/// ```
/// use slumber_util::doc_link;
/// assert_eq!(
///     doc_link("api/chain"),
///     "https://slumber.lucaspickering.me/book/api/chain.html",
/// );
/// ```
pub fn doc_link(path: &str) -> String {
    const ROOT: &str = "https://slumber.lucaspickering.me/book/";
    if path.is_empty() {
        ROOT.into()
    } else {
        format!("{ROOT}{path}.html")
    }
}

/// Parse bytes from a reader into YAML. This will merge any anchors/aliases.
pub fn parse_yaml<T: DeserializeOwned>(reader: impl Read) -> anyhow::Result<T> {
    // We use two-step parsing to enable pre-processing on the YAML

    // Parse into a YAML value
    let deserializer = serde_yaml::Deserializer::from_reader(reader);
    let mut yaml_value: serde_yaml::Value =
        serde_path_to_error::deserialize(deserializer)?;

    // Merge anchors+aliases
    yaml_value.apply_merge()?;
    // Remove any top-level fields that start with .
    if let serde_yaml::Value::Mapping(mapping) = &mut yaml_value {
        mapping.retain(|key, _| {
            !key.as_str().is_some_and(|key| key.starts_with('.'))
        });
    }

    let output = serde_path_to_error::deserialize(yaml_value)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Data {
        data: Inner,
    }

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Inner {
        i: i32,
        b: bool,
        s: String,
    }

    /// Test YAML preprocessing: anchor/alias merging and removing . fields
    #[test]
    fn test_parse_yaml() {
        let yaml = "
.ignore: &base
  i: 1
  b: true
  s: base

data:
  i: 2
  <<: *base
  s: hello
";

        let actual: Data = parse_yaml(yaml.as_bytes()).unwrap();
        let expected = Data {
            data: Inner {
                i: 2,
                b: true,
                s: "hello".into(),
            },
        };
        assert_eq!(actual, expected);
    }
}
