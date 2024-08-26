//! Implementation of the [persisted] crate for UI data

use crate::view::ViewContext;
use persisted::PersistedStore;
use serde::{de::DeserializeOwned, Serialize};
use slumber_core::{collection::RecipeId, template::Template};
use std::{collections::HashMap, fmt::Debug};
use tracing::debug;

/// Wrapper for [persisted::PersistedKey] that applies additional bounds
/// necessary for our store
pub trait PersistedKey: Debug + Serialize + persisted::PersistedKey {}
impl<T: Debug + Serialize + persisted::PersistedKey> PersistedKey for T {}

/// Wrapper for [persisted::Persisted] bound to our store
pub type Persisted<K> = persisted::Persisted<ViewContext, K>;

/// Wrapper for [persisted::PersistedLazy] bound to our store
pub type PersistedLazy<K, C> = persisted::PersistedLazy<ViewContext, K, C>;

/// Persist UI state via the database. We have to be able to serialize keys to
/// insert and lookup. We have to serialize values to insert, and deserialize
/// them to retrieve.
impl<K> PersistedStore<K> for ViewContext
where
    K: PersistedKey,
    K::Value: Debug + Serialize + DeserializeOwned,
{
    fn load_persisted(key: &K) -> Option<K::Value> {
        Self::with_database(|database| database.get_ui(K::type_name(), key))
            // Error is already traced in the DB, nothing to do with it here
            .ok()
            .flatten()
    }

    fn store_persisted(key: &K, value: &K::Value) {
        Self::with_database(|database| {
            database.set_ui(K::type_name(), key, value)
        })
        // Error is already traced in the DB, nothing to do with it here
        .ok();
    }
}

/// Special single-session [PersistedStore] just for edited recipe templates.
/// We don't want to store recipe overrides across sessions, because they could
/// be very large and conflict with changes in the recipe. Using a dedicated
/// type for this makes the generic bounds stricter which is nice.
///
/// To persist something in this store, you probably want to implement
/// [PersistedContainer](persisted::PersistedContainer) for your component/state
/// field.
#[derive(Debug, Default)]
pub struct RecipeOverrideStore(HashMap<RecipeOverrideKey, Template>);

impl PersistedStore<RecipeOverrideKey> for RecipeOverrideStore {
    fn load_persisted(key: &RecipeOverrideKey) -> Option<RecipeOverrideValue> {
        if let Some(template) =
            ViewContext::with_override_store(|store| store.0.get(key).cloned())
        {
            // Only overridden values are persisted
            debug!(?key, ?template, "Loaded persisted recipe override");
            Some(RecipeOverrideValue::Override(template))
        } else {
            None
        }
    }

    fn store_persisted(key: &RecipeOverrideKey, value: &RecipeOverrideValue) {
        // The value will be None if the template isn't overridden, in which
        // case we don't want to store it
        if let RecipeOverrideValue::Override(template) = value {
            debug!(?key, ?template, "Persisting recipe override");
            ViewContext::with_override_store_mut(|store| {
                store.0.insert(key.clone(), template.clone());
            })
        }
    }
}

/// An override value that may be persisted in the store
#[derive(Debug, PartialEq)]
pub enum RecipeOverrideValue {
    /// Default recipe value is in use, i.e. no override is present. Nothing
    /// will be persisted
    Default,
    /// User has provided an override for this field, persist it
    Override(Template),
}

/// Helper for some piece of the recipe UI that supports overriding and persists
/// its overrides to [RecipeOverrideStore]
pub type RecipeOverrideContainer<T> =
    persisted::PersistedLazy<RecipeOverrideStore, RecipeOverrideKey, T>;

/// Persisted key for anything that goes in [RecipeOverrideStore]. This uniquely
/// identifies any piece of a recipe that can be overridden.
#[derive(Clone, Debug, Eq, Hash, PartialEq, persisted::PersistedKey)]
#[persisted(RecipeOverrideValue)]
pub enum RecipeOverrideKey {
    /// Overridden body for a particular recipe
    Body { recipe_id: RecipeId },
}
