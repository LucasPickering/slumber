use crate::{
    message::{Message, RecipeCopyTarget},
    util::{ResultReported, TempFile},
    view::{
        Component, Generate,
        common::{
            actions::MenuItem,
            template_preview::{TemplatePreview, TemplatePreviewEvent},
            text_window::{TextWindow, TextWindowProps},
        },
        component::{
            Canvas, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            recipe::table::{RecipeTable, RecipeTableKind, RecipeTableProps},
        },
        context::{UpdateContext, ViewContext},
        event::{Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentKey, SessionKey},
        util::{highlight, view_text},
    },
};
use anyhow::Context;
use indexmap::IndexMap;
use mime::Mime;
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{JsonTemplate, Recipe, RecipeBody, RecipeId},
    http::content_type::ContentType,
};
use slumber_template::{Template, TemplateParseError};
use std::{error::Error as StdError, fs};
use tracing::{debug, error};

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
    Form(RecipeTable<FormTableKind>),
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
                let template = preview_json_template(json);
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
    ) -> RecipeTable<FormTableKind> {
        RecipeTable::new(
            "Field",
            recipe_id.clone(),
            fields
                .iter()
                .map(|(field, value)| (field.clone(), value.clone())),
            can_stream,
        )
    }

    /// If the user has applied a temporary edit to the body, get the override
    /// value. Return `None` to use the recipe's stock body.
    pub fn override_value(&self) -> Option<Template> {
        match self {
            RecipeBodyDisplay::Raw(inner) | RecipeBodyDisplay::Json(inner)
                if inner.preview.is_overridden() =>
            {
                // For JSON bodies, the template will be parsed as JSON by the
                // HTTP engine
                Some(inner.preview.template().clone())
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
            Self::Raw(text_body) | Self::Json(text_body) => {
                vec![text_body.to_child_mut()]
            }
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
                RecipeTableProps {
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
    /// Container for both the original and override templates
    preview: TemplatePreview<BodyKey>,
    /// Body MIME type, used for syntax highlighting and pager selection. This
    /// has no impact on content of the rendered body
    mime: Option<Mime>,
    /// Visible text. If the current template is valid, this will be `Ok` and
    /// show a preview of the template. If it's invalid, it's `Err`. The
    /// `TextWindow` will hold the invalid template, and the error is stored to
    /// display the error message.
    text_window: Result<TextWindow, (TextWindow, TemplateParseError)>,
}

impl TextBody {
    fn new(template: Template, recipe: &Recipe) -> Self {
        let mime = recipe.mime();

        // Start rendering the preview in the background
        let preview =
            TemplatePreview::new(BodyKey(recipe.id.clone()), template, true);

        // Display the raw template while the preview renders
        let text = highlight(mime.as_ref(), preview.render_raw());
        let text_window = TextWindow::new(text);

        Self {
            id: ComponentId::default(),
            override_emitter: Default::default(),
            actions_emitter: Default::default(),
            preview,
            mime,
            text_window: Ok(text_window),
        }
    }

    /// Open rendered body in the pager
    fn view_body(&self) {
        let text_window = match &self.text_window {
            Ok(text_window) | Err((text_window, _)) => text_window,
        };
        view_text(text_window.text(), self.mime.clone());
    }

    /// Send a message to open the body in an external editor. We have to write
    /// the body to a temp file so the editor subprocess can access it. We'll
    /// read it back later.
    fn open_editor(&mut self) {
        let Some(file) =
            TempFile::new(self.preview.template().display().as_bytes())
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

        // Parse the template. If parsing fails, set the error
        match body.parse::<Template>() {
            Ok(template) => {
                self.preview.set_override(template);
                // Reset our text. The preview will immediately send an event
                // with the raw template text, then once the preview is done
                // we'll get another even with the rendered text
                self.text_window = Ok(TextWindow::default());
            }
            Err(error) => {
                // Override is invalid. We'll show the invalid text with the
                // error, but if a request is built it'll use the stock template
                self.preview.reset_override();
                let raw_text = highlight(
                    self.mime.as_ref(),
                    error.input().to_owned().into(),
                );
                self.text_window = Err((TextWindow::new(raw_text), error));
            }
        }
    }
}

impl Component for TextBody {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::View => self.view_body(),
                Action::Edit => self.open_editor(),
                Action::Reset => self.preview.reset_override(),
                _ => propagate.set(),
            })
            .emitted(self.override_emitter, |SaveBodyOverride(file)| {
                self.load_override(file);
            })
            .emitted(self.preview.to_emitter(), |TemplatePreviewEvent(text)| {
                // If the template is valid, accept its preview renders. If not,
                // the previews will not correspond to the invalid template
                // we're holding, so ignore these events
                if let Ok(text_window) = &mut self.text_window {
                    // Apply syntax highlighting
                    let text = highlight(self.mime.as_ref(), text);
                    *text_window = TextWindow::new(text);
                }
            })
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                RawBodyMenuAction::View => self.view_body(),
                RawBodyMenuAction::Copy => ViewContext::send_message(
                    Message::CopyRecipe(RecipeCopyTarget::Body),
                ),
                RawBodyMenuAction::Edit => self.open_editor(),
                RawBodyMenuAction::Reset => self.preview.reset_override(),
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
                .enable(self.preview.is_overridden())
                .shortcut(Some(Action::Reset))
                .into(),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        let text_window = match &mut self.text_window {
            Ok(text_window) | Err((text_window, _)) => text_window,
        };
        vec![self.preview.to_child_mut(), text_window.to_child_mut()]
    }
}

