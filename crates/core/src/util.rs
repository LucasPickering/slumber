//! Miscellaneous utility constants/types/functions

pub mod paths;

use crate::{http::RequestError, template::ChainError};
use chrono::{
    format::{DelayedFormat, StrftimeItems},
    DateTime, Duration, Local, Utc,
};
use derive_more::{DerefMut, Display};
use serde::de::DeserializeOwned;
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::{self, Debug},
    hash::Hash,
    io::Read,
    ops::Deref,
    sync::Arc,
};
use tokio::sync::{Mutex, OwnedRwLockWriteGuard, RwLock};
use tracing::error;

const WEBSITE: &str = "https://slumber.lucaspickering.me";
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
    format!("{WEBSITE}/book/{path}.html")
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

/// Format a datetime for the user
pub fn format_time(time: &DateTime<Utc>) -> DelayedFormat<StrftimeItems> {
    time.with_timezone(&Local).format("%b %-d %H:%M:%S")
}

/// Format a duration for the user
pub fn format_duration(duration: &Duration) -> String {
    let ms = duration.num_milliseconds();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.2}s", ms as f64 / 1000.0)
    }
}

/// Format a byte total, e.g. 1_000_000 -> 1 MB
pub fn format_byte_size(size: usize) -> String {
    const K: usize = 10usize.pow(3);
    const M: usize = 10usize.pow(6);
    const G: usize = 10usize.pow(9);
    const T: usize = 10usize.pow(12);
    let (denom, suffix) = match size {
        ..K => return format!("{size} B"),
        K..M => (K, "K"),
        M..G => (M, "M"),
        G..T => (G, "G"),
        T.. => (T, "T"),
    };
    let size = size as f64 / denom as f64;
    format!("{size:.1} {suffix}B")
}

/// Extension trait for [Result]
pub trait ResultTraced<T, E>: Sized {
    /// If this is an error, trace it. Return the same result.
    fn traced(self) -> Self;
}

// This is deliberately *not* implemented for non-anyhow errors, because we only
// want to trace errors that have full context attached
impl<T> ResultTraced<T, anyhow::Error> for anyhow::Result<T> {
    fn traced(self) -> Self {
        if let Err(err) = &self {
            error!(error = err.deref());
        }
        self
    }
}

impl<T> ResultTraced<T, RequestError> for Result<T, RequestError> {
    fn traced(self) -> Self {
        if let Err(err) = &self {
            error!(error = %err);
        }
        self
    }
}

impl<T> ResultTraced<T, ChainError> for Result<T, ChainError> {
    fn traced(self) -> Self {
        if let Err(err) = &self {
            error!(error = %err);
        }
        self
    }
}

/// Helper to printing bytes. If the bytes aren't valid UTF-8, they'll be
/// printed in hex representation instead
pub struct MaybeStr<'a>(pub &'a [u8]);

impl<'a> Display for MaybeStr<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = std::str::from_utf8(self.0) {
            write!(f, "{s}")
        } else {
            let bytes_per_line = 12;
            // Format raw bytes in pairs of bytes
            for (i, byte) in self.0.iter().enumerate() {
                if i > 0 {
                    // Add whitespace before this group. Only use line breaks
                    // in alternate mode
                    if f.alternate() && i % bytes_per_line == 0 {
                        writeln!(f)?;
                    } else {
                        write!(f, " ")?;
                    }
                }

                write!(f, "{byte:02x}")?;
            }
            Ok(())
        }
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

/// A cache of values that either have been computed, or are asynchronously
/// being computed. This allows multiple computers of the same async values to
/// deduplicate their work.
#[derive(Debug)]
pub(crate) struct FutureCache<K: Hash + Eq, V: Clone> {
    /// Cache each value by key. The outer mutex will only be held open for as
    /// long as it takes to check if the value is in the cache or not. The
    /// inner lock will be blocked on until the value is available.
    cache: Mutex<HashMap<K, Arc<RwLock<Option<V>>>>>,
}

impl<K: Hash + Eq, V: 'static + Clone> FutureCache<K, V> {
    /// Get a value from the cache, or if not present, insert a placeholder
    /// value and return a guard that can be used to insert the completed value
    /// later. The placeholder will tell subsequent accessors of this key that
    /// the value is being computed, and will be present later. If the
    /// placeholder is present and the final value being computed, **this block
    /// will not return until the value is available**.
    pub async fn get_or_init(&self, key: K) -> FutureCacheOutcome<V> {
        let mut cache = self.cache.lock().await;
        match cache.entry(key) {
            Entry::Occupied(entry) => {
                let lock = Arc::clone(entry.get());
                drop(cache); // Drop the outer lock as quickly as possible

                match &*lock.read_owned().await {
                    Some(value) => FutureCacheOutcome::Hit(value.clone()),
                    None => FutureCacheOutcome::NoResponse,
                }
            }
            Entry::Vacant(entry) => {
                let lock = Arc::new(RwLock::new(None));
                entry.insert(Arc::clone(&lock));
                drop(cache); // Drop the outer lock as quickly as possible

                // Grab the write lock and hold it as long as the parent is
                // working to compute the value
                let guard = lock
                    .try_write_owned()
                    .expect("Lock was just created, who the hell grabbed it??");
                FutureCacheOutcome::Miss(FutureCacheGuard(guard))
            }
        }
    }
}

impl<K: Hash + Eq, V: Clone> Default for FutureCache<K, V> {
    fn default() -> Self {
        Self {
            cache: Default::default(),
        }
    }
}

/// Outcome of check a future cache for a particular key
pub(crate) enum FutureCacheOutcome<V> {
    /// The value is already in the cache
    Hit(V),
    /// The value is not in the cache. Caller is responsible for inserting it
    /// by calling [FutureCacheGuard::set] once computed.
    Miss(FutureCacheGuard<V>),
    /// The first entrant dropped their write guard without writing to it, so
    /// there's no response to return
    NoResponse,
}

/// A handle for writing a computed future value back into the cache. This is
/// returned once per key, to the first caller of that key. The caller is then
/// responsible for calling [FutureCacheGuard::set] to insert the value for
/// everyone else. Subsequent callers to the cache will block until `set` is
/// called.
pub(crate) struct FutureCacheGuard<V>(OwnedRwLockWriteGuard<Option<V>>);

impl<V> FutureCacheGuard<V> {
    pub fn set(mut self, value: V) {
        *self.0.deref_mut() = Some(value);
    }
}

impl<V> Drop for FutureCacheGuard<V> {
    fn drop(&mut self) {
        if self.0.is_none() {
            // Friendly little error logging. We don't have a good way of
            // identifying *which* lock this happened to :(
            error!("Future cache guard dropped without setting a value");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::zero(0, "0 B")]
    #[case::one(1, "1 B")]
    #[case::almost_kb(999, "999 B")]
    #[case::kb(1000, "1.0 KB")]
    #[case::kb_round_down(1049, "1.0 KB")]
    #[case::kb_round_up(1050, "1.1 KB")]
    #[case::almost_mb(999_999, "1000.0 KB")]
    #[case::mb(1_000_000, "1.0 MB")]
    #[case::almost_gb(999_999_999, "1000.0 MB")]
    #[case::gb(1_000_000_000, "1.0 GB")]
    #[case::almost_tb(999_999_999_999, "1000.0 GB")]
    #[case::tb(1_000_000_000_000, "1.0 TB")]
    fn test_format_byte_size(#[case] size: usize, #[case] expected: &str) {
        assert_eq!(&format_byte_size(size), expected);
    }
}
