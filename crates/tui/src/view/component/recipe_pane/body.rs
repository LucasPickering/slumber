use crate::{
    context::TuiContext,
    message::Message,
    util::{delete_temp_file, temp_file, ResultReported},
    view::{
        common::text_window::{ScrollbarMargins, TextWindow, TextWindowProps},
        component::recipe_pane::{
            persistence::{RecipeOverrideKey, RecipeTemplate},
            table::{RecipeFieldTable, RecipeFieldTableProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::Identified,
        Component, ViewContext,
    },
};
use anyhow::Context;
use ratatui::{style::Styled, text::Text, Frame};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{RecipeBody, RecipeId},
    http::content_type::ContentType,
    template::Template,
};
use std::{
    fs,
    ops::Deref,
    path::{Path, PathBuf},
};
use tracing::debug;

/// Render recipe body. The variant is based on the incoming body type, and
/// determines the representation
#[derive(Debug)]
pub enum RecipeBodyDisplay {
    Raw(Component<RawBody>),
    Form(Component<RecipeFieldTable<FormRowKey, FormRowToggleKey>>),
}

impl RecipeBodyDisplay {
    /// Build a component to display the body, based on the body type
    pub fn new(body: &RecipeBody, recipe_id: RecipeId) -> Self {
        match body {
            RecipeBody::Raw { body, content_type } => Self::Raw(
                RawBody::new(recipe_id, body.clone(), *content_type).into(),
            ),
            RecipeBody::FormUrlencoded(fields)
            | RecipeBody::FormMultipart(fields) => {
                let inner = RecipeFieldTable::new(
                    FormRowKey(recipe_id.clone()),
                    fields.iter().enumerate().map(|(i, (field, value))| {
                        (
                            field.clone(),
                            value.clone(),
                            RecipeOverrideKey::form_field(recipe_id.clone(), i),
                            FormRowToggleKey {
                                recipe_id: recipe_id.clone(),
                                field: field.clone(),
                            },
                        )
                    }),
                );
                Self::Form(inner.into())
            }
        }
    }

    /// Get body text. Return `None` for form bodies
    pub fn text(
        &self,
    ) -> Option<impl '_ + Deref<Target = Identified<Text<'static>>>> {
        match self {
            RecipeBodyDisplay::Raw(body) => {
                Some(body.data().body.preview().text())
            }
            RecipeBodyDisplay::Form(_) => None,
        }
    }

    /// If the user has applied a temporary edit to the body, get the override
    /// value. Return `None` to use the recipe's stock body.
    pub fn override_value(&self) -> Option<RecipeBody> {
        match self {
            RecipeBodyDisplay::Raw(inner)
                if inner.data().body.is_overridden() =>
            {
                let inner = inner.data();
                Some(RecipeBody::Raw {
                    body: inner.body.template().clone(),
                    content_type: inner.body.content_type(),
                })
            }
            _ => None,
        }
    }
}

impl EventHandler for RecipeBodyDisplay {
    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        match self {
            Self::Raw(inner) => vec![inner.to_child_mut()],
            Self::Form(form) => vec![form.to_child_mut()],
        }
    }
}

impl Draw for RecipeBodyDisplay {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        match self {
            RecipeBodyDisplay::Raw(inner) => {
                inner.draw(frame, (), metadata.area(), true)
            }
            RecipeBodyDisplay::Form(form) => form.draw(
                frame,
                RecipeFieldTableProps {
                    key_header: "Field",
                    value_header: "Value",
                },
                metadata.area(),
                true,
            ),
        }
    }
}

#[derive(Debug)]
pub struct RawBody {
    emitter: Emitter<SaveBodyOverride>,
    body: RecipeTemplate,
    text_window: Component<TextWindow>,
}

impl RawBody {
    fn new(
        recipe_id: RecipeId,
        template: Template,
        content_type: Option<ContentType>,
    ) -> Self {
        Self {
            emitter: Default::default(),
            body: RecipeTemplate::new(
                RecipeOverrideKey::body(recipe_id),
                template,
                content_type,
            ),
            text_window: Component::default(),
        }
    }

    /// Send a message to open the body in an external editor. We have to write
    /// the body to a temp file so the editor subprocess can access it. We'll
    /// read it back later.
    fn open_editor(&mut self) {
        let path = temp_file();
        debug!(?path, "Writing body to file for editing");
        let Some(_) =
            fs::write(&path, self.body.template().display().as_bytes())
                .with_context(|| {
                    format!("Error writing body to file {path:?} for editing")
                })
                .reported(&ViewContext::messages_tx())
        else {
            // Write failed
            return;
        };

        let emitter = self.emitter;
        ViewContext::send_message(Message::FileEdit {
            path,
            on_complete: Box::new(move |path| {
                emitter.emit(SaveBodyOverride(path))
            }),
        })
    }

