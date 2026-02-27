use crate::{
    message::Message,
    util::{ResultReported, TempFile, syntax::SyntaxType},
    view::{
        Generate, UpdateContext, ViewContext,
        common::{
            actions::MenuItem,
            template_preview::{
                Preview, TemplatePreview, TemplatePreviewEvent,
            },
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
            text_window::{ScrollMode, TextWindow, TextWindowProps},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentStore, SessionKey},
        util::{highlight, view_text},
    },
};
use anyhow::Context;
use mime::Mime;
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
};
use slumber_config::Action;
use slumber_template::Template;
use std::{borrow::Cow, error::Error, fmt::Debug, fs};
use tracing::debug;

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
pub struct EditableTemplate<PK, T: Preview = Template> {
    id: ComponentId,
    /// Emitter for the callback from editing the template externally
    override_emitter: Emitter<SaveTemplateOverride>,
    /// Emitter for menu actions
    actions_emitter: Emitter<EditableTemplateMenuAction>,

    // Inner state
    /// The template from the collection
    original_template: T,
    /// Temporary override entered by the user
    ///
    /// Single-line templates are edited inline, while multi-line templates are
    /// opened in the external editor. Because of this, it's possible for the
    /// input to be invalid. In that case, we'll store the invalid source and
    /// the error and show them. We'll use the original template for request
    /// building while the override is invalid.
    override_result: Option<Result<T, (String, T::Err)>>,
    /// Session store key to persist the override template
    persistent_key: PK,
    /// Container for both the original and override templates
    preview: TemplatePreview<T>,
    /// Rendered preview text
    text_window: TextWindow,
    /// An inline text box for editing the override template. `Some` only when
    /// editing.
    edit_text_box: Option<TextBox>,

    // Component customization settings
    /// Descriptor for the *type* of template being shown, e.g. "Header"
    noun: &'static str,
    /// Content MIME type, used for syntax highlighting and pager selection of
    /// request bodies. This has no impact on content of the rendered template.
    mime: Option<Mime>,
    /// Is streaming possible for the output destination? If `false`, streams
    /// will be eagerly evaluated
    can_stream: bool,
    /// After a new valid template is submitted, should we send
    /// [BroadcastEvent::RefreshPreviews]? Enable for profile fields, because
    /// those can affect other templates
    refresh_on_edit: bool,
    /// Generally speaking, is the content large?
    ///
    /// Use this for request bodies. When enabled:
    /// - External editor will *always* be used
    /// - Line numbers are scrollbars are shown on the text window
    window_mode: bool,
}

impl<PK, T: Preview> EditableTemplate<PK, T> {
    /// Start building an [EditableTemplate]
    ///
    /// If you don't need any customizations, just use [Self::new].
    ///
    /// ## Params
    ///
    /// - `noun`: Name of the thing this template renders to, e.g. "Header"
    /// - `persistent_key`: Key to store the override in the *session* store
    /// - `template`: Template being edited
    ///
    /// ```notest
    /// EditableTemplate("Body", BodyKey, "{{ data }}".into())
    ///     .can_stream(true)
    ///     .build()
    /// ```
    pub fn builder(
        noun: &'static str,
        persistent_key: PK,
        template: T,
    ) -> EditableTemplateBuilder<PK, T> {
        EditableTemplateBuilder {
            noun,
            persistent_key,
            original_template: template,
            mime: None,
            can_stream: false,
            refresh_on_edit: false,
            window_mode: false,
        }
    }

