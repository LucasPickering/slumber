//! Single-session persistence for recipe overrides

use crate::{
    context::TuiContext,
    view::{
        ViewContext, common::template_preview::TemplatePreview,
        state::Identified,
    },
};
use persisted::{PersistedContainer, PersistedLazy, PersistedStore};
use ratatui::text::Text;
use slumber_core::{
    collection::RecipeId, http::content_type::ContentType, template::Template,
};
use std::{collections::HashMap, fmt::Debug, ops::Deref};
use tracing::debug;

/// Special single-session [PersistedStore] just for edited recipe templates.
/// We don't want to store recipe overrides across sessions, because they could
/// be very large and conflict with changes in the recipe. Using a dedicated
/// type for this makes the generic bounds stricter which is nice.
///
/// To persist something in this store, you probably want to use
/// [RecipeTemplate] for your component/state field.
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
        // The value will be None if the template isn't overridden, in which
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

/// A template that can be previewed, overridden, and persisted. Parent is
/// responsible for implementing the override behavior, and calling
/// [set_override](Self::set_override) when needed.
#[derive(Debug)]
pub struct RecipeTemplate(
    persisted::PersistedLazy<
        RecipeOverrideStore,
        RecipeOverrideKey,
        RecipeTemplateInner,
    >,
);

impl RecipeTemplate {
    pub fn new(
        persisted_key: RecipeOverrideKey,
        template: Template,
        content_type: Option<ContentType>,
    ) -> Self {
        Self(PersistedLazy::new(
            persisted_key,
            RecipeTemplateInner {
                override_value: None,
                preview: TemplatePreview::new(template, content_type),
                content_type,
            },
        ))
    }

    /// Override the template with a new output value. The value will *not* be
    /// rendered; it's treated literally.
    pub fn set_override(&mut self, value: String) {
        self.0.get_mut().set_override(value);
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
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

    /// Get renderable text for this preview. If there is an override value,
    /// show that. Otherwise, show the template preview.
    pub fn text(&self) -> impl Deref<Target = Identified<Text<'static>>> {
        if let Some(value) = &self.0.override_value {
            let styles = &TuiContext::get().styles;
            // value.generate().style(styles.text.edited)
            todo!()
        } else {
            self.0.preview.text()
        }
    }

    /// TODO
    pub fn text_cloned(&self) -> Text {
        (**self.text()).clone()
    }

    pub fn content_type(&self) -> Option<ContentType> {
        self.0.content_type
    }

    pub fn is_overridden(&self) -> bool {
        self.0.override_value.is_some()
    }
}

/// A template that can be previewed and overridden. Parent is responsible for
/// implementing the override behavior, and calling
/// [set_override](Self::set_override) when needed.
#[derive(Debug)]
struct RecipeTemplateInner {
    override_value: Option<String>,
    preview: TemplatePreview,
    /// Retain this so we can rebuild the preview with it
    content_type: Option<ContentType>,
}

impl RecipeTemplateInner {
    fn set_override(&mut self, value: String) {
        // TODO remove clone
        self.override_value = Some(value.clone());
    }

    fn reset_override(&mut self) {
        self.override_value = None;
    }
}

impl PersistedContainer for RecipeTemplateInner {
    type Value = RecipeOverrideValue;

    fn get_to_persist(&self) -> Self::Value {
        match &self.override_value {
            Some(value) => RecipeOverrideValue::Override(value.clone()),
            None => RecipeOverrideValue::Default,
        }
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        if let RecipeOverrideValue::Override(template) = value {
            self.set_override(template);
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
