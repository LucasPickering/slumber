//! Single-session persistence for recipe overrides

use crate::{
    context::TuiContext,
    view::{
        ViewContext, common::preview::Preview, state::Identified,
        util::highlight,
    },
};
use persisted::{PersistedContainer, PersistedLazy, PersistedStore};
use ratatui::text::Text;
use slumber_core::{
    collection::RecipeId, http::content_type::ContentType, render::Procedure,
};
use std::{collections::HashMap, fmt::Debug};
use tracing::debug;

/// Special single-session [PersistedStore] just for edited recipe procedures.
/// We don't want to store recipe overrides across sessions, because they could
/// be very large and conflict with changes in the recipe. Using a dedicated
/// type for this makes the generic bounds stricter which is nice.
///
/// To persist something in this store, you probably want to use
/// [RecipeProcedure] for your component/state field.
#[derive(Debug, Default)]
pub struct RecipeOverrideStore(HashMap<RecipeOverrideKey, String>);

impl PersistedStore<RecipeOverrideKey> for RecipeOverrideStore {
    fn load_persisted(key: &RecipeOverrideKey) -> Option<RecipeOverrideValue> {
        if let Some(value) =
            ViewContext::with_override_store(|store| store.0.get(key).cloned())
        {
            // Only overridden values are persisted
            debug!(?key, ?value, "Loaded persisted recipe override");
            Some(RecipeOverrideValue::Override(value))
        } else {
            None
        }
    }

