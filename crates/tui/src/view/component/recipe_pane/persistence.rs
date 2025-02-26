//! Single-session persistence for recipe overrides

use crate::view::{ViewContext, common::template_preview::TemplatePreview};
use persisted::{PersistedContainer, PersistedLazy, PersistedStore};
use slumber_core::{
    collection::RecipeId, http::content_type::ContentType, template::Template,
};
use std::{collections::HashMap, fmt::Debug};
use tracing::debug;

/// Special single-session [PersistedStore] just for edited recipe templates.
/// We don't want to store recipe overrides across sessions, because they could
/// be very large and conflict with changes in the recipe. Using a dedicated
/// type for this makes the generic bounds stricter which is nice.
///
/// To persist something in this store, you probably want to use
/// [RecipeTemplate] for your component/state field.
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
                original_template: template.clone(),
                override_template: None,
                preview: TemplatePreview::new(template, content_type, false),
                content_type,
            },
        ))
    }

    /// Override the recipe with a new template
    pub fn set_override(&mut self, template: Template) {
        self.0.get_mut().set_override(template);
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
    pub fn reset_override(&mut self) {
        self.0.get_mut().reset_override();
    }

    pub fn template(&self) -> &Template {
        self.0.template()
    }

    pub fn preview(&self) -> &TemplatePreview {
        &self.0.preview
    }

    pub fn content_type(&self) -> Option<ContentType> {
        self.0.content_type
    }

    pub fn is_overridden(&self) -> bool {
        self.0.override_template.is_some()
    }
}

/// A template that can be previewed and overridden. Parent is responsible for
/// implementing the override behavior, and calling
/// [set_override](Self::set_override) when needed.
#[derive(Debug)]
struct RecipeTemplateInner {
    original_template: Template,
    override_template: Option<Template>,
    preview: TemplatePreview,
    /// Retain this so we can rebuild the preview with it
    content_type: Option<ContentType>,
}

impl RecipeTemplateInner {
    fn template(&self) -> &Template {
        self.override_template
            .as_ref()
            .unwrap_or(&self.original_template)
    }

    fn set_override(&mut self, template: Template) {
        self.override_template = Some(template);
        self.render_preview();
    }

    fn reset_override(&mut self) {
        self.override_template = None;
        self.render_preview();
    }

    fn render_preview(&mut self) {
        self.preview = TemplatePreview::new(
            self.template().clone(),
            self.content_type,
            self.override_template.is_some(),
        );
    }
}

impl PersistedContainer for RecipeTemplateInner {
    type Value = RecipeOverrideValue;

    fn get_to_persist(&self) -> Self::Value {
        match &self.override_template {
            Some(template) => RecipeOverrideValue::Override(template.clone()),
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
