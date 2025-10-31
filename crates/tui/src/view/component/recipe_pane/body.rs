use crate::{
    message::Message,
    util::{ResultReported, TempFile},
    view::{
        Component, ViewContext,
        common::{
            actions::MenuItem,
            text_window::{ScrollbarMargins, TextWindow, TextWindowProps},
        },
        component::{
            Canvas, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            recipe_pane::{
                persistence::{RecipeOverrideKey, RecipeTemplate},
                table::{RecipeFieldTable, RecipeFieldTableProps},
            },
        },
        context::UpdateContext,
        event::{Emitter, Event, OptionEvent},
        util::view_text,
    },
};
use anyhow::Context;
use indexmap::IndexMap;
use mime::Mime;
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{JsonTemplate, Recipe, RecipeBody, RecipeId},
    http::content_type::ContentType,
};
use slumber_template::Template;
use std::fs;
use tracing::debug;

/// Render recipe body. The variant is based on the incoming body type, and
/// determines the representation
#[derive(Debug)]
pub enum RecipeBodyDisplay {
    /// A raw text body with no known content type
    Raw(TextBody),
    /// A body declared with the `json` type. This is presented as text so it
    /// uses the same internal type as `Raw`, but the distinction allows us to
    /// parse and generate an override body correctly
    Json(TextBody),
    Form(RecipeFieldTable<FormRowKey, FormRowToggleKey>),
}

impl RecipeBodyDisplay {
    /// Build a component to display the body, based on the body type. This
    /// takes in the full recipe as well as the body so we can guarantee the
    /// body is not `None`.
    pub fn new(body: &RecipeBody, recipe: &Recipe) -> Self {
        match body {
            RecipeBody::Raw(body) | RecipeBody::Stream(body) => {
                Self::Raw(TextBody::new(body.clone(), recipe))
            }
            RecipeBody::Json(json) => {
                // Stringify all the individual templates in the JSON, pretty
                // print that as JSON, then parse it back as one big template.
                // This is clumsy but it's the easiest way to represent the body
                // as a single template, and shouldn't be too expensive
                let json_string =
                    format!("{:#}", serde_json::Value::from(json));
                // This unwrap is safe because we know the body was originally
                // parsed from a single string so all the individual strings
                // are valid templates. JSON syntax can't create an invalid
                // template string anywhere because the braces all get
                // whitespace between them.
                let template = json_string.parse().unwrap();
                Self::Json(TextBody::new(template, recipe))
            }
            RecipeBody::FormUrlencoded(fields) => {
                Self::Form(Self::form_table(&recipe.id, fields, false))
            }
            RecipeBody::FormMultipart(fields) => {
                Self::Form(Self::form_table(&recipe.id, fields, true))
            }
        }
    }

    fn form_table(
        recipe_id: &RecipeId,
        fields: &IndexMap<String, Template>,
        can_stream: bool,
    ) -> RecipeFieldTable<FormRowKey, FormRowToggleKey> {
        RecipeFieldTable::new(
            "Field",
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
            can_stream,
        )
    }

    /// If the user has applied a temporary edit to the body, get the override
    /// value. Return `None` to use the recipe's stock body.
    pub fn override_value(&self) -> Option<RecipeBody> {
        match self {
            RecipeBodyDisplay::Raw(inner) if inner.body.is_overridden() => {
                Some(RecipeBody::Raw(inner.body.template().clone()))
            }
            RecipeBodyDisplay::Json(inner) if inner.body.is_overridden() => {
                // Parse the template as JSON. The inner templates within the
                // JSON strings will be parsed into individual templates
                let json: JsonTemplate = inner
                    .body
                    .template()
                    .display()
                    .parse()
                    .reported(&ViewContext::messages_tx())?;
                Some(RecipeBody::Json(json))
            }
            // Form bodies override per-field so return None for them
            _ => None,
        }
    }
}

impl Component for RecipeBodyDisplay {
    fn id(&self) -> ComponentId {
        match self {
            RecipeBodyDisplay::Raw(text_body)
            | RecipeBodyDisplay::Json(text_body) => text_body.id(),
            RecipeBodyDisplay::Form(table) => table.id(),
        }
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match self {
            Self::Raw(inner) => vec![inner.to_child_mut()],
            Self::Json(inner) => vec![inner.to_child_mut()],
            Self::Form(form) => vec![form.to_child_mut()],
        }
    }
}

