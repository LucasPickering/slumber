//! Utilities for persisting UI state between sessions

use crate::tui::{
    context::TuiContext,
    view::{
        component::Component,
        event::{Event, EventHandler, Update, UpdateContext},
    },
};
use derive_more::{Deref, DerefMut, Display};
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
                .get_ui::<_, <T::Value as Persistable>::Persisted>(key)
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
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        self.container.update(context, event)
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        self.container.children()
    }
}

impl<T: PersistentContainer> Drop for Persistent<T> {
    fn drop(&mut self) {
        let _ = TuiContext::get().database.set_ui(
            self.key,
            self.container.get().map(Persistable::get_persistent),
        );
    }
}

#[derive(Copy, Clone, Debug, Display)]
pub enum PersistentKey {
    PrimaryPane,
    FullscreenMode,
    ProfileId,
    RecipeId,
    RequestTab,
    ResponseTab,
}

/// A value type that can be persisted to the database
pub trait Persistable {
    /// The type of the value that's actually persisted to the database. In most
    /// cases this is the value itself, but some types use others (e.g. an ID).
    type Persisted: Debug + Serialize + DeserializeOwned + PartialEq<Self>;

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
    use crate::tui::context::tui_context;
    use rstest::rstest;

    #[rstest]
    fn test_persistent(_tui_context: ()) {
        let mut persistent = Persistent::new(PersistentKey::RecipeId, 0);
        *persistent = 37;
        // Trigger the save
        drop(persistent);

        let persistent = Persistent::new(PersistentKey::RecipeId, 0);
        assert_eq!(*persistent, 37);
    }
}
