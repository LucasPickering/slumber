//! Overridable templates and single-session persistence for those overrides

use crate::{
    util::PersistentStore,
    view::{
        UpdateContext,
        common::{
            template_preview::TemplatePreview,
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{Event, EventMatch, ToEmitter},
    },
};
use slumber_config::Action;
use slumber_core::{collection::RecipeId, http::content_type::ContentType};
use slumber_template::Template;
use std::{cell::RefCell, collections::HashMap, fmt::Debug};
use tracing::debug;

/// Special single-session persistence store just for edited recipe templates.
/// This is to discourage users from making long-term overrides. We push them
/// toward modifying the YAML instead because:
/// - It keeps it as a single source of truth
/// - Changes will be shared if using version control
/// - Overrides may be more brittle/fragile than the user realizes
///
/// "Single-session persistence" sounds like an oxymoron; why persist at all?
/// We need this to maintain overrides when the collection file is modified.
/// It's possible someone has a few overrides in place, then modifies the YAML
/// file. That triggers a rebuild of the entire view, but we don't want to wipe
/// out the template overrides! That could be frustrating. We want them to only
/// clear when the entire process exits.
///
/// This is public just for getting/setting values in component tests.
#[derive(Debug, Default)]
pub(super) struct RecipeOverrideStore(HashMap<RecipeOverrideKey, Template>);

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
pub struct OverrideTemplate {
    id: ComponentId,
    persistent_key: RecipeOverrideKey,
    /// The template from the collection
    original_template: Template,
    /// Temporary override entered by the user
    #[expect(clippy::struct_field_names)]
    override_template: Option<Template>,
    preview: TemplatePreview,
    /// Retain this so we can rebuild the preview with it
    content_type: Option<ContentType>,
    /// Does the consumer support streams, or does everything have to be
    /// resolved to a concrete value?
    can_stream: bool,
}

impl OverrideTemplate {
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
            id: ComponentId::default(),
            persistent_key,
            original_template: template,
            override_template,
            preview,
            content_type,
            can_stream,
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
        // If this matches the original template, it's not an override
        if template == self.original_template {
            self.override_template = None;
        } else {
            self.override_template = Some(template);
        }
        self.render_preview();
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
    pub fn reset_override(&mut self) {
        self.override_template = None;
        self.render_preview();
    }

    /// Is a override template set?
    pub fn is_overridden(&self) -> bool {
        self.override_template.is_some()
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

impl Component for OverrideTemplate {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn persist(&self, _store: &mut PersistentStore) {
        // We persist to our local store instead of the DB
        if let Some(template) = &self.override_template {
            RecipeOverrideStore::set(&self.persistent_key, template);
        } else {
            RecipeOverrideStore::remove(&self.persistent_key);
        }
    }
}

impl Draw for OverrideTemplate {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.render_widget(self.preview(), metadata.area());
    }
}

/// An extension of [OverrideTemplate] that uses an inline text box to
/// enable editing. This handles edit/reset events itself and manages the state
/// of the text box.
#[derive(Debug)]
pub struct EditableTemplate {
    id: ComponentId,
    template: OverrideTemplate,
    /// An inline text box for editing the override template. `Some` only when
    /// editing.
    edit_text_box: Option<TextBox>,
}

impl EditableTemplate {
    pub fn new(
        persistent_key: RecipeOverrideKey,
        template: Template,
        content_type: Option<ContentType>,
        can_stream: bool,
    ) -> Self {
        Self {
            id: ComponentId::default(),
            template: OverrideTemplate::new(
                persistent_key,
                template,
                content_type,
                can_stream,
            ),
            edit_text_box: None,
        }
    }

    /// Get the active template. If an override is present, return that.
    /// Otherwise return the original.
    pub fn template(&self) -> &Template {
        self.template.template()
    }

    /// Get the active template preview. If an override is present, return that.
    /// Otherwise return the original.
    pub fn preview(&self) -> &TemplatePreview {
        self.template.preview()
    }

    /// Override the recipe with a new template
    pub fn set_override(&mut self, template: Template) {
        self.template.set_override(template);
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
    pub fn reset_override(&mut self) {
        self.template.reset_override();
    }

    /// Is a override template set?
    pub fn is_overridden(&self) -> bool {
        self.template.is_overridden()
    }

    /// Enter edit mode
    pub fn edit(&mut self) {
        let template = self.template().display().into_owned();
        self.edit_text_box = Some(
            TextBox::default()
                .default_value(template)
                .subscribe([TextBoxEvent::Cancel, TextBoxEvent::Submit])
                .validator(|value| value.parse::<Template>().is_ok()),
        );
    }

    /// Stop editing and save the current template as the override. If the
    /// current value is invalid, revert to the original.
    pub fn submit_edit(&mut self) {
        // Should always be defined when submission is triggered
        let Some(text_box) = self.edit_text_box.take() else {
            return;
        };

        // It's possible to attempt a submit while the current template is
        // invalid (if the user de-selects this template). In this case, we
        // toss the edits
        if let Ok(template) = text_box.into_text().parse::<Template>() {
            self.set_override(template);
        }
    }
}

impl Component for EditableTemplate {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Edit => self.edit(),
                Action::Reset => self.reset_override(),
                _ => propagate.set(),
            })
            .emitted_opt(
                self.edit_text_box.as_ref().map(ToEmitter::to_emitter),
                |event| match event {
                    TextBoxEvent::Change => {}
                    TextBoxEvent::Cancel => self.edit_text_box = None,
                    TextBoxEvent::Submit => self.submit_edit(),
                },
            )
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            self.template.to_child_mut(),
            self.edit_text_box.to_child_mut(),
        ]
    }
}

impl Draw for EditableTemplate {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        if let Some(edit_text_box) = &self.edit_text_box {
            canvas.draw(
                edit_text_box,
                TextBoxProps::default(),
                metadata.area(),
                true,
            );
        } else {
            canvas.render_widget(self.preview(), metadata.area());
        }
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
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use rstest::rstest;
    use slumber_util::Factory;
    use std::iter;
    use terminput::KeyCode;

    /// Test persisting and restoring overrides
    #[rstest]
    fn test_persistence(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        let key = RecipeOverrideKey::url(recipe_id);
        RecipeOverrideStore::set(&key, &"persisted".into());
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            EditableTemplate::new(key.clone(), "default".into(), None, false),
        );

        // Persisted value is loaded on creation
        assert_eq!(component.template(), &"persisted".into());

        // Modify the override and persist, should be updated in the store
        component
            .int()
            // Edit and replace the text
            .send_key(KeyCode::Char('e'))
            .send_keys(iter::repeat_n(KeyCode::Backspace, 10))
            .send_text("override")
            .send_key(KeyCode::Enter)
            .assert_empty();
        assert_eq!(component.template(), &"override".into());
        assert_eq!(RecipeOverrideStore::get(&key), Some("override".into()));

        // Clear the override; should be removed from the store
        component.int().send_key(KeyCode::Char('z')).assert_empty();
        component.persist(&mut PersistentStore::new(&harness.database));
        assert_eq!(component.template(), &"default".into());
        assert_eq!(RecipeOverrideStore::get(&key), None);
    }
}
