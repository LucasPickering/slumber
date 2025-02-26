use crate::{
    message::Message,
    util::{ResultReported, delete_temp_file, temp_file},
    view::{
        Component, ViewContext,
        common::{
            actions::{IntoMenuAction, MenuAction},
            text_window::{ScrollbarMargins, TextWindow, TextWindowProps},
        },
        component::recipe_pane::{
            persistence::{RecipeOverrideKey, RecipeTemplate},
            table::{RecipeFieldTable, RecipeFieldTableProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata},
        event::{Child, Emitter, Event, EventHandler, OptionEvent},
        util::view_text,
    },
};
use anyhow::Context;
use mime::Mime;
use ratatui::Frame;
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{Recipe, RecipeBody, RecipeId},
    http::content_type::ContentType,
    template::Template,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
use strum::{EnumIter, IntoEnumIterator};
use tracing::debug;

/// Render recipe body. The variant is based on the incoming body type, and
/// determines the representation
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum RecipeBodyDisplay {
    Raw(Component<RawBody>),
    Form(Component<RecipeFieldTable<FormRowKey, FormRowToggleKey>>),
}

impl RecipeBodyDisplay {
    /// Build a component to display the body, based on the body type. This
    /// takes in the full recipe as well as the body so we can guarantee the
    /// body is not `None`.
    pub fn new(body: &RecipeBody, recipe: &Recipe) -> Self {
        match body {
            RecipeBody::Raw { body, .. } => {
                Self::Raw(RawBody::new(body.clone(), recipe).into())
            }
            RecipeBody::FormUrlencoded(fields)
            | RecipeBody::FormMultipart(fields) => {
                let inner = RecipeFieldTable::new(
                    "Field",
                    FormRowKey(recipe.id.clone()),
                    fields.iter().enumerate().map(|(i, (field, value))| {
                        (
                            field.clone(),
                            value.clone(),
                            RecipeOverrideKey::form_field(recipe.id.clone(), i),
                            FormRowToggleKey {
                                recipe_id: recipe.id.clone(),
                                field: field.clone(),
                            },
                        )
                    }),
                );
                Self::Form(inner.into())
            }
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
    /// Emitter for the callback from editing the body
    override_emitter: Emitter<SaveBodyOverride>,
    /// Emitter for menu actions
    actions_emitter: Emitter<RawBodyMenuAction>,
    body: RecipeTemplate,
    mime: Option<Mime>,
    text_window: Component<TextWindow>,
}

impl RawBody {
    fn new(template: Template, recipe: &Recipe) -> Self {
        let mime = recipe.mime();
        let content_type = mime.as_ref().and_then(ContentType::from_mime);
        Self {
            override_emitter: Default::default(),
            actions_emitter: Default::default(),
            body: RecipeTemplate::new(
                RecipeOverrideKey::body(recipe.id.clone()),
                template,
                content_type,
            ),
            mime,
            text_window: Component::default(),
        }
    }

    /// Open rendered body in the pager
    fn view_body(&self) {
        view_text(&self.body.preview().text(), self.mime.clone());
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

        let emitter = self.override_emitter;
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
                Action::View => self.view_body(),
                Action::Edit => self.open_editor(),
                Action::Reset => self.body.reset_override(),
                _ => propagate.set(),
            })
            .emitted(self.override_emitter, |SaveBodyOverride(path)| {
                self.load_override(&path)
            })
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                RawBodyMenuAction::View => self.view_body(),
                RawBodyMenuAction::Copy => {
                    ViewContext::send_message(Message::CopyRequestBody)
                }
                RawBodyMenuAction::Edit => self.open_editor(),
                RawBodyMenuAction::Reset => self.body.reset_override(),
            })
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        RawBodyMenuAction::iter()
            .map(MenuAction::with_data(self, self.actions_emitter))
            .collect()
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.text_window.to_child_mut()]
    }
}

