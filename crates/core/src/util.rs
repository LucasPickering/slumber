//! Miscellaneous utility constants/types/functions

use derive_more::{DerefMut, Display};
use dialoguer::Confirm;
use std::{
    collections::{HashMap, hash_map::Entry},
    fmt::{self, Debug},
    hash::Hash,
    sync::Arc,
};
use tokio::sync::{Mutex, OwnedRwLockWriteGuard, RwLock};
use tracing::error;

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

/// Show the user a confirmation prompt
pub fn confirm(prompt: impl Into<String>) -> bool {
    Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .wait_for_newline(true)
        .interact()
        .unwrap_or(false)
}

/// Helper to printing bytes. If the bytes aren't valid UTF-8, they'll be
/// printed in hex representation instead
pub struct MaybeStr<'a>(pub &'a [u8]);

impl Display for MaybeStr<'_> {
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
