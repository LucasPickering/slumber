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

use anyhow::anyhow;
use itertools::Itertools;
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, Error as _},
};
use std::{
    fmt::{self, Debug, Display},
    hash::Hash,
    io::Read,
    ops::Deref,
    str::FromStr,
    time,
};
use tracing::error;
use winnow::{ModalResult, Parser, ascii::digit1, token::take_while};

/// Link to the GitHub New Issue form
pub const NEW_ISSUE_LINK: &str =
    "https://github.com/LucasPickering/slumber/issues/new/choose";

/// Get a link to a page on the doc website. This will append the doc prefix,
/// as well as the suffix.
///
/// ```
/// use slumber_core::util::doc_link;
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
    fn traced(self) -> Self;
}

impl<T> ResultTraced<T, anyhow::Error> for anyhow::Result<T> {
    fn traced(self) -> Self {
        self.inspect_err(|err| error!(error = err.deref()))
    }
}

/// A type that has an `id` field. This is ripe for a derive macro, maybe a fun
/// project some day?
pub trait HasId {
    type Id: Clone + Eq + Hash;

    fn id(&self) -> &Self::Id;

    fn set_id(&mut self, id: Self::Id);
}

/// Deserialize a map, and update each key so its `id` field matches its key in
/// the map. Useful if you need to access the ID when you only have a value
/// available, not the full entry.
pub fn deserialize_id_map<'de, Map, V, D>(
    deserializer: D,
) -> Result<Map, D::Error>
where
    Map: Deserialize<'de>,
    for<'m> &'m mut Map: IntoIterator<Item = (&'m V::Id, &'m mut V)>,
    D: Deserializer<'de>,
    V: Deserialize<'de> + HasId,
    V::Id: Deserialize<'de>,
{
    let mut map: Map = Map::deserialize(deserializer)?;
    // Update the ID on each value to match the key
    for (k, v) in &mut map {
        v.set_id(k.clone());
    }
    Ok(map)
}

/// A newtype for [std::time::Duration] that provides formatting, parsing, and
/// deserialization
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Duration(time::Duration);

impl Duration {
    /// Get the inner [std::time::Duration]
    pub fn inner(self) -> time::Duration {
        self.0
    }
}

impl Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Always print as seconds, because it's easiest. Sub-second precision
        // is lost
        write!(f, "{}s", self.0.as_secs())
    }
}

impl FromStr for Duration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        /// Supported units for duration parsing/formatting
        #[derive(Debug)]
        enum DurationUnit {
            Second,
            Minute,
            Hour,
            Day,
        }

        impl DurationUnit {
            const ALL: &[Self] =
                &[Self::Second, Self::Minute, Self::Hour, Self::Day];
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
                    _ => Err(anyhow!(
                        "Unknown duration unit `{s}`; must be one of {:?}",
                        Self::ALL.iter().format_with(", ", |unit, f| f(
                            &format_args!("`{unit}`")
                        ))
                    )),
                }
            }
        }

        fn quantity(input: &mut &str) -> ModalResult<u64> {
            digit1.parse_to().parse_next(input)
        }

        fn unit<'a>(input: &mut &'a str) -> ModalResult<&'a str> {
            take_while(1.., char::is_alphabetic).parse_next(input)
        }

        let (quantity, unit) = (quantity, unit)
            .parse(s)
            // The format is so simple there isn't much value in spitting out a
            // specific parsing error, just use a canned one
            .map_err(|_| {
                anyhow!(
                    "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)",
                )
            })?;

        let unit = unit.parse()?;
        let seconds = match unit {
            DurationUnit::Second => quantity,
            DurationUnit::Minute => quantity * 60,
            DurationUnit::Hour => quantity * 60 * 60,
            DurationUnit::Day => quantity * 60 * 60 * 24,
        };
        Ok(Self(time::Duration::from_secs(seconds)))
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(D::Error::custom)
    }
}

/// Parse bytes from a reader into YAML. This will merge any anchors/aliases.
pub fn parse_yaml<T: DeserializeOwned>(reader: impl Read) -> anyhow::Result<T> {
    // Two-step parsing is required for anchor/alias merging
    let deserializer = serde_yaml::Deserializer::from_reader(reader);
    let mut yaml_value: serde_yaml::Value =
        serde_path_to_error::deserialize(deserializer)?;
    yaml_value.apply_merge()?;
    let output = serde_path_to_error::deserialize(yaml_value)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::seconds_short(time::Duration::from_secs(3), "3s")]
    #[case::seconds_long(time::Duration::from_secs(3000), "3000s")]
    // Subsecond precision is lost
    #[case::seconds_subsecond_lost(time::Duration::from_millis(400), "0s")]
    #[case::seconds_subsecond_round_down(
        time::Duration::from_millis(1999),
        "1s"
    )]
    fn test_duration_to_string(
        #[case] duration: time::Duration,
        #[case] expected: &'static str,
    ) {
        assert_eq!(&Duration(duration).to_string(), expected);
    }

    #[rstest]
    #[case::seconds_zero("0s", time::Duration::from_secs(0))]
    #[case::seconds_short("1s", time::Duration::from_secs(1))]
    #[case::seconds_longer("100s", time::Duration::from_secs(100))]
    #[case::minutes("3m", time::Duration::from_secs(180))]
    #[case::hours("3h", time::Duration::from_secs(10800))]
    #[case::days("2d", time::Duration::from_secs(172800))]
    fn test_duration_parse(
        #[case] s: &'static str,
        #[case] expected: time::Duration,
    ) {
        assert_eq!(s.parse::<Duration>().unwrap(), Duration(expected));
    }

    #[rstest]
    #[case::negative(
        "-1s",
        "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)"
    )]
    #[case::whitespace(
        " 1s ",
        "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)"
    )]
    #[case::trailing_whitespace(
        "1s ",
        "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)"
    )]
    #[case::decimal(
        "3.5s",
        "Invalid duration, must be `<quantity><unit>` (e.g. `12d`)"
    )]
    #[case::invalid_unit(
        "3hr",
        "Unknown duration unit `hr`; must be one of `s`, `m`, `h`, `d`"
    )]
    fn test_duration_parse_error(
        #[case] s: &'static str,
        #[case] expected_error: &str,
    ) {
        assert_err!(s.parse::<Duration>(), expected_error);
    }
}