    /// Read the user's edited body from the temp file we created, and rebuild
    /// the body from that
    fn load_override(&mut self, path: &Path) {
        // Read the body back from the temp file we handed to the editor, then
        // delete it to prevent cluttering the disk
        debug!(?path, "Reading edited body from file");
        let Some(body) = fs::read_to_string(path)
            .with_context(|| {
                format!("Error reading edited body from file {path:?}")
            })
            .reported(&ViewContext::messages_tx())
        else {
            // Read failed. It might be worth deleting the file here, but if the
            // read failed it's very unlikely the delete would succeed
            return;
        };

        // Clean up after ourselves
        delete_temp_file(path);

        let Some(template) = body
            .parse::<Template>()
            .reported(&ViewContext::messages_tx())
        else {
            // Whatever the user wrote isn't a valid template
            return;
        };

        // Update state and regenerate the preview
        self.body.set_override(template);
    }
}

impl EventHandler for RawBody {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::Edit => self.open_editor(),
                Action::Reset => self.body.reset_override(),
                _ => propagate.set(),
            })
            .emitted(self.emitter, |SaveBodyOverride(path)| {
                self.load_override(&path)
            })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.text_window.to_child_mut()]
    }
}

impl Draw for RawBody {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let styles = &TuiContext::get().styles;
        let area = metadata.area();
        self.text_window.draw(
            frame,
            TextWindowProps {
                // Do *not* call generate, because that clones the text and
                // we only need a reference
                text: &self.body.preview().text(),
                margins: ScrollbarMargins {
                    right: 1,
                    bottom: 2, // Extra buffer for edit/reset note
                },
                footer: if self.body.is_overridden() {
                    Some("(edited)".set_style(styles.text.hint).into())
                } else {
                    None
                },
            },
            area,
            true,
        );
    }
}

/// Emit events to ourselves for override editing
impl ToEmitter<SaveBodyOverride> for RawBody {
    fn to_emitter(&self) -> Emitter<SaveBodyOverride> {
        self.emitter
    }
}

/// Persistence key for selected form field, per recipe. Value is the field name
#[derive(Debug, Serialize, persisted::PersistedKey)]
#[persisted(Option<String>)]
pub struct FormRowKey(RecipeId);

/// Persistence key for toggle state for a single form field in the table
#[derive(Debug, Serialize, persisted::PersistedKey)]
#[persisted(bool)]
pub struct FormRowToggleKey {
    recipe_id: RecipeId,
    field: String,
}

/// Local event to save a user's override body. Triggered from the on_complete
/// callback when the user closes the editor.
#[derive(Debug)]
pub struct SaveBodyOverride(PathBuf);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::{
            component::recipe_pane::persistence::{
                RecipeOverrideStore, RecipeOverrideValue,
            },
            test_util::TestComponent,
        },
    };
    use crossterm::event::KeyCode;
    use persisted::PersistedStore;
    use ratatui::text::Span;
    use rstest::rstest;
    use slumber_core::{assert_matches, test_util::Factory};

    /// Test editing the body, which should open a file for the user to edit,
    /// then load the response
    #[rstest]
    fn test_edit(
        mut harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let body: RecipeBody = RecipeBody::Raw {
            body: "hello!".into(),
            content_type: Some(ContentType::Json),
        };
        let recipe_id = RecipeId::factory(());
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            RecipeBodyDisplay::new(&body, recipe_id.clone()),
        );

        // Check initial state
        assert_eq!(component.data().override_value(), None);
        terminal.assert_buffer_lines([vec![gutter("1"), " hello!  ".into()]]);

        // Open the editor
        harness.clear_messages();
        component.send_key(KeyCode::Char('e')).assert_empty();
        let (path, on_complete) = assert_matches!(
            harness.pop_message_now(),
            Message::FileEdit {
                path,
                on_complete,
            } => (path, on_complete),
        );
        assert_eq!(fs::read(&path).unwrap(), b"hello!");

        // Simulate the editor modifying the file
        fs::write(&path, "goodbye!").unwrap();
        on_complete(path);
        component.drain_draw().assert_empty();

        assert_eq!(
            component.data().override_value(),
            Some(RecipeBody::Raw {
                body: "goodbye!".into(),
                content_type: Some(ContentType::Json),
            })
        );
        terminal.assert_buffer_lines([vec![gutter("1"), " goodbye!".into()]]);

        // Persistence store should be updated
        let persisted = RecipeOverrideStore::load_persisted(
            &RecipeOverrideKey::body(recipe_id),
        );
        assert_eq!(
            persisted,
            Some(RecipeOverrideValue::Override("goodbye!".into()))
        );

        // Reset edited state
        component.send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.data().override_value(), None);
    }

    /// Override template should be loaded from the persistence store on init
    #[rstest]
    fn test_persisted_override(
        harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let recipe_id = RecipeId::factory(());
        RecipeOverrideStore::store_persisted(
            &RecipeOverrideKey::body(recipe_id.clone()),
            &RecipeOverrideValue::Override("hello!".into()),
        );

        let body: RecipeBody = RecipeBody::Raw {
            body: "".into(),
            content_type: Some(ContentType::Json),
        };
        let component = TestComponent::new(
            &harness,
            &terminal,
            RecipeBodyDisplay::new(&body, recipe_id),
        );

        assert_eq!(
            component.data().override_value(),
            Some(RecipeBody::Raw {
                body: "hello!".into(),
                content_type: Some(ContentType::Json),
            })
        );
        terminal.assert_buffer_lines([vec![gutter("1"), " hello!  ".into()]]);
    }

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span {
        let styles = &TuiContext::get().styles;
        Span::styled(text, styles.text_window.gutter)
    }
}
