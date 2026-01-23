use crate::view::{
    UpdateContext, ViewContext,
    common::{
        actions::MenuItem,
        template_preview::{TemplatePreview, TemplatePreviewEvent},
        text_box::{TextBox, TextBoxEvent, TextBoxProps},
    },
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
    },
    event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
    persistent::SessionKey,
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
/// `PK` is the persistent key used to store override state in the session store
#[derive(Debug)]
pub struct EditableTemplate<PK> {
    id: ComponentId,
    /// Descriptor for the *type* of template being shown, e.g. "Header"
    noun: &'static str,
    actions_emitter: Emitter<EditableTemplateMenuAction>,
    /// Container for both the original and override templates
    preview: TemplatePreview<PK>,
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

impl<PK> EditableTemplate<PK> {
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
        template: Template,
        can_stream: bool,
        refresh_on_edit: bool,
    ) -> Self
    where
        PK: SessionKey<Value = Template>,
    {
        let preview =
            TemplatePreview::new(persistent_key, template, can_stream);
        let initial_text = preview.render_raw(); // Show raw while rendering
        Self {
            id: ComponentId::default(),
            noun,
            actions_emitter: Emitter::default(),
            preview,
            text: initial_text,
            edit_text_box: None,
            refresh_on_edit,
        }
    }

    /// Get the active template. If an override is present, return that.
    /// Otherwise return the original.
    pub fn template(&self) -> &Template {
        self.preview.template()
    }

    /// Get visible preview text
    pub fn text(&self) -> &Text {
        &self.text
    }

    /// Override the recipe with a new template
    pub fn set_override(&mut self, template: Template) {
        self.preview.set_override(template);
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
    pub fn reset_override(&mut self) {
        self.preview.reset_override();
    }

    /// Is a override template set?
    pub fn is_overridden(&self) -> bool {
        self.preview.is_overridden()
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
                ViewContext::push_event(BroadcastEvent::RefreshPreviews);
            }
        }
    }
}

impl<PK> Component for EditableTemplate<PK>
where
    PK: Clone + SessionKey<Value = Template>,
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

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            self.preview.to_child_mut(),
            self.edit_text_box.to_child_mut(),
        ]
    }
}

impl<PK> Draw for EditableTemplate<PK>
where
    PK: Clone + SessionKey<Value = Template>,
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
    use crate::{
        test_util::{TestTerminal, terminal},
        view::{
            persistent::PersistentStore,
            test_util::{TestComponent, TestHarness, harness},
        },
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
    fn test_persistence(harness: TestHarness, terminal: TestTerminal) {
        harness
            .persistent_store()
            .set_session(Key, "persisted".into());
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            EditableTemplate::new("Item", Key, "default".into(), false, false),
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
            .assert()
            .empty();
        assert_eq!(component.template(), &"override".into());
        assert_eq!(PersistentStore::get_session(&Key), Some("override".into()));

        // Clear the override; should be removed from the store
        component
            .int()
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
        component.persist(&mut PersistentStore::new(harness.database));
        assert_eq!(component.template(), &"default".into());
        assert_eq!(PersistentStore::get_session(&Key), None);
    }
}
