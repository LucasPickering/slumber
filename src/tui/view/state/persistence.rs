//! Utilities for persisting UI state between sessions

use crate::{
    collection::RecipeId,
    tui::{
        context::TuiContext,
        message::MessageSender,
        view::{
            component::Component,
            event::{Event, EventHandler, Update},
        },
    },
};
use derive_more::{Deref, DerefMut};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;

/// A wrapper for any value that will automatically persist it to the state DB.
/// The value will be loaded from the DB on creation, and saved to the DB on
/// drop.
#[derive(derive_more::Debug, Deref, DerefMut)]
pub struct Persistent<T: PersistentContainer> {
    key: PersistentKey,
    #[deref]
    #[deref_mut]
    container: T,
}

impl<T: PersistentContainer> Persistent<T> {
    /// Load the latest persisted value from the DB. If present, set the value
    /// of the container.
    pub fn new(key: PersistentKey, mut container: T) -> Self {
        // Load saved value from the database, and select it if available
        if let Ok(Some(value)) =
            TuiContext::get()
                .database
                .get_ui::<_, <T::Value as Persistable>::Persisted>(&key)
        {
            container.set(value);
        }

        Self { key, container }
    }
}

/// Forward events to the inner state cell
impl<T> EventHandler for Persistent<T>
where
    T: EventHandler + PersistentContainer,
{
    fn update(&mut self, messages_tx: &MessageSender, event: Event) -> Update {
        self.container.update(messages_tx, event)
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        self.container.children()
    }
}

impl<T: PersistentContainer> Drop for Persistent<T> {
    fn drop(&mut self) {
        let _ = TuiContext::get().database.set_ui(
            &self.key,
            self.container.get().map(Persistable::get_persistent),
        );
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
}

/// A value type that can be persisted to the database
pub trait Persistable {
    /// The type of the value that's actually persisted to the database. In most
    /// cases this is the value itself, but some types use others (e.g. an ID).
    type Persisted: Debug + Serialize + DeserializeOwned;

    /// Get the value that should be persisted to the DB
    fn get_persistent(&self) -> &Self::Persisted;
}

/// Any simple type can be persisted. The `Copy` bound isn't strictly necessary,
/// but it restricts the blanket to only simple types.
impl<T> Persistable for T
where
    T: Copy + Debug + PartialEq + Serialize + DeserializeOwned,
{
    type Persisted = Self;

    fn get_persistent(&self) -> &Self::Persisted {
        self
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use rstest::rstest;

    #[rstest]
    fn test_persistent(_tui_context: &TuiContext) {
        let mut persistent = Persistent::new(PersistentKey::RecipeId, 0);
        *persistent = 37;
        // Trigger the save
        drop(persistent);

        let persistent = Persistent::new(PersistentKey::RecipeId, 0);
        assert_eq!(*persistent, 37);
    }
}