    /// Convenience function for building a new [EditableTemplate] with no
    /// builder customization.
    ///
    /// Equivalent to:
    ///
    /// ```notest
    /// EditableTemplate::builder(noun, persistent_key, template).build()
    /// ```
    pub fn new(noun: &'static str, persistent_key: PK, template: T) -> Self
    where
        PK: SessionKey<Value = String>,
    {
        Self::builder(noun, persistent_key, template).build()
    }

    /// Get the shown template
    ///
    /// This is the override template if present *and valid*, otherwise it's
    /// the recipe template.
    pub fn template(&self) -> &T {
        self.override_template().unwrap_or(&self.original_template)
    }

    /// Get the current *valid* override
    ///
    /// If there is no override, or it's not a valid template, return `None`.
    pub fn override_template(&self) -> Option<&T> {
        match &self.override_result {
            Some(Ok(template)) => Some(template),
            Some(Err(_)) | None => None,
        }
    }

    /// Get the source the user inputted for the current override
    ///
    /// This is what we'll persist and what we'll show when they re-open
    /// the editor.
    fn override_source(&self) -> Option<Cow<'_, str>> {
        self.override_result.as_ref().map(|result| match result {
            Ok(template) => template.display(),
            Err((source, _)) => source.into(),
        })
    }

    /// Get visible preview text
    pub fn text(&'_ self) -> &'_ Text<'_> {
        self.text_window.text()
    }

    /// Parse the source string as `T` and set the override result
    fn set_override(&mut self, source: String) {
        match source.parse::<T>() {
            Ok(template) if template != self.original_template => {
                // Show raw text until the preview loads
                let (preview, text) = TemplatePreview::new(
                    template.clone(),
                    self.can_stream,
                    true,
                );
                self.preview = preview;
                self.set_text(text);
                self.override_result = Some(Ok(template));
            }
            // Override is equal to the original - delete the override
            Ok(_) => self.reset_override(),
            Err(error) => {
                // We'll draw the source text. Since the error is also stored,
                // we'll show that outside the text window
                //
                // Since there's no valid template here, we're not touching
                // the template preview at all. It won't emit any events until
                // the next time we have a valid template.
                self.set_text(source.clone().into());
                // We have to store the input text separately from the display
                // text, so we can retrieve it when persisting and re-opening
                // the editor
                self.override_result = Some(Err((source, error)));
            }
        }
    }

    /// Reset the template override to the default from the recipe, and
    /// recompute the template preview
    fn reset_override(&mut self) {
        self.override_result = None;
        let (preview, text) = TemplatePreview::new(
            self.original_template.clone(),
            self.can_stream,
            false,
        );
        self.preview = preview;
        self.set_text(text);
    }

    /// Apply syntax highlight and present the text
    fn set_text(&mut self, text: Text<'static>) {
        let syntax_type = self.mime.as_ref().and_then(|mime| {
            SyntaxType::from_mime(ViewContext::config().mime_overrides(), mime)
        });
        let text = highlight::highlight_if(syntax_type, text);

        // In window mode, there's only one template shown so we can scroll
        // up/down with the arrow keys. In inline mode, we may be inside a list
        // so leave up/down for the Select. Never eat left/right arrow keys
        // because that's used for the recipe tab header.
        self.text_window =
            TextWindow::new(text).scroll_mode(if self.window_mode {
                ScrollMode::Vertical
            } else {
                ScrollMode::None
            });
    }

    /// Open rendered preview in the pager
    fn view(&self) {
        view_text(self.text_window.text(), self.mime.clone());
    }

    /// Edit the template
    ///
    /// If it's one line, edit inline. If multiple, open externally
    fn edit(&mut self) {
        // If there's an existing override, use its source. Otherwise, start
        // with the default template
        let source = self
            .override_source()
            .unwrap_or_else(|| self.original_template.display());
        if self.window_mode || source.lines().count() > 1 {
            // Template has multiple lines so it can't be edited inline. Open it
            // externally
            let Some(file) = TempFile::new(source.as_bytes(), None)
                .reported(&ViewContext::messages_tx())
            else {
                // Write failed
                return;
            };
            debug!(?file, "Wrote body to file for editing");

            // Send a message to open the body in an external editor. We have to
            // write the body to a temp file so the editor subprocess can access
            // it. We'll read it back later.
            let emitter = self.override_emitter;
            ViewContext::push_message(Message::FileEdit {
                file,
                on_complete: Box::new(move |file| {
                    emitter.emit(SaveTemplateOverride(file));
                }),
            });
        } else {
            // Inline edit
            self.edit_text_box = Some(
                TextBox::default()
                    .default_value(source.into_owned())
                    .subscribe([TextBoxEvent::Cancel, TextBoxEvent::Submit])
                    .validator(|value| value.parse::<T>().is_ok()),
            );
        }
    }