impl Draw for RawBody {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let area = metadata.area();
        self.text_window.draw(
            frame,
            TextWindowProps {
                // Do *not* call generate, because that clones the text and
                // we only need a reference
                text: &self.body.preview().text(),
                margins: ScrollbarMargins {
                    right: 1,
                    bottom: 1,
                },
            },
            area,
            true,
        );
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

/// Action menu items for a raw body
#[derive(Copy, Clone, Debug, derive_more::Display, EnumIter)]
enum RawBodyMenuAction {
    #[display("View Body")]
    View,
    #[display("Copy Body")]
    Copy,
    #[display("Edit Body")]
    Edit,
    #[display("Reset Body")]
    Reset,
}

impl IntoMenuAction<RawBody> for RawBodyMenuAction {
    fn enabled(&self, data: &RawBody) -> bool {
        match self {
            Self::View | Self::Copy | Self::Edit => true,
            Self::Reset => data.body.is_overridden(),
        }
    }

    fn shortcut(&self, _: &RawBody) -> Option<Action> {
        match self {
            Self::View => Some(Action::View),
            Self::Copy => None,
            Self::Edit => Some(Action::Edit),
            Self::Reset => Some(Action::Reset),
        }
    }
}

/// Local event to save a user's override body. Triggered from the on_complete
/// callback when the user closes the editor.
#[derive(Debug)]
struct SaveBodyOverride(PathBuf);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::TuiContext,
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::{
            component::recipe_pane::persistence::{
                RecipeOverrideStore, RecipeOverrideValue,
            },
            test_util::TestComponent,
        },
    };
    use crossterm::event::KeyCode;
    use persisted::PersistedStore;
    use ratatui::{style::Styled, text::Span};
    use rstest::rstest;
    use slumber_core::{assert_matches, test_util::Factory};

    /// Test editing the body, which should open a file for the user to edit,
    /// then load the response
    #[rstest]
    fn test_edit(
        mut harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let recipe = Recipe {
            body: Some(RecipeBody::Raw {
                body: "hello!".into(),
                content_type: Some(ContentType::Json),
            }),
            ..Recipe::factory(())
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        // Check initial state
        assert_eq!(component.data().override_value(), None);
        terminal.assert_buffer_lines([vec![gutter("1"), " hello!  ".into()]]);

        // Open the editor
        harness.clear_messages();
        component.int().send_key(KeyCode::Char('e')).assert_empty();
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
        component.int().drain_draw().assert_empty();

        assert_eq!(
            component.data().override_value(),
            Some(RecipeBody::Raw {
                body: "goodbye!".into(),
                content_type: Some(ContentType::Json),
            })
        );
        terminal.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            edited("goodbye!"),
        ]]);

        // Persistence store should be updated
        let persisted = RecipeOverrideStore::load_persisted(
            &RecipeOverrideKey::body(recipe.id.clone()),
        );
        assert_eq!(
            persisted,
            Some(RecipeOverrideValue::Override("goodbye!".into()))
        );

        // Reset edited state
        component.int().send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.data().override_value(), None);
    }

    /// Override template should be loaded from the persistence store on init
    #[rstest]
    fn test_persisted_override(
        harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let recipe = Recipe {
            body: Some(RecipeBody::Raw {
                body: "".into(),
                content_type: Some(ContentType::Json),
            }),
            ..Recipe::factory(())
        };
        RecipeOverrideStore::store_persisted(
            &RecipeOverrideKey::body(recipe.id.clone()),
            &RecipeOverrideValue::Override("hello!".into()),
        );

        let component = TestComponent::new(
            &harness,
            &terminal,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        assert_eq!(
            component.data().override_value(),
            Some(RecipeBody::Raw {
                body: "hello!".into(),
                content_type: Some(ContentType::Json),
            })
        );
        terminal.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            edited("hello!"),
            "  ".into(),
        ]]);
    }

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span {
        let styles = &TuiContext::get().styles;
        Span::styled(text, styles.text_window.gutter)
    }

    /// Style text to match the edited/overridden style
    fn edited(text: &str) -> Span {
        let styles = &TuiContext::get().styles;
        Span::from(text).set_style(styles.text.edited)
    }
}