impl Draw for RecipeBodyDisplay {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        match self {
            RecipeBodyDisplay::Raw(inner) => {
                canvas.draw(inner, (), metadata.area(), true);
            }
            RecipeBodyDisplay::Json(inner) => {
                canvas.draw(inner, (), metadata.area(), true);
            }
            RecipeBodyDisplay::Form(form) => canvas.draw(
                form,
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

/// A body represented and editable as a single block of text
#[derive(Debug)]
pub struct TextBody {
    id: ComponentId,
    /// Emitter for the callback from editing the body
    override_emitter: Emitter<SaveBodyOverride>,
    /// Emitter for menu actions
    actions_emitter: Emitter<RawBodyMenuAction>,
    body: RecipeTemplate,
    /// Body MIME type, used for syntax highlighting and pager selection. This
    /// has no impact on content of the rendered body
    mime: Option<Mime>,
    text_window: TextWindow,
}

impl TextBody {
    fn new(template: Template, recipe: &Recipe) -> Self {
        let mime = recipe.mime();
        let content_type = mime.as_ref().and_then(ContentType::from_mime);
        Self {
            id: ComponentId::default(),
            override_emitter: Default::default(),
            actions_emitter: Default::default(),
            body: RecipeTemplate::new(
                RecipeOverrideKey::body(recipe.id.clone()),
                template,
                content_type,
                true,
            ),
            mime,
            text_window: TextWindow::default(),
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
        let Some(file) =
            TempFile::new(self.body.template().display().as_bytes())
                .reported(&ViewContext::messages_tx())
        else {
            // Write failed
            return;
        };
        debug!(?file, "Wrote body to file for editing");

        let emitter = self.override_emitter;
        ViewContext::send_message(Message::FileEdit {
            file,
            on_complete: Box::new(move |file| {
                emitter.emit(SaveBodyOverride(file));
            }),
        });
    }

    /// Read the user's edited body from the temp file we created, and rebuild
    /// the body from that
    fn load_override(&mut self, file: TempFile) {
        // Read the body back from the temp file we handed to the editor, then
        // delete it to prevent cluttering the disk
        debug!(?file, "Reading edited body from file");
        let Some(body) = fs::read_to_string(file.path())
            .with_context(|| {
                format!(
                    "Error reading edited body from file `{}`",
                    file.path().display()
                )
            })
            .reported(&ViewContext::messages_tx())
        else {
            // Read failed. It might be worth deleting the file here, but if the
            // read failed it's very unlikely the delete would succeed
            return;
        };

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

impl Component for TextBody {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::View => self.view_body(),
                Action::Edit => self.open_editor(),
                Action::Reset => self.body.reset_override(),
                _ => propagate.set(),
            })
            .emitted(self.override_emitter, |SaveBodyOverride(file)| {
                self.load_override(file);
            })
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                RawBodyMenuAction::View => self.view_body(),
                RawBodyMenuAction::Copy => {
                    ViewContext::send_message(Message::CopyRequestBody);
                }
                RawBodyMenuAction::Edit => self.open_editor(),
                RawBodyMenuAction::Reset => self.body.reset_override(),
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        let emitter = self.actions_emitter;
        vec![
            emitter
                .menu(RawBodyMenuAction::View, "View Body")
                .shortcut(Some(Action::View))
                .into(),
            emitter.menu(RawBodyMenuAction::Copy, "Copy Body").into(),
            emitter
                .menu(RawBodyMenuAction::Edit, "Edit Body")
                .shortcut(Some(Action::Edit))
                .into(),
            emitter
                .menu(RawBodyMenuAction::Reset, "Reset Body")
                .enable(self.body.is_overridden())
                .shortcut(Some(Action::Reset))
                .into(),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.text_window.to_child_mut()]
    }
}

impl Draw for TextBody {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let area = metadata.area();
        canvas.draw(
            &self.text_window,
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
#[derive(Copy, Clone, Debug)]
enum RawBodyMenuAction {
    View,
    Copy,
    Edit,
    Reset,
}

/// Local event to save a user's override body. Triggered from the on_complete
/// callback when the user closes the editor.
#[derive(Debug)]
struct SaveBodyOverride(TempFile);

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
    use persisted::PersistedStore;
    use ratatui::{
        style::{Color, Styled},
        text::Span,
    };
    use rstest::rstest;
    use serde_json::json;
    use slumber_util::{Factory, assert_matches};
    use terminput::KeyCode;

    /// Test editing a raw body, which should open a file for the user to edit,
    /// then load the response
    #[rstest]
    fn test_edit(
        mut harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let recipe = Recipe {
            body: Some(RecipeBody::Raw("hello!".into())),
            ..Recipe::factory(())
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        // Check initial state
        assert_eq!(component.override_value(), None);
        terminal.assert_buffer_lines([vec![gutter("1"), " hello!  ".into()]]);

        // Open the editor
        harness.clear_messages();
        component.int().send_key(KeyCode::Char('e')).assert_empty();
        let (file, on_complete) = assert_matches!(
            harness.pop_message_now(),
            Message::FileEdit {
                file,
                on_complete,
            } => (file, on_complete),
        );
        assert_eq!(fs::read_to_string(file.path()).unwrap(), "hello!");

        // Simulate the editor modifying the file
        fs::write(file.path(), "goodbye!").unwrap();
        on_complete(file);
        component.int().drain_draw().assert_empty();

        assert_eq!(
            component.override_value(),
            Some(RecipeBody::Raw("goodbye!".into()))
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
        assert_eq!(component.override_value(), None);
    }

    /// Test editing a JSON body, which should open a file for the user to edit,
    /// then load the response
    #[rstest]
    fn test_edit_json(
        mut harness: TestHarness,
        #[with(12, 1)] terminal: TestTerminal,
    ) {
        let initial_text = r#""hello!""#;
        let override_text = r#""goodbye!""#;

        let recipe = Recipe {
            body: Some(RecipeBody::json(json!("hello!")).unwrap()),
            ..Recipe::factory(())
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        // Check initial state
        assert_eq!(component.override_value(), None);
        terminal.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            // Apply syntax highlighting
            Span::from(initial_text).patch_style(Color::LightGreen),
            "  ".into(),
        ]]);

        // Open the editor
        harness.clear_messages();
        component.int().send_key(KeyCode::Char('e')).assert_empty();
        let (file, on_complete) = assert_matches!(
            harness.pop_message_now(),
            Message::FileEdit {
                file,
                on_complete,
            } => (file, on_complete),
        );
        assert_eq!(fs::read_to_string(file.path()).unwrap(), initial_text);

        // Simulate the editor modifying the file
        fs::write(file.path(), override_text).unwrap();
        on_complete(file);
        component.int().drain_draw().assert_empty();

        assert_eq!(
            component.override_value(),
            Some(RecipeBody::json(json!("goodbye!")).unwrap())
        );
        terminal.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            // Apply syntax highlighting
            edited(override_text).patch_style(Color::LightGreen),
        ]]);

        // Persistence store should be updated
        let persisted = RecipeOverrideStore::load_persisted(
            &RecipeOverrideKey::body(recipe.id.clone()),
        );
        assert_eq!(
            persisted,
            Some(RecipeOverrideValue::Override(override_text.into()))
        );

        // Reset edited state
        component.int().send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.override_value(), None);
    }

    /// Override template should be loaded from the persistence store on init
    #[rstest]
    fn test_persisted_override(
        harness: TestHarness,
        #[with(10, 1)] terminal: TestTerminal,
    ) {
        let recipe = Recipe {
            body: Some(RecipeBody::Raw("".into())),
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
            component.override_value(),
            Some(RecipeBody::Raw("hello!".into()))
        );
        terminal.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            edited("hello!"),
            "  ".into(),
        ]]);
    }

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span<'_> {
        let styles = &TuiContext::get().styles;
        Span::styled(text, styles.text_window.gutter)
    }

    /// Style text to match the edited/overridden style
    fn edited(text: &str) -> Span<'_> {
        let styles = &TuiContext::get().styles;
        Span::from(text).set_style(styles.text.edited)
    }
}