    /// Stop inline editing and save the current template as the override
    fn submit_edit(&mut self) {
        // Should always be defined when submission is triggered
        let Some(text_box) = self.edit_text_box.take() else {
            return;
        };

        self.set_override(text_box.into_text());
        if self.refresh_on_edit {
            ViewContext::push_message(BroadcastEvent::RefreshPreviews);
        }
    }

    /// Read the user's edited body from the temp file we created, and rebuild
    /// the body from that
    fn load_override(&mut self, file: TempFile) {
        // Read the body back from the temp file we handed to the editor, then
        // delete it to prevent cluttering the disk
        debug!(?file, "Reading edited body from file");
        let Some(source) = fs::read_to_string(file.path())
            .with_context(|| {
                format!(
                    "Error reading edited template from file `{}`",
                    file.path().display()
                )
            })
            .reported(&ViewContext::messages_tx())
        else {
            // Read failed. It might be worth deleting the file here, but if the
            // read failed it seems unlikely the delete would succeed
            return;
        };

        self.set_override(source);
    }
}

impl<PK, T> Component for EditableTemplate<PK, T>
where
    PK: Clone + SessionKey<Value = String>,
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
                Action::View => self.view(),
                _ => propagate.set(),
            })
            .emitted(self.override_emitter, |SaveTemplateOverride(file)| {
                self.load_override(file);
            })
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                EditableTemplateMenuAction::View => self.view(),
                EditableTemplateMenuAction::Copy => todo!(),
                EditableTemplateMenuAction::Edit => self.edit(),
                EditableTemplateMenuAction::Reset => self.reset_override(),
            })
            // Store text rendered by the template preview
            .emitted(self.preview.to_emitter(), |TemplatePreviewEvent(text)| {
                self.set_text(text);
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
        let emitter = self.actions_emitter;
        vec![MenuItem::Group {
            name: self.noun.to_owned(),
            children: vec![
                emitter
                    .menu(EditableTemplateMenuAction::View, "View")
                    .shortcut(Some(Action::View))
                    .into(),
                emitter
                    .menu(EditableTemplateMenuAction::Copy, "Copy")
                    .into(),
                emitter
                    .menu(EditableTemplateMenuAction::Edit, "Edit")
                    .shortcut(Some(Action::Edit))
                    .into(),
                emitter
                    .menu(EditableTemplateMenuAction::Reset, "Reset")
                    .enable(self.override_result.is_some())
                    .shortcut(Some(Action::Reset))
                    .into(),
            ],
        }]
    }

    fn persist(&self, store: &mut PersistentStore) {
        if let Some(source) = self.override_source() {
            // The override could be a template OR an error. Persist the source
            // that the user entered, so we can restore in either case.
            store.set_session(self.persistent_key.clone(), source.into_owned());
        } else {
            store.remove_session(&self.persistent_key);
        }
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            self.preview.to_child(),
            self.edit_text_box.to_child(),
            self.text_window.to_child(),
        ]
    }
}

