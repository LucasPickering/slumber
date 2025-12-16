//! Overridable templates and single-session persistence for those overrides

use crate::view::{
    UpdateContext, ViewContext,
    common::{
        actions::MenuItem,
        template_preview::TemplatePreview,
        text_box::{TextBox, TextBoxEvent, TextBoxProps},
    },
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
    },
    event::{Emitter, Event, EventMatch, ToEmitter},
    persistent::{PersistentStore, SessionKey},
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    http::content_type::ContentType,
};
use slumber_template::Template;
use std::fmt::Debug;

/// A template that can be previewed, overridden, and persisted. Parent is
/// responsible for implementing the override behavior, and calling
/// [set_override](Self::set_override) when needed.
#[derive(Debug)]
pub struct OverrideTemplate {
    id: ComponentId,
    persistent_key: TemplateOverrideKey,
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
        persistent_key: TemplateOverrideKey,
        template: Template,
        content_type: Option<ContentType>,
        can_stream: bool,
    ) -> Self {
        let override_template = PersistentStore::get_session(&persistent_key);
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

    fn persist(&self, store: &mut PersistentStore) {
        // Persist to the session store. Overrides are meant to be temporary, so
        // we don't want to encourage users to rely on them long-term. They
        // should be making edits to their YAML file instead.
        if let Some(template) = &self.override_template {
            store.set_session(self.persistent_key.clone(), template.clone());
        } else {
            store.remove_session(&self.persistent_key);
        }
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.preview.to_child_mut()]
    }
}

impl Draw for OverrideTemplate {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(self.preview(), (), metadata.area(), false);
    }
}

/// An extension of [OverrideTemplate] that uses an inline text box to
/// enable editing. This handles edit/reset events itself and manages the state
/// of the text box.
#[derive(Debug)]
pub struct EditableTemplate {
    id: ComponentId,
    /// Descriptor for the *type* of template being shown, e.g. "Header"
    noun: &'static str,
    actions_emitter: Emitter<EditableTemplateMenuAction>,
    template: OverrideTemplate,
    /// An inline text box for editing the override template. `Some` only when
    /// editing.
    edit_text_box: Option<TextBox>,
    /// After a new valie template is submitted, should we send
    /// [Event::RefreshPreviews]? Use for profile fields, because those can
    /// affect other templates
    refresh_on_edit: bool,
}

impl EditableTemplate {
    /// Construct a new template that can be edited inline.
    ///
    /// ## Params
    ///
    /// - `persistent_key`: Key to store the override in the *session* store
    /// - `template`: Template being edited
    /// - `can_stream`: Is it possible for the output of this template to be
    ///   streamed? If `true`, the template will not be fully rendered in the
    ///   preview, as the output may be very large.
    /// - `refresh_on_edit`: Should all previews in the app be refreshed after
    ///   this template is modified? Use this for profile field templates,
    ///   because those can have downstream effects.
    pub fn new(
        noun: &'static str,
        persistent_key: TemplateOverrideKey,
        template: Template,
        can_stream: bool,
        refresh_on_edit: bool,
    ) -> Self {
        Self {
            id: ComponentId::default(),
            noun,
            actions_emitter: Emitter::default(),
            template: OverrideTemplate::new(
                persistent_key,
                template,
                // The only template that uses content_type is the body, and
                // that doesn't use inline editing so we don't have to support
                None,
                can_stream,
            ),
            edit_text_box: None,
            refresh_on_edit,
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
            if self.refresh_on_edit {
                ViewContext::push_event(Event::RefreshPreviews);
            }
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
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                EditableTemplateMenuAction::Edit => self.edit(),
                EditableTemplateMenuAction::Reset => self.reset_override(),
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

    fn menu(&self) -> Vec<MenuItem> {
        let noun = self.noun;
        vec![
            self.actions_emitter
                .menu(EditableTemplateMenuAction::Edit, format!("Edit {noun}"))
                .shortcut(Some(Action::Edit))
                .into(),
            self.actions_emitter
                .menu(
                    EditableTemplateMenuAction::Reset,
                    format!("Reset {noun}"),
                )
                .enable(self.is_overridden())
                .shortcut(Some(Action::Reset))
                .into(),
        ]
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
                TextBoxProps {
                    // This template is generally shown in a table, where the
                    // scrollbar can cover up other rows
                    scrollbar: false,
                    ..TextBoxProps::default()
                },
                metadata.area(),
                true,
            );
        } else {
            canvas.draw(self.preview(), (), metadata.area(), false);
        }
    }
}

