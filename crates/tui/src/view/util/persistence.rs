//! Implementation of the [persisted] crate for UI data

use crate::view::ViewContext;
use persisted::PersistedStore;
use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Debug;

/// This struct exists solely to hold an impl of [PersistedStore], which
/// persists UI state into the database.
#[derive(Debug)]
pub struct DatabasePersistedStore;

/// Wrapper for [persisted::PersistedKey] that applies additional bounds
/// necessary for our store
pub trait PersistedKey: Debug + Serialize + persisted::PersistedKey {}
impl<T: Debug + Serialize + persisted::PersistedKey> PersistedKey for T {}

/// Wrapper for [persisted::Persisted] bound to our store
pub type Persisted<K> = persisted::Persisted<DatabasePersistedStore, K>;

/// Wrapper for [persisted::PersistedLazy] bound to our store
pub type PersistedLazy<K, C> =
    persisted::PersistedLazy<DatabasePersistedStore, K, C>;

/// Persist UI state via the database. We have to be able to serialize keys to
/// insert and lookup. We have to serialize values to insert, and deserialize
/// them to retrieve.
impl<K> PersistedStore<K> for DatabasePersistedStore
where
    K: PersistedKey,
    K::Value: Debug + Serialize + DeserializeOwned,
{
    fn load_persisted(key: &K) -> Option<K::Value> {
        ViewContext::with_database(|database| {
            database.get_ui(K::type_name(), key)
        })
        // Error is already traced in the DB, nothing to do with it here
        .ok()
        .flatten()
    }

    fn store_persisted(key: &K, value: &K::Value) {
        ViewContext::with_database(|database| {
            database.set_ui(K::type_name(), key, value)
        })
        // Error is already traced in the DB, nothing to do with it here
        .ok();
    }
}