impl<PK, T> Draw for EditableTemplate<PK, T>
where
    PK: Clone + SessionKey<Value = String>,
    T: Preview,
    T::Err: Error,
{
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let text_area = match &self.override_result {
            Some(Ok(_)) | None => metadata.area(),
            Some(Err((_, error))) => {
                // We have an override but it's invalid - show the source+error
                let [text_area, _, error_area] = Layout::vertical([
                    Constraint::Length(self.text_window.text().height() as u16),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(metadata.area());

                // Draw the error below the text
                let styles = ViewContext::styles();
                let error_text =
                    (error as &dyn Error).generate().style(styles.text.error);
                canvas.render_widget(error_text, error_area);
                text_area
            }
        };

        if let Some(edit_text_box) = &self.edit_text_box {
            canvas.draw(
                edit_text_box,
                TextBoxProps {
                    // This template is generally shown in a table, where the
                    // scrollbar can cover up other rows
                    scrollbar: false,
                    ..TextBoxProps::default()
                },
                text_area,
                true,
            );
        } else {
            canvas.draw(
                &self.text_window,
                TextWindowProps {
                    // Hide gutter/scrollbar in inline mode
                    gutter: self.window_mode,
                    scrollbar: self.window_mode,
                    ..Default::default()
                },
                text_area,
                true,
            );
        }
    }
}

/// Builder for [EditableTemplate]
pub struct EditableTemplateBuilder<PK, T> {
    // See EditableTemplate for a description of each field
    noun: &'static str,
    persistent_key: PK,
    original_template: T,
    mime: Option<Mime>,
    can_stream: bool,
    refresh_on_edit: bool,
    window_mode: bool,
}

impl<PK, T> EditableTemplateBuilder<PK, T> {
    /// Enable/disable streaming output for the template
    pub fn can_stream(mut self, can_stream: bool) -> Self {
        self.can_stream = can_stream;
        self
    }

    /// Set the content MIME type of the template
    pub fn mime(mut self, mime: Option<Mime>) -> Self {
        self.mime = mime;
        self
    }

    /// Should all template previews be refreshed when this one is edited?
    ///
    /// Enable for profile fields, because they can have downstream effects.
    pub fn refresh_on_edit(mut self, refresh_on_edit: bool) -> Self {
        self.refresh_on_edit = refresh_on_edit;
        self
    }

    /// Enable window mode, where the template is given more breathing room
    ///
    /// Use this for request bodies, because they could be L A R G E.
    pub fn window_mode(mut self, window_mode: bool) -> Self {
        self.window_mode = window_mode;
        self
    }

    /// Build the [EditableTemplate] and trigger preview rendering
    pub fn build(self) -> EditableTemplate<PK, T>
    where
        PK: SessionKey<Value = String>,
        T: Preview,
    {
        let override_source =
            PersistentStore::get_session(&self.persistent_key);
        let override_result = override_source.map(|source| {
            // If it fails, store the error *and* the original text
            source.parse::<T>().map_err(|error| (source, error))
        });

        // Start rendering the preview in the background
        let (preview, initial_text) = TemplatePreview::new(
            // Use the override if present and valid, otherwise default
            override_result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .unwrap_or(&self.original_template)
                .clone(),
            self.can_stream,
            override_result.is_some(),
        );

        let mut component = EditableTemplate {
            id: ComponentId::default(),
            noun: self.noun,
            override_emitter: Emitter::default(),
            actions_emitter: Emitter::default(),
            original_template: self.original_template,
            override_result,
            persistent_key: self.persistent_key,
            preview,
            mime: self.mime,
            text_window: TextWindow::default(),
            edit_text_box: None,
            can_stream: self.can_stream,
            refresh_on_edit: self.refresh_on_edit,
            window_mode: self.window_mode,
        };

        // Start with the raw template text, until the preview loads. Do this
        // in a separate step so we can re-use highlighting from set_text()
        component.set_text(initial_text);

        component
    }
}

/// Local event to save a user's override template. Triggered from the
/// on_complete callback when the user closes the editor.
#[derive(Debug)]
struct SaveTemplateOverride(TempFile);

/// Menu action for [EditableTemplate]
#[derive(Copy, Clone, Debug)]
enum EditableTemplateMenuAction {
    /// Open the preview text in the external pager
    View,
    /// Copy the preview text to the clipboard
    Copy,
    /// Edit the override
    Edit,
    /// Wipe out the current override
    Reset,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{
        persistent::PersistentStore,
        test_util::{TestComponent, TestHarness, harness},
    };
    use ratatui::{
        style::Styled,
        text::{Line, Span},
    };
    use rstest::rstest;
    use slumber_util::assert_matches;
    use std::iter;
    use terminput::KeyCode;

    /// Persistent key for testing
    #[derive(Clone, Debug, PartialEq)]
    struct Key;

    impl SessionKey for Key {
        type Value = String;
    }

    /// Test editing a multiline template, which should open a file for the user
    /// to edit, then load the response
    #[rstest]
    fn test_edit_external(#[with(10, 2)] mut harness: TestHarness) {
        let mut component = TestComponent::new(
            &mut harness,
            EditableTemplate::new("Stuff", Key, "line 1\nline 2".into()),
        );

        // Check initial state
        assert_eq!(component.override_template(), None);
        harness.assert_buffer_lines(["line 1    ", "line 2    "]);

        // Edit the template
        edit(&mut component, &mut harness, "line 1\nline 2", "goodbye!");

        assert_eq!(component.override_template(), Some(&"goodbye!".into()));
        harness.assert_buffer_lines([
            Line::from_iter([edited("goodbye!"), "  ".into()]),
            "".into(),
        ]);

        // Persistence store should be updated
        let persisted = PersistentStore::get_session(&Key);
        assert_eq!(persisted, Some("goodbye!".into()));

        // Reset edited state
        component
            .int(&mut harness)
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
        assert_eq!(component.override_template(), None);
    }
    /// Test persisting and restoring overrides
    #[rstest]
    fn test_persistence(mut harness: TestHarness) {
        harness.set_session(Key, "persisted".into());
        let mut component = TestComponent::new(
            &mut harness,
            EditableTemplate::<_, Template>::new("Item", Key, "default".into()),
        );

        // Persisted value is loaded on creation
        assert_eq!(component.override_template(), Some(&"persisted".into()));

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
        assert_eq!(component.override_template(), Some(&"override".into()));
        assert_eq!(PersistentStore::get_session(&Key), Some("override".into()));

        // Clear the override; should be removed from the store
        component
            .int(&mut harness)
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
        assert_eq!(component.override_template(), None);
        assert_eq!(PersistentStore::get_session(&Key), None);
    }

    /// Test edit and provide an invalid template. It should show the template
    /// with the error
    #[rstest]
    fn test_edit_invalid(#[with(20, 5)] mut harness: TestHarness) {
        let mut component = TestComponent::new(
            &mut harness,
            EditableTemplate::<_, Template>::new(
                "Item",
                Key,
                "line 1\nline 2".into(),
            ),
        );

        // Open the editor
        edit(&mut component, &mut harness, "line 1\nline 2", "{{");

        // We don't have a valid override, so we'll let the HTTP engine use the
        // original template
        assert_eq!(component.override_template(), None);
        harness.assert_buffer_lines([
            vec!["{{".into()],
            vec![],
            vec![error("{{                  ")],
            vec![error("  ^                 ")],
            vec![error("invalid expression  ")],
        ]);

        // Invalid template is persisted
        let persisted = PersistentStore::get_session(&Key);
        assert_eq!(persisted.as_deref(), Some("{{"));
    }

    /// Style text to match the edited/overridden style
    fn edited(text: &str) -> Span<'_> {
        let styles = ViewContext::styles();
        Span::from(text).set_style(styles.text.edited)
    }

    /// Style text as an error
    fn error(text: &str) -> Span<'_> {
        let style = ViewContext::styles();
        Span::from(text).set_style(style.text.error)
    }

    /// Simulate template editing in a raw/JSON body. This will send an event
    /// to open the editor, assert the opened file has the expected initial
    /// content, write the new content (overwriting old content), then close the
    /// file and allow the component to update with the new template.
    fn edit(
        component: &mut TestComponent<EditableTemplate<Key, Template>>,
        harness: &mut TestHarness,
        expected_initial_content: &str,
        content: &str,
    ) {
        harness.messages_rx().clear();
        let (file, on_complete) = assert_matches!(
            component
                .int(harness)
                .send_key(KeyCode::Char('e'))
                .into_propagated(),
            [Message::FileEdit {
                file,
                on_complete,
            }] => (file, on_complete),
        );
        // Make sure the initial content is present as expected
        assert_eq!(
            fs::read_to_string(file.path()).unwrap(),
            expected_initial_content
        );

        // Simulate the editor modifying the file
        fs::write(file.path(), content).unwrap();
        on_complete(file);
        // Handle completion event
        component.int(harness).drain_draw().assert().empty();
    }
}