    fn store_persisted(key: &RecipeOverrideKey, value: &RecipeOverrideValue) {
        // The value will be None if the procedure isn't overridden, in which
        // case we don't want to store it
        if let RecipeOverrideValue::Override(value) = value {
            debug!(?key, ?value, "Persisting recipe override");
            ViewContext::with_override_store_mut(|store| {
                store.0.insert(key.clone(), value.clone());
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
    /// User has provided an override value for this field, persist it
    Override(String),
}

/// A procedure that can be previewed, overridden, and persisted. Parent is
/// responsible for implementing the override behavior and calling
/// [set_override](Self::set_override) when needed.
#[derive(Debug)]
pub struct RecipeProcedure(
    persisted::PersistedLazy<
        RecipeOverrideStore,
        RecipeOverrideKey,
        RecipeProcedureInner,
    >,
);

impl RecipeProcedure {
    pub fn new(
        persisted_key: RecipeOverrideKey,
        procedure: Procedure,
        content_type: Option<ContentType>,
    ) -> Self {
        Self(PersistedLazy::new(
            persisted_key,
            RecipeProcedureInner {
                override_value: None,
                override_text: None,
                preview: Preview::new(procedure, content_type),
                content_type,
            },
        ))
    }

    /// Override the procedure with a new output value. The value will *not* be
    /// rendered; it's treated literally.
    pub fn set_override(&mut self, value: String) {
        self.0.get_mut().set_override(value);
    }

    /// Reset the procedure override to the default from the recipe, and
    /// recompute the procedure preview
    pub fn reset_override(&mut self) {
        self.0.get_mut().reset_override();
    }

    /// TODO
    pub fn value(&self) -> String {
        self.0
            .override_value
            .clone()
            .unwrap_or_else(|| self.0.preview.text().to_string())
    }

    /// Call a function with access to the visible text of this procedure. This
    /// will use the override text if the user has set one, otherwise the
    /// rendered preview.
    /// TODO come up with a better pattern than this
    pub fn with_text(&self, f: impl FnOnce(&Identified<Text<'static>>)) {
        let text = if let Some(text) = &self.0.override_text {
            text
        } else {
            self.0.preview.text()
        };
        f(text)
    }

    /// Get a clone of the text of this procedure. If an override is available,
    /// get that. Otherwise get the preview.
    pub fn text_cloned(&self) -> Text {
        if let Some(text) = &self.0.override_text {
            (*text).clone()
        } else {
            (**self.0.preview.text()).clone()
        }
    }

    pub fn is_overridden(&self) -> bool {
        self.0.override_value.is_some()
    }
}

#[derive(Debug)]
struct RecipeProcedureInner {
    /// Raw string value for overriden text
    override_value: Option<String>,
    /// Overridden display text. This is the same content as `override_value`,
    /// but has syntax highlighting applied as appropriate and has been broken
    /// into a [Text]
    override_text: Option<Identified<Text<'static>>>,
    preview: Preview,
    /// Retain this so we can rebuild the preview with it
    content_type: Option<ContentType>,
}

impl RecipeProcedureInner {
    fn set_override(&mut self, value: String) {
        // Clone is necessary because we can't have the text object
        // self-reference the string
        // TODO should we store _just_ text and regenerate the string at request
        // time?
        self.override_value = Some(value.clone());
        let styles = &TuiContext::get().styles;
        let text = highlight::highlight_if(
            self.content_type,
            Text::styled(value, styles.text.edited),
        );
        self.override_text = Some(text.into());
    }

    fn reset_override(&mut self) {
        self.override_value = None;
        self.override_text = None;
    }
}

impl PersistedContainer for RecipeProcedureInner {
    type Value = RecipeOverrideValue;

    fn get_to_persist(&self) -> Self::Value {
        match &self.override_value {
            Some(value) => RecipeOverrideValue::Override(value.clone()),
            None => RecipeOverrideValue::Default,
        }
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        if let RecipeOverrideValue::Override(value) = value {
            self.set_override(value);
        }
    }
}

/// Persisted key for anything that goes in [RecipeOverrideStore]. This uniquely
/// identifies any piece of a recipe that can be overridden.
#[derive(Clone, Debug, Eq, Hash, PartialEq, persisted::PersistedKey)]
#[persisted(RecipeOverrideValue)]
pub struct RecipeOverrideKey {
    kind: RecipeOverrideKeyKind,
    recipe_id: RecipeId,
}

impl RecipeOverrideKey {
    pub fn body(recipe_id: RecipeId) -> Self {
        Self {
            kind: RecipeOverrideKeyKind::Body,
            recipe_id,
        }
    }

    pub fn auth_basic_username(recipe_id: RecipeId) -> Self {
        Self {
            kind: RecipeOverrideKeyKind::AuthenticationBasicUsername,
            recipe_id,
        }
    }

    pub fn auth_basic_password(recipe_id: RecipeId) -> Self {
        Self {
            kind: RecipeOverrideKeyKind::AuthenticationBasicPassword,
            recipe_id,
        }
    }

    pub fn auth_bearer_token(recipe_id: RecipeId) -> Self {
        Self {
            kind: RecipeOverrideKeyKind::AuthenticationBearerToken,
            recipe_id,
        }
    }

    /// Get a unique key for a query parameter. This can use index instead of
    /// param name because it's only used within one session, and params can't
    /// be added/reordered/removed without reloading the collection.
    pub fn query_param(recipe_id: RecipeId, index: usize) -> Self {
        Self {
            kind: RecipeOverrideKeyKind::QueryParam(index),
            recipe_id,
        }
    }

    /// Get a unique key for a header. This can use index instead of
    /// param name because it's only used within one session, and params can't
    /// be added/reordered/removed without reloading the collection.
    pub fn header(recipe_id: RecipeId, index: usize) -> Self {
        Self {
            kind: RecipeOverrideKeyKind::Header(index),
            recipe_id,
        }
    }

    /// Get a unique key for a form field. This can use index instead of
    /// param name because it's only used within one session, and params can't
    /// be added/reordered/removed without reloading the collection.
    pub fn form_field(recipe_id: RecipeId, index: usize) -> Self {
        Self {
            kind: RecipeOverrideKeyKind::FormField(index),
            recipe_id,
        }
    }
}

/// Different kinds of recipe fields that can be persisted. This is exposed only
/// through methods on [RecipeOverrideKey] to make usage a bit terser.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum RecipeOverrideKeyKind {
    Body,
    AuthenticationBasicUsername,
    AuthenticationBasicPassword,
    AuthenticationBearerToken,
    QueryParam(usize),
    Header(usize),
    FormField(usize),
}