impl Draw for TextBody {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let area = metadata.area();
        match &self.text_window {
            Ok(text_window) => {
                // Override is missing or valid - render normally
                canvas.draw(
                    text_window,
                    TextWindowProps::default(),
                    area,
                    true,
                );
            }
            Err((text_window, error)) => {
                // We have an override but it's invalid - show the source+error

                let [text_area, _, error_area] = Layout::vertical([
                    Constraint::Length(text_window.text().height() as u16),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(area);
                canvas.draw(
                    text_window,
                    TextWindowProps::default(),
                    text_area,
                    true,
                );

                // Draw the error down below
                let styles = ViewContext::styles();
                let error_text = (error as &dyn StdError)
                    .generate()
                    .style(styles.text.error);
                canvas.render_widget(error_text, error_area);
            }
        }
    }
}

/// Persistent key for text body override template
#[derive(Clone, Debug, PartialEq)]
struct BodyKey(RecipeId);

impl SessionKey for BodyKey {
    type Value = Template;
}

/// [RecipeTableKind] for the form field table
#[derive(Debug)]
pub struct FormTableKind;

impl RecipeTableKind for FormTableKind {
    type Key = String;

    fn key_as_str(key: &Self::Key) -> &str {
        key.as_str()
    }
}

/// Persistence key for selected form field, per recipe. Value is the field name
#[derive(Debug, Serialize)]
pub struct SelectedFormRowKey(RecipeId);

impl PersistentKey for SelectedFormRowKey {
    type Value = String;
}

/// Persistence key for toggle state for a single form field in the table
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FormRowKey {
    recipe_id: RecipeId,
    field: String,
}

// Toggle persistent
impl PersistentKey for FormRowKey {
    type Value = bool;
}

// Override template persistent
impl SessionKey for FormRowKey {
    type Value = Template;
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

/// Apply syntax highlighting according to the body MIME type
fn highlight(mime: Option<&Mime>, text: Text<'static>) -> Text<'static> {
    let content_type = mime.and_then(ContentType::from_mime);
    highlight::highlight_if(content_type, text)
}

/// Convert a JSON object into a single template for preview in a TextBody
fn preview_json_template(json: &JsonTemplate) -> Template {
    // Kill this in https://github.com/LucasPickering/slumber/issues/627

    // Stringify all the individual templates in the JSON, pretty
    // print that as JSON, then parse it back as one big template.
    // This is clumsy but it's the easiest way to represent the body
    // as a single template, and shouldn't be too expensive
    let json_string = format!("{:#}", serde_json::Value::from(json));
    // This unwrap *should* be safe because we know the body was originally
    // parsed from a single string so all the individual strings are valid
    // templates. JSON syntax can't create an invalid template string anywhere
    // because the braces all get whitespace between them. To be safe though,
    // we fall back to the raw string
    json_string.parse().unwrap_or_else(|error| {
        error!(?json, %error, "Failed to parse JSON preview template");
        Template::raw(json_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::{
            context::ViewContext,
            persistent::PersistentStore,
            test_util::{TestComponent, TestHarness, harness},
        },
    };
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

        // Edit the template
        edit(&mut component, &mut harness, "hello!", "goodbye!");

        assert_eq!(component.override_value(), Some("goodbye!".into()));
        terminal.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            edited("goodbye!"),
        ]]);

        // Persistence store should be updated
        let persisted =
            PersistentStore::get_session(&BodyKey(recipe.id.clone()));
        assert_eq!(persisted, Some("goodbye!".into()));

        // Reset edited state
        component
            .int()
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
        assert_eq!(component.override_value(), None);
    }

