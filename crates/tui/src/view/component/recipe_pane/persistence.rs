//! Single-session persistence for recipe overrides

use crate::{
    util::ResultReported,
    view::{
        ViewContext,
        common::{template_preview::TemplatePreview, text_box::TextBox},
        component::misc::TextBoxModal,
    },
};
use slumber_core::{collection::RecipeId, http::content_type::ContentType};
use slumber_template::Template;
use std::{cell::RefCell, collections::HashMap, fmt::Debug};
use tracing::debug;

/// Special single-session persistence store just for edited recipe templates.
/// We don't want to store recipe overrides across sessions, because they could
/// be very large and conflict with changes in the recipe. Using a dedicated
/// type for this makes the generic bounds stricter which is nice.
///
/// To persist something in this store, you probably want to use
/// [RecipeTemplate] for your component/state field.
#[derive(Debug, Default)]
pub struct RecipeOverrideStore(HashMap<RecipeOverrideKey, Template>);

impl RecipeOverrideStore {
    thread_local! {
        /// Static instance for the store. Persistence is handled in the main
        /// view thread. We could potentially put this in the view context, but
        /// isolating it here limits what we need to borrow from the cell to
        /// just what we need. It also prevents external access to the store.
        static INSTANCE: RefCell<RecipeOverrideStore> = RefCell::default();
    }

    /// Get a persisted value from the store
    pub fn get(key: &RecipeOverrideKey) -> Option<Template> {
        Self::INSTANCE
            .with_borrow(|store| store.0.get(key).cloned())
            .inspect(|template| {
                debug!(?key, ?template, "Loaded persisted recipe override");
            })
    }

    /// Set a persisted value in the store
    pub fn set(key: &RecipeOverrideKey, template: &Template) {
        debug!(?key, ?template, "Persisting recipe override");
        Self::INSTANCE.with_borrow_mut(|store| {
            store.0.insert(key.clone(), template.clone());
        });
    }

    /// Remove a persisted value from the store
    fn remove(key: &RecipeOverrideKey) {
        debug!(?key, "Resetting recipe override");
        Self::INSTANCE.with_borrow_mut(|store| store.0.remove(key));
    }
}

/// A template that can be previewed, overridden, and persisted. Parent is
/// responsible for implementing the override behavior, and calling
/// [set_override](Self::set_override) when needed.
#[derive(Debug)]
pub struct RecipeTemplate {
    persistent_key: RecipeOverrideKey,
    original_template: Template,
    override_template: Option<Template>,
    preview: TemplatePreview,
    /// Retain this so we can rebuild the preview with it
    content_type: Option<ContentType>,
    /// Does the consumer support streams, or does everything have to be
    /// resolved to a concrete value?
    can_stream: bool,
}

impl RecipeTemplate {
    pub fn new(
        persistent_key: RecipeOverrideKey,
        template: Template,
        content_type: Option<ContentType>,
        can_stream: bool,
    ) -> Self {
        let override_template = RecipeOverrideStore::get(&persistent_key);
        let preview = TemplatePreview::new(
            override_template.as_ref().unwrap_or(&template).clone(),
            content_type,
            override_template.is_some(),
            can_stream,
        );
        Self {
            persistent_key,
            original_template: template,
            override_template,
            preview,
            content_type,
            can_stream,
        }
    }

    /// Persist the current override (if any) to the [RecipeOverrideStore]
    pub fn persist(&self) {
        if let Some(template) = &self.override_template {
            RecipeOverrideStore::set(&self.persistent_key, template);
        } else {
            RecipeOverrideStore::remove(&self.persistent_key);
        }
    }

    /// Get the active template. If an override is present, return that.
    /// Otherwise return the original.
    pub fn template(&self) -> &Template {
        self.override_template
            .as_ref()
            .unwrap_or(&self.original_template)
    }

    /// Get the active template preview. If an override is present, return that.
    /// Otherwise return the original.
    pub fn preview(&self) -> &TemplatePreview {
        &self.preview
    }

    /// Override the recipe with a new template
    pub fn set_override(&mut self, template: Template) {
        self.override_template = Some(template);
        self.render_preview();
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
    pub fn reset_override(&mut self) {
        self.override_template = None;
        self.render_preview();
    }

    pub fn is_overridden(&self) -> bool {
        self.override_template.is_some()
    }

    /// Create a modal that will edit this template in a text box. This **does
    /// not** open the modal. The parent has to open and store the modal,
    /// because modals aren't necessarily 1:1 with templates (e.g. a table
    /// has multiple templates but only one edit modal).
    pub fn edit_modal(
        &self,
        title: String,
        on_submit: impl 'static + FnOnce(Template),
    ) -> TextBoxModal {
        let template = self.template().display().into_owned();
        TextBoxModal::new(
            title,
            TextBox::default()
                .default_value(template)
                .validator(|value| value.parse::<Template>().is_ok()),
            move |value| {
                // The template *should* always parse because the text box
                // has a validator, but this is just a safety check
                if let Some(template) = value
                    .parse::<Template>()
                    .reported(&ViewContext::messages_tx())
                {
                    on_submit(template);
                }
            },
        )
    }

    /// Update the preview based on the current template. Call after any changes
    /// to the override
    fn render_preview(&mut self) {
        self.preview = TemplatePreview::new(
            self.template().clone(),
            self.content_type,
            self.override_template.is_some(),
            self.can_stream,
        );
    }
}

/// Persisted key for anything that goes in [RecipeOverrideStore]. This uniquely
/// identifies any piece of a recipe that can be overridden.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RecipeOverrideKey {
    kind: RecipeOverrideKeyKind,
    recipe_id: RecipeId,
}

impl RecipeOverrideKey {
    pub fn url(recipe_id: RecipeId) -> Self {
        Self {
            kind: RecipeOverrideKeyKind::Url,
            recipe_id,
        }
    }

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
    Url,
    Body,
    AuthenticationBasicUsername,
    AuthenticationBasicPassword,
    AuthenticationBearerToken,
    QueryParam(usize),
    Header(usize),
    FormField(usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TestHarness, harness};
    use rstest::rstest;
    use slumber_util::Factory;

    /// Test persisting and restoring overrides
    #[rstest]
    fn test_persistence(_harness: TestHarness) {
        let recipe_id = RecipeId::factory(());
        let key = RecipeOverrideKey::url(recipe_id);
        RecipeOverrideStore::set(&key, &"persisted".into());
        // Persisted value is loaded on creation
        let mut template =
            RecipeTemplate::new(key.clone(), "default".into(), None, false);
        assert_eq!(template.template(), &"persisted".into());

        // Modify the override and persist, should be updated in the store
        template.set_override("override".into());
        template.persist();
        assert_eq!(template.template(), &"override".into());
        assert_eq!(RecipeOverrideStore::get(&key), Some("override".into()));

        // Clear the override; should be removed from the store
        template.reset_override();
        template.persist();
        assert_eq!(template.template(), &"default".into());
        assert_eq!(RecipeOverrideStore::get(&key), None);
    }
}
