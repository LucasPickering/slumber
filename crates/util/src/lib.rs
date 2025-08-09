//! Common utilities that aren't specific to one other subcrate and are unlikely
//! to change frequently. The main purpose of this is to pull logic out of the
//! core crate, because that one changes a lot and requires constant
//! recompilation.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

pub mod paths;
#[cfg(any(test, feature = "test"))]
mod test_util;
pub mod yaml;

#[cfg(any(test, feature = "test"))]
pub use test_util::*;

use anyhow::anyhow;
use itertools::Itertools;
use serde::{
    Deserialize,
    de::{DeserializeOwned, Error as _},
};
use std::{
    fmt::{self, Debug, Display},
    io::Read,
    ops::Deref,
    str::FromStr,
    time::Duration,
};
use tracing::error;
use winnow::{
    ModalResult, Parser, ascii::digit1, combinator::repeat, token::take_while,
};

/// Link to the GitHub New Issue form
pub const NEW_ISSUE_LINK: &str =
    "https://github.com/LucasPickering/slumber/issues/new";

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
///     "https://slumber.lucaspickering.me/api/chain.html",
/// );
/// ```
pub fn doc_link(path: &str) -> String {
    const ROOT: &str = "https://slumber.lucaspickering.me/";
    if path.is_empty() {
        ROOT.into()
    } else {
        format!("{ROOT}{path}.html")
    }
}

/// Get a link to a file in the remote git repo. This is the raw link, not the
/// fancy UI link. It will be pinned to tag of the current crate version.
pub fn git_link(path: &str) -> String {
    format!(
        "https://raw.githubusercontent.com\
        /LucasPickering/slumber/refs/tags/v{version}/{path}",
        version = env!("CARGO_PKG_VERSION"),
    )
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

/// A newtype for [Duration] that provides formatting, parsing, and
/// deserialization. The name is meant to make it harder to confuse with
/// [Duration].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TimeSpan(Duration);

impl TimeSpan {
    /// Get the inner [Duration]
    pub fn inner(self) -> Duration {
        self.0
    }
}

impl Display for TimeSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use the largest units possible
        let mut remaining = self.0.as_secs();

        // Make sure 0 doesn't give us an empty string
        if remaining == 0 {
            return write!(f, "0s");
        }

        // Start with the biggest units
        let units = DurationUnit::ALL
            .iter()
            .sorted_by_key(|unit| unit.seconds())
            .rev();
        for unit in units {
            let quantity = remaining / unit.seconds();
            if quantity > 0 {
                remaining %= unit.seconds();
                write!(f, "{quantity}{unit}")?;
            }
        }
        Ok(())
    }
}

impl FromStr for TimeSpan {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn quantity(input: &mut &str) -> ModalResult<u64> {
            digit1.parse_to().parse_next(input)
        }

        fn unit(input: &mut &str) -> ModalResult<DurationUnit> {
            take_while(1.., char::is_alphabetic)
                .parse_to()
                .parse_next(input)
        }

        // Parse one or more quantity-unit pairs and sum them all up
        let seconds = repeat(1.., (quantity, unit))
            .fold(
                || 0,
                |acc, (quantity, unit)| acc + (quantity * unit.seconds()),
            )
            .parse(s)
            // The format is so simple there isn't much value in spitting out a
            // specific parsing error, just use a canned one
            .map_err(|_| {
                format!(
                    "Invalid duration, must be `(<quantity><unit>)+` \
                    (e.g. `12d` or `1h30m`). Units are {}",
                    DurationUnit::ALL.iter().format_with(", ", |unit, f| f(
                        &format_args!("`{unit}`")
                    ))
                )
            })?;

        Ok(Self(Duration::from_secs(seconds)))
    }
}

impl<'de> Deserialize<'de> for TimeSpan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(D::Error::custom)
    }
}

/// Supported units for duration parsing/formatting
#[derive(Debug)]
enum DurationUnit {
    Second,
    Minute,
    Hour,
    Day,
}

impl DurationUnit {
    const ALL: &[Self] = &[Self::Second, Self::Minute, Self::Hour, Self::Day];

    fn seconds(&self) -> u64 {
        match self {
            DurationUnit::Second => 1,
            DurationUnit::Minute => 60,
            DurationUnit::Hour => 60 * 60,
            DurationUnit::Day => 60 * 60 * 24,
        }
    }
}

impl Display for DurationUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Second => write!(f, "s"),
            Self::Minute => write!(f, "m"),
            Self::Hour => write!(f, "h"),
            Self::Day => write!(f, "d"),
        }
    }
}

impl FromStr for DurationUnit {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "s" => Ok(Self::Second),
            "m" => Ok(Self::Minute),
            "h" => Ok(Self::Hour),
            "d" => Ok(Self::Day),
            _ => Err(anyhow!("Invalid duration unit `{s}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_err;
    use rstest::rstest;
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

    #[rstest]
    #[case::zero(Duration::from_secs(0), "0s")]
    #[case::seconds_short(Duration::from_secs(3), "3s")]
    #[case::seconds_hour(Duration::from_secs(3600), "1h")]
    #[case::seconds_composite(Duration::from_secs(3690), "1h1m30s")]
    // Subsecond precision is lost
    #[case::seconds_subsecond_lost(Duration::from_millis(400), "0s")]
    #[case::seconds_subsecond_round_down(Duration::from_millis(1999), "1s")]
    fn test_time_span_to_string(
        #[case] duration: Duration,
        #[case] expected: &'static str,
    ) {
        assert_eq!(&TimeSpan(duration).to_string(), expected);
    }

    #[rstest]
    #[case::seconds_zero("0s", Duration::from_secs(0))]
    #[case::seconds_short("1s", Duration::from_secs(1))]
    #[case::seconds_longer("100s", Duration::from_secs(100))]
    #[case::minutes("3m", Duration::from_secs(180))]
    #[case::hours("3h", Duration::from_secs(10_800))]
    #[case::days("2d", Duration::from_secs(172_800))]
    #[case::composite("2d3h10m17s", Duration::from_secs(
        2 * 86400 + 3 * 3600 + 10 * 60 + 17
    ))]
    fn test_time_span_parse(
        #[case] s: &'static str,
        #[case] expected: Duration,
    ) {
        assert_eq!(s.parse::<TimeSpan>().unwrap(), TimeSpan(expected));
    }

    #[rstest]
    #[case::negative("-1s", "Invalid duration")]
    #[case::whitespace(" 1s ", "Invalid duration")]
    #[case::trailing_whitespace("1s ", "Invalid duration")]
    #[case::decimal("3.5s", "Invalid duration")]
    #[case::invalid_unit("3hr", "Units are `s`, `m`, `h`, `d`")]
    fn test_time_span_parse_error(
        #[case] s: &'static str,
        #[case] expected_error: &str,
    ) {
        assert_err!(
            // Map to anyhow error because assert_err! requires it
            s.parse::<TimeSpan>().map_err(anyhow::Error::msg),
            expected_error
        );
    }
}
