//! UI state persistence. The store uses the DB as a backend to persist various
//! parts of the UI state, such as list selections, toggle states, etc.

use crate::view::ViewContext;
use anyhow::Context;
use serde::{Serialize, de::DeserializeOwned};
use slumber_core::database::CollectionDatabase;
use slumber_util::ResultTracedAnyhow;
use std::{
    any::{self, Any},
    cell::RefCell,
    collections::HashMap,
    fmt::Debug,
};
use tracing::error;

/// Persistence store backed by the SQLite database. This is a cheap facade to
/// the DB. The store should be recreated whenever it's needed.
///
/// The keys and values are serialized as JSON and stored in the database.
///
/// Values are persisted by the event loop at the end of each event phase.
/// Values are restored adhoc from each component's constructor.
///
/// ## Session
///
/// In addition to persistent values across sessions in the database, this also
/// supports single-session persistence. "Single-session persistence" sounds
/// like an oxymoron; why persist at all? Some components specifically want to
/// trash their values at the end of a session, but need to persist them when
/// unmounted or when the collection reloads. For example, recipe template
/// overrides are designed to be temporary, so we don't want to keep them in the
/// DB.
///
/// Unlike the DB store, the session store doesn't serialize the key and value.
/// The key is stored as a string (via [ToString]) and the value is a trait
/// object. This is possible because we're storing it in a thread local.
pub struct PersistentStore<'a> {
    database: &'a CollectionDatabase,
}

impl<'a> PersistentStore<'a> {
    thread_local! {
        /// Static instance for the session store. Persistence is handled in the
        /// main view thread, so we only even need this in one thread. We could
        /// potentially put this in the view context, but isolating it here
        /// limits what we need to borrow from the cell to just what we need.
        /// It also prevents external access to the store.
        static SESSION: RefCell<HashMap<String, Box<dyn Any>>> =
            RefCell::default();
    }

    /// Create a new store from a database. This is a cheap operation, as it
    /// just requires a reference to the database. The store should be recreated
    /// for each update phase.
    pub fn new(database: &'a CollectionDatabase) -> Self {
        Self { database }
    }

    /// Get a value from the store
    pub fn get<K: PersistentKey>(key: &K) -> Option<K::Value> {
        ViewContext::with_database(|db| {
            let key = Self::encode_json(key);
            let value =
                db.get_ui(Self::key_type::<K>(), &key).ok().flatten()?;
            Self::decode_json(&value).traced().ok()
        })
    }

    /// Set a value in the store
    pub fn set<K: PersistentKey>(&mut self, key: &K, value: &K::Value) {
        let key = Self::encode_json(key);
        let value = Self::encode_json(value);
        self.database
            .set_ui(Self::key_type::<K>(), &key, &value)
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

    /// Get a value from the session store
    pub fn get_session<K: SessionKey>(key: &K) -> Option<K::Value> {
        Self::SESSION.with_borrow(|store| {
            let value = store.get(&key.to_string())?;

            if let Some(value) = value.downcast_ref::<K::Value>() {
                Some(value.clone())
            } else {
                // Keys should be unique and only bound to a single value
                error!(
                    "Incorrect value type for session key {key:?}; \
                    expected value type {value_type}",
                    value_type = any::type_name::<K::Value>()
                );
                None
            }
        })
    }

    /// Insert a value into the session store
    pub fn set_session<K: SessionKey>(&mut self, key: &K, value: K::Value) {
        Self::SESSION.with_borrow_mut(|store| {
            store.insert(key.to_string(), Box::new(value));
        });
    }

    /// Remove a value from the session store
    pub fn remove_session<K: SessionKey>(&mut self, key: &K) {
        Self::SESSION.with_borrow_mut(|store| {
            store.remove(&key.to_string());
        });
    }

    /// Get the encoded string for a key type
    fn key_type<K>() -> &'static str {
        any::type_name::<K>()
    }

    /// Encode a value as JSON for insertion into the DB
    fn encode_json<T: Serialize>(key: &T) -> String {
        // Serialization only fails if the type can't be encoded as JSON, which
        // would mean a type is wonky and would show up immediately in dev
        serde_json::to_string(key).unwrap()
    }

    /// Decode a JSON value from the DB
    fn decode_json<T: DeserializeOwned>(value: &str) -> anyhow::Result<T> {
        serde_json::from_str(value)
            .context("Error deserializing persisted value")
    }
}

/// A key that can be used to persist and restore a value in the database store
pub trait PersistentKey: Serialize {
    /// Type of the value associated with this key. This enforces that the
    /// correct value is given during persisting and defines the return value
    /// when loading from the store.
    type Value: Serialize + DeserializeOwned;
}

/// A key that can be used to persist and restore a value in the **session**
/// store
pub trait SessionKey: Debug + ToString {
    /// Type of value associated with this key. Values are stored as trait
    /// objects, so they must implement `Any`. The value is cloned out of the
    /// store when restored, so it must implement `Clone`.
    type Value: Any + Clone;
}