/// Menu action for [EditableTemplate]
#[derive(Copy, Clone, Debug)]
enum EditableTemplateMenuAction {
    /// Edit the override
    Edit,
    /// Wipe ou the current override
    Reset,
}

/// Persisted key for override templates in the session store. This uniquely
/// identifies any piece of a recipe that can be overridden.
#[derive(Clone, Debug, PartialEq)]
pub enum TemplateOverrideKey {
    /// A profile field is overridden
    Profile {
        profile_id: ProfileId,
        field: String,
    },
    /// A piece of a recipe is overridden
    Recipe {
        recipe_id: RecipeId,
        kind: TemplateOverrideKeyKind,
    },
}

impl TemplateOverrideKey {
    pub fn profile(profile_id: ProfileId, field: String) -> Self {
        Self::Profile { profile_id, field }
    }

    pub fn url(recipe_id: RecipeId) -> Self {
        Self::Recipe {
            kind: TemplateOverrideKeyKind::Url,
            recipe_id,
        }
    }

    pub fn body(recipe_id: RecipeId) -> Self {
        Self::Recipe {
            kind: TemplateOverrideKeyKind::Body,
            recipe_id,
        }
    }

    pub fn auth_basic_username(recipe_id: RecipeId) -> Self {
        Self::Recipe {
            kind: TemplateOverrideKeyKind::AuthenticationBasicUsername,
            recipe_id,
        }
    }

    pub fn auth_basic_password(recipe_id: RecipeId) -> Self {
        Self::Recipe {
            kind: TemplateOverrideKeyKind::AuthenticationBasicPassword,
            recipe_id,
        }
    }

    pub fn auth_bearer_token(recipe_id: RecipeId) -> Self {
        Self::Recipe {
            kind: TemplateOverrideKeyKind::AuthenticationBearerToken,
            recipe_id,
        }
    }

    /// Get a unique key for a query parameter. This can use index instead of
    /// param name because it's only used within one session, and params can't
    /// be added/reordered/removed without reloading the collection.
    pub fn query_param(recipe_id: RecipeId, index: usize) -> Self {
        Self::Recipe {
            kind: TemplateOverrideKeyKind::QueryParam(index),
            recipe_id,
        }
    }

    /// Get a unique key for a header. This can use index instead of
    /// param name because it's only used within one session, and params can't
    /// be added/reordered/removed without reloading the collection.
    pub fn header(recipe_id: RecipeId, index: usize) -> Self {
        Self::Recipe {
            kind: TemplateOverrideKeyKind::Header(index),
            recipe_id,
        }
    }

    /// Get a unique key for a form field. This can use index instead of
    /// param name because it's only used within one session, and params can't
    /// be added/reordered/removed without reloading the collection.
    pub fn form_field(recipe_id: RecipeId, index: usize) -> Self {
        Self::Recipe {
            kind: TemplateOverrideKeyKind::FormField(index),
            recipe_id,
        }
    }
}

impl SessionKey for TemplateOverrideKey {
    type Value = Template;
}

/// Different kinds of recipe fields that can be persisted. This is should be
/// used through methods on [TemplateOverrideKey] to make usage a bit terser.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
pub enum TemplateOverrideKeyKind {
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
        let key = TemplateOverrideKey::url(recipe_id);
        harness
            .persistent_store()
            .set_session(key.clone(), "persisted".into());
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            EditableTemplate::new(
                "Item",
                key.clone(),
                "default".into(),
                false,
                false,
            ),
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
        assert_eq!(PersistentStore::get_session(&key), Some("override".into()));

        // Clear the override; should be removed from the store
        component.int().send_key(KeyCode::Char('z')).assert_empty();
        component.persist(&mut PersistentStore::new(harness.database));
        assert_eq!(component.template(), &"default".into());
        assert_eq!(PersistentStore::get_session(&key), None);
    }
}
