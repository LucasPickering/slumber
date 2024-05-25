//! Utilities for persisting UI state between sessions

use crate::{
    collection::RecipeId,
    http::RequestId,
    tui::view::{
        component::Component,
        context::ViewContext,
        event::{Event, EventHandler, Update},
        state::fixed_select::FixedSelect,
    },
};
use derive_more::{Deref, DerefMut};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;

/// A wrapper for any value that will automatically persist it to the state DB.
/// The value will be loaded from the DB on creation, and saved to the DB on
/// drop.
#[derive(Debug, Deref, DerefMut)]
pub struct Persistent<T: PersistentContainer> {
    key: Option<PersistentKey>,
    #[deref]
    #[deref_mut]
    container: T,
}

impl<T: PersistentContainer> Persistent<T> {
    /// Load the latest persisted value from the DB. If present, set the value
    /// of the container.
    pub fn new(key: PersistentKey, container: T) -> Self {
        Self::optional(Some(key), container)
    }

    /// Create a new persistent cell, with an optional key. If the key is not
    /// defined, this does not do any persistence loading/saving. This is
    /// helpful for usages that should only be persistent sometimes.
    pub fn optional(key: Option<PersistentKey>, mut container: T) -> Self {
        if let Some(key) = &key {
            // Load saved value from the database, and select it if available
            let loaded = ViewContext::with_database(|database| {
                database.get_ui::<_, <T::Value as Persistable>::Persisted>(key)
            });
            if let Ok(Some(value)) = loaded {
                container.set(value);
            }
        }

        Self { key, container }
    }
}

/// Forward events to the inner state cell
impl<T> EventHandler for Persistent<T>
where
    T: EventHandler + PersistentContainer,
{
    fn update(&mut self, event: Event) -> Update {
        self.container.update(event)
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        self.container.children()
    }
}

impl<T: PersistentContainer> Drop for Persistent<T> {
    fn drop(&mut self) {
        if let Some(key) = &self.key {
            let _ = ViewContext::with_database(|database| {
                database.set_ui(
                    key,
                    self.container.get().map(Persistable::get_persistent),
                )
            });
        }
    }
}

/// Unique identifier for a single persisted UI value. Some keys are singleton,
/// meaning there's only one corresponding value in the app. Others are dynamic,
/// e.g. for toggle state on each row in a table.
#[derive(Clone, Debug, Serialize)]
pub enum PersistentKey {
    /// Which pane is selected?
    PrimaryPane,
    /// Which tab in the record (AKA request/response) pane is selected?
    RecordTab,
    /// Which pane (if any) is fullscreened?
    FullscreenMode,
    /// Selected profile in the list
    ProfileId,
    /// Selected recipe/folder in the tree
    RecipeId,
    /// Selected request. Should belong to the persisted profile/recipe
    RequestId,
    /// Set of folders that are collapsed in the recipe tree
    RecipeCollapsed,
    /// Selected tab in the recipe pane
    RecipeTab,
    /// Selected query param, per recipe. Value is the query param name
    RecipeSelectedQuery(RecipeId),
    /// Toggle state for a single recipe+query param
    RecipeQuery { recipe: RecipeId, param: String },
    /// Selected header, per recipe. Value is the header name
    RecipeSelectedHeader(RecipeId),
    /// Toggle state for a single recipe+header
    RecipeHeader { recipe: RecipeId, header: String },
    /// Response body JSONPath query (**not** related to query params)
    ResponseBodyQuery(RecipeId),
}

/// A value type that can be persisted to the database
pub trait Persistable {
    /// The type of the value that's actually persisted to the database. In most
    /// cases this is the value itself, but some types use others (e.g. an ID).
    type Persisted: Debug + Serialize + DeserializeOwned;

    /// Get the value that should be persisted to the DB
    fn get_persistent(&self) -> &Self::Persisted;
}

impl<T: FixedSelect + Serialize + DeserializeOwned> Persistable for T {
    type Persisted = Self;

    fn get_persistent(&self) -> &Self::Persisted {
        self
    }
}

impl_persistable!(bool);
impl_persistable!(String);
impl_persistable!(RequestId);

/// A container that holds a persisted value. The container has to tell us how
/// to get and set the value, and [Persistent] will handle the actual DB
/// interfacing.
pub trait PersistentContainer {
    type Value: Persistable;

    /// Get the *persistable* value. The caller will be responsible for calling
    /// [Persistable::get_persistent].
    ///
    /// It's a little wonky that this returns an `Option` because not all
    /// containers will actually have an optional value, but I'm soft and can't
    /// figure out the correct typing to have it just return `&Self::Value`.
    fn get(&self) -> Option<&Self::Value>;

    /// Set the container's value, based on the persisted value`
    fn set(&mut self, value: <Self::Value as Persistable>::Persisted);
}

/// Any persistable type can be a container of itself
impl<T: Persistable<Persisted = T>> PersistentContainer for T {
    type Value = Self;

    fn get(&self) -> Option<&Self::Value> {
        Some(self)
    }

    fn set(&mut self, value: <Self::Value as Persistable>::Persisted) {
        *self = value;
    }
}

/// Implement [Persistable] for a type that can be persisted as itself. It'd be
/// great if this was just a derive macro, but that requires a whole separate
/// crate.
macro_rules! impl_persistable {
    ($type:ty) => {
        impl Persistable for $type {
            type Persisted = Self;

            fn get_persistent(&self) -> &Self::Persisted {
                self
            }
        }
    };
}
pub(crate) use impl_persistable;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_util::{harness, TestHarness};
    use rstest::rstest;

    #[rstest]
    fn test_persistent(_harness: TestHarness) {
        let mut persistent =
            Persistent::new(PersistentKey::RecipeId, "".to_owned());
        *persistent = "hello!".to_owned();
        // Trigger the save
        drop(persistent);

        let persistent =
            Persistent::new(PersistentKey::RecipeId, "".to_owned());
        assert_eq!(*persistent, "hello!".to_owned());
    }
}