    /// Test edit and provide an invalid template. It should show the template
    /// with the error
    #[rstest]
    fn test_edit_invalid(
        mut harness: TestHarness,
        #[with(20, 5)] terminal: TestTerminal,
    ) {
        let recipe = Recipe {
            body: Some(RecipeBody::Raw("init".into())),
            ..Recipe::factory(())
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        // Open the editor
        edit(&mut component, &mut harness, "init", "{{");

        // We don't have a valid override, so we'll let the HTTP engine use the
        // original template
        assert_eq!(component.override_value(), None);
        terminal.assert_buffer_lines([
            vec![gutter("1"), " ".into(), "{{".into()],
            vec![],
            vec![error("{{                  ")],
            vec![error("  ^                 ")],
            vec![error("invalid expression  ")],
        ]);

        // Invalid template is *not* persisted. This is a little shitty but it's
        // annoying to get it to be supported. We'd have to move the support for
        // invalid templates from here into OverrideTemplate, or duplicate a
        // bunch of persistence logic
        let persisted =
            PersistentStore::get_session(&BodyKey(recipe.id.clone()));
        assert_eq!(persisted, None);
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
        edit(&mut component, &mut harness, initial_text, override_text);

        assert_eq!(component.override_value(), Some(override_text.into()));
        terminal.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            // Apply syntax highlighting
            edited(override_text).patch_style(Color::LightGreen),
        ]]);

        // Persistence store should be updated
        let persisted =
            PersistentStore::get_session(&BodyKey(recipe.id.clone()));
        assert_eq!(persisted, Some(override_text.into()));

        // Reset edited state
        component
            .int()
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
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
        harness
            .persistent_store()
            .set_session(BodyKey(recipe.id.clone()), "hello!".into());

        let component = TestComponent::new(
            &harness,
            &terminal,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        assert_eq!(component.override_value(), Some("hello!".into()));
        terminal.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            edited("hello!"),
            "  ".into(),
        ]]);
    }

    /// Convert JSON templates into string templates for preview. This is a
    /// shortcut to make previewing JSON templates easy. It's actually broken
    /// and needs to be replaced.
    /// https://github.com/LucasPickering/slumber/issues/627
    #[rstest]
    // Make sure two objects don't look like a template expression
    #[case::object(
        json!({"a": {"b": "my name is {{ name }}!"}}).try_into().unwrap(),
        r#"{
  "a": {
    "b": "my name is {{ name }}!"
  }
}"#
    )]
    // https://github.com/LucasPickering/slumber/issues/646
    #[case::escaped_quote(
        JsonTemplate::String(r#"{{ jq('.name="Nemo"') }}"#.into()),
        // JSON stringification escapes the inner double quotes, which isn't
        // actually needed and interferes with the template parsing. This
        // causes it to fall back to treating it as a raw template. Totally a
        // bug, but not worth fixing before this is replaced.
        r#""{_{ jq('.name=\"Nemo\"') }}""#
    )]
    fn test_preview_json_template(
        #[case] json: JsonTemplate,
        #[case] expected: Template,
    ) {
        let actual = preview_json_template(&json);
        assert_eq!(actual, expected);
    }

    /// Style text to match the text window gutter
    fn gutter(text: &str) -> Span<'_> {
        let styles = ViewContext::styles();
        Span::styled(text, styles.text_window.gutter)
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
        component: &mut TestComponent<RecipeBodyDisplay>,
        harness: &mut TestHarness,
        initial_content: &str,
        content: &str,
    ) {
        harness.messages().clear();
        component
            .int()
            .send_key(KeyCode::Char('e'))
            .assert()
            .empty();
        let (file, on_complete) = assert_matches!(
            harness.messages().pop_now(),
            Message::FileEdit {
                file,
                on_complete,
            } => (file, on_complete),
        );
        // Make sure the initial content is present as expected
        assert_eq!(fs::read_to_string(file.path()).unwrap(), initial_content);

        // Simulate the editor modifying the file
        fs::write(file.path(), content).unwrap();
        on_complete(file);
        // Handle completion event
        component.int().drain_draw().assert().empty();
    }
}
