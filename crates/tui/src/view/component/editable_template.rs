use crate::view::{
    UpdateContext, ViewContext,
    common::{
        actions::MenuItem,
        template_preview::{Preview, TemplatePreview, TemplatePreviewEvent},
        text_box::{TextBox, TextBoxEvent, TextBoxProps},
    },
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
    },
    event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
    persistent::{PersistentStore, SessionKey},
};
use ratatui::text::Text;
use slumber_config::Action;
use slumber_template::Template;
use std::fmt::Debug;

/// A component for a template that can be edited in the UI
///
/// This handles:
/// - Storing both the original and override template
/// - Persisting the override in the session store
/// - Override can be edited with a hotkeyaction. Edit text box is shown inline
/// - Override can be reset with a hotkey/action
///
/// - `PK` is the persistent key used to store override state in the session
///   store
/// - `T` is the template type. Typically [Template] but could be anything
///   implementing [Preview]
#[derive(Debug)]
pub struct EditableTemplate<PK, T = Template> {
    id: ComponentId,
    /// Descriptor for the *type* of template being shown, e.g. "Header"
    noun: &'static str,
    actions_emitter: Emitter<EditableTemplateMenuAction>,
    /// The template from the collection
    original_template: T,
    /// Temporary override entered by the user
    override_template: Option<T>,
    /// Session store key to persist the override template
    persistent_key: PK,
    /// Container for both the original and override templates
    preview: TemplatePreview<T>,
    /// Rendered preview text
    text: Text<'static>,
    /// An inline text box for editing the override template. `Some` only when
    /// editing.
    edit_text_box: Option<TextBox>,
    /// After a new valie template is submitted, should we send
    /// [BroadcastEvent::RefreshPreviews]? Enable for profile fields, because
    /// those can affect other templates
    refresh_on_edit: bool,
}

impl<PK, T: Preview> EditableTemplate<PK, T> {
    /// Construct a new template that can be edited inline.
    ///
    /// ## Params
    ///
    /// - `noun`: Name of the thing this template renders to, e.g. "Header"
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
        persistent_key: PK,
        template: T,
        can_stream: bool,
        refresh_on_edit: bool,
    ) -> Self
    where
        PK: SessionKey<Value = T>,
    {
        let override_template = PersistentStore::get_session(&persistent_key);
        let (preview, text) = TemplatePreview::new(
            override_template.as_ref().unwrap_or(&template).clone(),
            can_stream,
            override_template.is_some(),
        );

        Self {
            id: ComponentId::default(),
            noun,
            actions_emitter: Emitter::default(),
            original_template: template,
            override_template,
            persistent_key,
            preview,
            text,
            edit_text_box: None,
            refresh_on_edit,
        }
    }

    /// Get the active template. If an override is present, return that.
    /// Otherwise return the original.
    pub fn template(&self) -> &T {
        self.override_template
            .as_ref()
            .unwrap_or(&self.original_template)
    }

    /// Get visible preview text
    pub fn text(&'_ self) -> &'_ Text<'_> {
        &self.text
    }

    /// Override the recipe with a new template
    pub fn set_override(&mut self, template: T) {
        if template == self.original_template {
            // If this matches the original template, it's not an override
            self.set_override_opt(None);
        } else if Some(&template) != self.override_template.as_ref() {
            // Only rerender if the override changed
            self.set_override_opt(Some(template));
        }
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
    pub fn reset_override(&mut self) {
        self.set_override_opt(None);
    }

    /// Internal helper to set/reset the override template and refresh the
    /// preview
    fn set_override_opt(&mut self, override_template: Option<T>) {
        self.override_template = override_template;
        // Re-render the preview
        (self.preview, self.text) = TemplatePreview::new(
            self.template().clone(),
            false,
            self.is_overridden(),
        );
    }

    /// Is a override template set?
    pub fn is_overridden(&self) -> bool {
        self.override_template.is_some()
    }

    /// Enter edit mode
    pub fn edit(&mut self) {
        // TODO open in external editor if multiple lines
        let template = self.template().display().into_owned();
        self.edit_text_box = Some(
            TextBox::default()
                .default_value(template)
                .subscribe([TextBoxEvent::Cancel, TextBoxEvent::Submit])
                .validator(|value| value.parse::<T>().is_ok()),
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
        if let Ok(template) = text_box.into_text().parse::<T>() {
            self.set_override(template);
            if self.refresh_on_edit {
                ViewContext::push_message(BroadcastEvent::RefreshPreviews);
            }
        }
    }
}

impl<PK, T> Component for EditableTemplate<PK, T>
where
    PK: Clone + SessionKey<Value = T>,
    T: Preview,
{
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
            // Store text rendered by the template preview
            .emitted(self.preview.to_emitter(), |TemplatePreviewEvent(text)| {
                self.text = text;
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
        vec![self.preview.to_child(), self.edit_text_box.to_child()]
    }
}

impl<PK, T> Draw for EditableTemplate<PK, T>
where
    PK: Clone + SessionKey<Value = T>,
    T: Preview,
{
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
            canvas.render_widget(&self.text, metadata.area());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{
        persistent::PersistentStore,
        test_util::{TestComponent, TestHarness, harness},
    };
    use rstest::rstest;
    use std::iter;
    use terminput::KeyCode;

    /// Persistent key for testing
    #[derive(Clone, Debug, PartialEq)]
    struct Key;

    impl SessionKey for Key {
        type Value = Template;
    }

    /// Test persisting and restoring overrides
    #[rstest]
    fn test_persistence(mut harness: TestHarness) {
        harness.set_session(Key, "persisted".into());
        let mut component = TestComponent::new(
            &mut harness,
            EditableTemplate::new("Item", Key, "default".into(), false, false),
        );

        // Persisted value is loaded on creation
        assert_eq!(component.template(), &"persisted".into());

        // Modify the override and persist, should be updated in the store
        component
            .int(&mut harness)
            // Edit and replace the text
            .send_key(KeyCode::Char('e'))
            .send_keys(iter::repeat_n(KeyCode::Backspace, 10))
            .send_text("override")
            .send_key(KeyCode::Enter)
            .assert()
            .empty();
        assert_eq!(component.template(), &"override".into());
        assert_eq!(PersistentStore::get_session(&Key), Some("override".into()));

        // Clear the override; should be removed from the store
        component
            .int(&mut harness)
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
        assert_eq!(component.template(), &"default".into());
        assert_eq!(PersistentStore::get_session(&Key), None);
    }
}
