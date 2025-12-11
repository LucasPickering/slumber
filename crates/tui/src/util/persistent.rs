//! UI state persistence. The store uses the DB as a backend to persist various
//! parts of the UI state, such as list selections, toggle states, etc.

use crate::view::ViewContext;
use serde::{Serialize, de::DeserializeOwned};
use slumber_core::database::CollectionDatabase;
use std::{any, fmt::Debug};

/// Persistence store backed by the SQLite database. This is a cheap facade to
/// the DB. The store should be recreated whenever it's needed.
///
/// Values are persisted by the event loop at the end of each event phase.
/// Values are restored adhoc from each component's constructor.
pub struct PersistentStore<'a> {
    database: &'a CollectionDatabase,
}

impl<'a> PersistentStore<'a> {
    /// Create a new store from a database. This is a cheap operation, as it
    /// just requires a reference to the database. The store should be recreated
    pub fn new(database: &'a CollectionDatabase) -> Self {
        Self { database }
    }

    /// Get a value from the store
    pub fn get<K: PersistentKey>(key: &K) -> Option<K::Value> {
        ViewContext::with_database(|db| {
            db.get_ui(Self::key_type::<K>(), key).ok().flatten()
        })
    }

    /// Set a value in the store
    pub fn set<K: PersistentKey>(&mut self, key: &K, value: &K::Value) {
        self.database
            .set_ui(Self::key_type::<K>(), key, value)
            // Error is already traced in the DB, nothing to do with it here
            .ok();
    }

    /// Set a value in the store; if the value is `None`, do nothing
    pub fn set_opt<K: PersistentKey>(
        &mut self,
        key: &K,
        value: Option<&K::Value>,
    ) {
        if let Some(value) = value {
            self.set(key, value);
        }
    }

    fn key_type<K>() -> &'static str {
        any::type_name::<K>()
    }
}

/// A key that can be used to persist and restore a value in the store
pub trait PersistentKey: Debug + Serialize {
    /// Type of the value associated with this key. This enforces that the
    /// corrent value is given during persisting and defines the return value
    /// when loading from the store.
    type Value: Debug + Serialize + DeserializeOwned;
}
