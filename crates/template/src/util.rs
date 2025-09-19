use crate::{Identifier, RenderError, Stream};
use std::{
    collections::{HashMap, hash_map::Entry},
    ops::DerefMut,
    sync::Arc,
};
use tokio::sync::{Mutex, OwnedRwLockWriteGuard, RwLock};
use tracing::error;

/// `None`: Another writer is computing the value
/// `Some(Ok(_))`: Render succeeded
/// `Some(Err(_))`: Render failed
type FieldCacheValue = Option<Result<Stream, RenderError>>;

/// A cache of template values that either have been computed, or are
/// asynchronously being computed. This allows multiple references to the same
/// template field to share their work.
#[derive(Debug, Default)]
pub struct FieldCache {
    /// Cache each result by key. The outer mutex will only be held open for as
    /// long as it takes to check if the value is in the cache or not. The
    /// inner lock will be blocked on until the value is available.
    cache: Mutex<HashMap<Identifier, Arc<RwLock<FieldCacheValue>>>>,
}

impl FieldCache {
    /// Get a value from the cache, or if not present, insert a placeholder
    /// value and return a guard that can be used to insert the completed value
    /// later. The placeholder will tell subsequent accessors of this key that
    /// the value is being computed, and will be present later. If the
    /// placeholder is present and the final value being computed, **this block
    /// will not return until the value is available**.
    pub(crate) async fn get_or_init(
        &self,
        field: Identifier,
    ) -> FieldCacheOutcome {
        let mut cache = self.cache.lock().await;
        match cache.entry(field) {
            Entry::Occupied(entry) => {
                let lock = Arc::clone(entry.get());
                drop(cache); // Drop the outer lock as quickly as possible

                match &*lock.read_owned().await {
                    Some(result) => FieldCacheOutcome::Hit(result.clone()),
                    // Only possible if the writer panics or there's a bug in
                    // the implementation where the guard is never written to
                    None => panic!("Value not written to cache"),
                }
            }
            Entry::Vacant(entry) => {
                let lock = Arc::new(RwLock::new(None));
                entry.insert(Arc::clone(&lock));
                // Grab the write lock and hold it as long as the parent is
                // working to compute the value
                let guard = lock
                    .try_write_owned()
                    .expect("Lock was just created, who the hell grabbed it??");
                // Drop the root cache lock *after* we acquire the lock for our
                // own future, to prevent other tasks grabbing it first
                drop(cache);

                FieldCacheOutcome::Miss(FieldCacheGuard(guard))
            }
        }
    }
}

/// Outcome of check a future cache for a particular key
pub(crate) enum FieldCacheOutcome {
    /// The value is already in the cache
    Hit(Result<Stream, RenderError>),
    /// The value is not in the cache. Caller is responsible for inserting it
    /// by calling [FutureCacheGuard::set] once computed.
    Miss(FieldCacheGuard),
}

/// A handle for writing a computed future value back into the cache. This is
/// returned once per key, to the first caller of that key. The caller is then
/// responsible for calling [FutureCacheGuard::set] to insert the value for
/// everyone else. Subsequent callers to the cache will block until `set` is
/// called.
pub(crate) struct FieldCacheGuard(OwnedRwLockWriteGuard<FieldCacheValue>);

impl FieldCacheGuard {
    pub fn set(mut self, result: Result<Stream, RenderError>) {
        *self.0.deref_mut() = Some(result);
    }
}

impl Drop for FieldCacheGuard {
    fn drop(&mut self) {
        if self.0.is_none() {
            // Friendly little error logging. We don't have a good way of
            // identifying *which* lock this happened to :(
            error!("Future cache guard dropped without setting a value");
        }
    }
}
