use slumber_template::{Identifier, Value};
use std::{
    collections::{HashMap, hash_map::Entry},
    ops::DerefMut,
    sync::Arc,
};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tracing::error;

/// A cache of template values that either have been computed, or are
/// asynchronously being computed. This allows multiple references to the same
/// template field to share their work.
#[derive(Debug, Default)]
pub struct FieldCache {
    /// Cache each value by key. The outer mutex will only be held open for as
    /// long as it takes to check if the value is in the cache or not. The
    /// inner mutex will be blocked on until the value is available.
    ///
    /// This uses a mutex instead of rwlock for the inner lock to prevent race
    /// conditions in the scenario where the first entrant doesn't write, in
    /// which case the second entrant has to upgrade their read to a write. The
    /// contention on the mutex should be extremely low once the write is done,
    /// so the difference between mutex and rwlock is minimal.
    cache: Mutex<HashMap<Identifier, Arc<Mutex<Option<Value>>>>>,
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
                drop(cache); // Drop the outer lock before acquiring the inner
                let guard = lock.clone().lock_owned().await;

                if let Some(value) = &*guard {
                    FieldCacheOutcome::Hit(value.clone())
                } else {
                    // If someone else grabbed the lock but didn't write to it,
                    // we're now responsible for computing+caching it. This can
                    // happen in two scenarios:
                    // - Other task failed in an unexpected way
                    // - Field evaluated to a stream, which can't be cached
                    FieldCacheOutcome::Miss(FieldCacheGuard(guard))
                }
            }
            Entry::Vacant(entry) => {
                let lock = Arc::new(Mutex::new(None));
                entry.insert(Arc::clone(&lock));
                // Grab the write lock and hold it as long as the parent is
                // working to compute the value
                let guard = lock
                    .try_lock_owned()
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
#[derive(Debug)]
pub(crate) enum FieldCacheOutcome {
    /// The value is already in the cache
    Hit(Value),
    /// The value is not in the cache. Caller is responsible for inserting it
    /// by calling [FieldCacheGuard::set] once computed.
    Miss(FieldCacheGuard),
}

/// A handle for writing a computed future value back into the cache. This is
/// returned once per key, to the first caller of that key. The caller is then
/// responsible for calling [FieldCacheGuard::set] to insert the value for
/// everyone else. Subsequent callers to the cache will block until `set` is
/// called.
#[derive(Debug)]
pub(crate) struct FieldCacheGuard(OwnedMutexGuard<Option<Value>>);

impl FieldCacheGuard {
    pub fn set(mut self, value: Value) {
        *self.0.deref_mut() = Some(value);
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::join;
    use slumber_util::assert_matches;

    /// If the first writer doesn't write anything, the second should get a
    /// chance to
    #[tokio::test]
    async fn test_field_cache() {
        let cache = FieldCache::default();
        let field: Identifier = "field".into();

        let fut1 = async {
            let guard = assert_matches!(
                cache.get_or_init(field.clone()).await,
                FieldCacheOutcome::Miss(guard) => guard,
            );
            let value: Value = true.into();
            guard.set(value.clone());
            value
        };
        let fut2 = async {
            assert_matches!(
                cache.get_or_init(field.clone()).await,
                FieldCacheOutcome::Hit(value) => value,
            )
        };

        // This should be deterministic because the futures are polled in order
        let (v1, v2) = join!(fut1, fut2);
        assert_eq!(v1, true.into());
        assert_eq!(v2, true.into());
    }

    /// If the first writer doesn't write anything, the second should get a
    /// chance to
    #[tokio::test]
    async fn test_field_cache_dropped_guard() {
        let cache = FieldCache::default();
        let field: Identifier = "field".into();

        let fut1 = async {
            // We get the write guard, but never write to it
            let guard = assert_matches!(
                cache.get_or_init(field.clone()).await,
                FieldCacheOutcome::Miss(guard) => guard,
            );
            drop(guard);
        };
        let fut2 = async {
            // After fut1 drops the write guard, we get it and write to it
            let guard = assert_matches!(
                cache.get_or_init(field.clone()).await,
                FieldCacheOutcome::Miss(guard) => guard,
            );
            let value: Value = true.into();
            guard.set(value.clone());
            value
        };
        let fut3 = async {
            // We get the value written by fut2
            assert_matches!(
                cache.get_or_init(field.clone()).await,
                FieldCacheOutcome::Hit(value) => value,
            )
        };

        // This should be deterministic because the futures are polled in order
        let ((), v2, v3) = join!(fut1, fut2, fut3);
        assert_eq!(v2, true.into());
        assert_eq!(v3, true.into());
    }
}
