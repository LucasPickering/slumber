use crate::{
    message::{Message, RecipeCopyTarget},
    util::{ResultReported, TempFile, syntax::SyntaxType},
    view::{
        Component, Generate,
        common::{
            actions::MenuItem,
            template_preview::{
                Preview, TemplatePreview, TemplatePreviewEvent,
                render_json_preview,
            },
            text_window::{TextWindow, TextWindowProps},
        },
        component::{
            Canvas, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            recipe_detail::table::{
                RecipeTable, RecipeTableKind, RecipeTableProps,
            },
        },
        context::{UpdateContext, ViewContext},
        event::{Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentStore, SessionKey},
        util::{highlight, view_text},
    },
};
use anyhow::Context as _;
use async_trait::async_trait;
use indexmap::IndexMap;
use mime::Mime;
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
};
use slumber_config::Action;
use slumber_core::{
    collection::{
        JsonTemplateError, Recipe, RecipeBody, RecipeId, ValueTemplate,
    },
    http::{BodyOverride, BuildFieldOverride},
};
use slumber_template::{Context, Template};
use std::{borrow::Cow, error::Error as StdError, fs, str::FromStr};
use tracing::debug;

/// Render recipe body. The variant is based on the incoming body type, and
/// determines the representation
#[derive(Debug)]
pub struct RecipeBodyDisplay(Inner);

impl RecipeBodyDisplay {
    /// Build a component to display the body, based on the body type. This
    /// takes in the full recipe as well as the body so we can guarantee the
    /// body is not `None`.
    pub fn new(body: &RecipeBody, recipe: &Recipe) -> Self {
        match body {
            RecipeBody::Raw(body) | RecipeBody::Stream(body) => {
                Self(Inner::Raw(TextBody::new(body.clone(), recipe)))
            }
            RecipeBody::Json(json) => Self(Inner::Json(TextBody::new(
                JsonTemplate(json.clone()),
                recipe,
            ))),
            RecipeBody::FormUrlencoded(fields) => {
                Self(Inner::Form(Self::form_table(&recipe.id, fields, false)))
            }
            RecipeBody::FormMultipart(fields) => {
                Self(Inner::Form(Self::form_table(&recipe.id, fields, true)))
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

    /// Get the user's temporary text body override (raw or JSON)
    ///
    /// Return `None` if this is not a text body or there's no override
    pub fn body_override(&self) -> Option<BodyOverride> {
        match &self.0 {
            Inner::Raw(inner) => {
                inner.override_template().cloned().map(BodyOverride::Raw)
            }
            Inner::Json(inner) => inner
                .override_template()
                .cloned()
                .map(|template| BodyOverride::Json(template.0)),
            // Form bodies override per-field so return None for them
            Inner::Form(_) => None,
        }
    }

    /// Get the user's temporary form field overrides
    ///
    /// Return `None` if this is not a form body or there are no overrides
    pub fn form_override(
        &self,
    ) -> Option<IndexMap<String, BuildFieldOverride>> {
        match &self.0 {
            Inner::Raw(_) | Inner::Json(_) => None,
            Inner::Form(form) => Some(form.to_build_overrides()),
        }
    }
}

impl Component for RecipeBodyDisplay {
    fn id(&self) -> ComponentId {
        match &self.0 {
            Inner::Raw(text_body) => text_body.id(),
            Inner::Json(text_body) => text_body.id(),
            Inner::Form(table) => table.id(),
        }
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        let child = match &mut self.0 {
            Inner::Raw(text_body) => text_body.to_child(),
            Inner::Json(text_body) => text_body.to_child(),
            Inner::Form(form) => form.to_child(),
        };
        vec![child]
    }
}

impl Draw for RecipeBodyDisplay {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        match &self.0 {
            Inner::Raw(inner) => {
                canvas.draw(inner, (), metadata.area(), true);
            }
            Inner::Json(inner) => {
                canvas.draw(inner, (), metadata.area(), true);
            }
            Inner::Form(form) => canvas.draw(
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

/// Inner state for [RecipeBodyDisplay]
///
/// This wrapper is needed so the contained types can be private
#[derive(Debug)]
enum Inner {
    /// A raw text body with no known content type
    Raw(TextBody<Template>),
    /// A body declared with the `json` type. This is presented as text so it
    /// uses the same internal type as `Raw`, but the distinction allows us to
    /// parse and generate an override body correctly
    Json(TextBody<JsonTemplate>),
    Form(RecipeTable<FormTableKind>),
}

/// A body represented and editable as a single block of text
///
/// The parameter `T` defines the template type of the body. Raw bodies use
/// [Template], JSON bodies use [JsonTemplate].
#[derive(Debug)]
struct TextBody<T: BodyTemplate> {
    id: ComponentId,
    /// Emitter for the callback from editing the body
    override_emitter: Emitter<SaveBodyOverride>,
    /// Emitter for menu actions
    actions_emitter: Emitter<RawBodyMenuAction>,
    /// The template from the collection
    original_template: T,
    /// Temporary override entered by the user
    ///
    /// Because the template is entered in an external editor, it's possible
    /// for the input to be invalid. In that case, we'll store the invalid
    /// source and the error and show them. We'll use the original template
    /// for request building while the override is invalid.
    override_result: Option<Result<T, (String, T::Err)>>,
    /// Persistent store key for temporary overrides
    persistent_key: BodyKey,
    /// Helper to render previews for the current template
    ///
    /// While the override is invalid, this will remain unused
    preview: TemplatePreview<T>,
    /// Body MIME type, used for syntax highlighting and pager selection. This
    /// has no impact on content of the rendered body
    mime: Option<Mime>,
    /// Visible template text
    ///
    /// This will start as the raw template text, but will be replaced with the
    /// rendered preview when available. If an *invalid* override is given, it
    /// will be the input string.
    text_window: TextWindow,
}

impl<T: BodyTemplate> TextBody<T> {
    fn new(template: T, recipe: &Recipe) -> Self {
        let mime = recipe.mime();

        let persistent_key = BodyKey(recipe.id.clone());
        let override_source = PersistentStore::get_session(&persistent_key);
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
                .unwrap_or(&template)
                .clone(),
            true,
            override_result.is_some(),
        );

        let mut slf = Self {
            id: ComponentId::default(),
            override_emitter: Default::default(),
            actions_emitter: Default::default(),
            original_template: template,
            override_result,
            persistent_key,
            preview,
            mime,
            text_window: TextWindow::default(),
        };

        // Start with the raw template text, until the preview loads
        slf.set_text(initial_text);

        slf
    }

    /// Open rendered body in the pager
    fn view_body(&self) {
        view_text(self.text_window.text(), self.mime.clone());
    }

    /// If a *valid* override is present for the body, return it
    fn override_template(&self) -> Option<&T> {
        match &self.override_result {
            Some(Ok(template)) => Some(template),
            Some(Err(_)) | None => None,
        }
    }

    /// Get the source the user inputted for the current override
    ///
    /// This is what we'll persist, as well as what we'll show when they re-open
    /// the editor.
    fn override_source(&self) -> Option<Cow<'_, str>> {
        self.override_result.as_ref().map(|result| match result {
            Ok(template) => template.display(),
            Err((source, _)) => source.into(),
        })
    }

    /// Send a message to open the body in an external editor. We have to write
    /// the body to a temp file so the editor subprocess can access it. We'll
    /// read it back later.
    fn open_editor(&mut self) {
        // If there's an existing override, use its source. Otherwise, start
        // with the default template
        let source = self
            .override_source()
            .unwrap_or_else(|| self.original_template.display());
        let Some(file) = TempFile::new(
            source.as_bytes(),
            self.mime.as_ref().and_then(|mime| {
                SyntaxType::from_mime(
                    ViewContext::config().mime_overrides(),
                    mime,
                )
            }),
        )
        .reported(&ViewContext::messages_tx()) else {
            // Write failed
            return;
        };
        debug!(?file, "Wrote body to file for editing");

        let emitter = self.override_emitter;
        ViewContext::push_message(Message::FileEdit {
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
        match body.parse::<T>() {
            Ok(template) if template != self.original_template => {
                // Show raw text until the preview loads
                let (preview, text) =
                    TemplatePreview::new(template.clone(), true, true);
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
                self.set_text(body.clone().into());
                // We have to store the input text separately from the display
                // text, so we can retrieve it when persisting and re-opening
                // the editor
                self.override_result = Some(Err((body, error)));
            }
        }
    }

    /// Remove the override and reset the preview to the original template
    fn reset_override(&mut self) {
        self.override_result = None;
        let (preview, text) =
            TemplatePreview::new(self.original_template.clone(), true, false);
        self.preview = preview;
        self.set_text(text);
    }

    /// Apply syntax highlight and present the text
    fn set_text(&mut self, text: Text<'static>) {
        let syntax_type = self.mime.as_ref().and_then(|mime| {
            SyntaxType::from_mime(ViewContext::config().mime_overrides(), mime)
        });
        let text = highlight::highlight_if(syntax_type, text);
        self.text_window = TextWindow::new(text);
    }
}

impl<T: BodyTemplate> Component for TextBody<T> {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::View => self.view_body(),
                Action::Edit => self.open_editor(),
                Action::Reset => self.reset_override(),
                _ => propagate.set(),
            })
            .emitted(self.override_emitter, |SaveBodyOverride(file)| {
                self.load_override(file);
            })
            .emitted(self.preview.to_emitter(), |TemplatePreviewEvent(text)| {
                // Don't accept the preview if we're currently showing invalid
                // text. This prevents delayed/refreshed previews from
                // overwriting the invalid override source (it's a bit jank)
                if matches!(&self.override_result, None | Some(Ok(_))) {
                    self.set_text(text);
                }
            })
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                RawBodyMenuAction::View => self.view_body(),
                RawBodyMenuAction::Copy => ViewContext::push_message(
                    Message::CopyRecipe(RecipeCopyTarget::Body),
                ),
                RawBodyMenuAction::Edit => self.open_editor(),
                RawBodyMenuAction::Reset => self.reset_override(),
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
                .enable(self.override_result.is_some())
                .shortcut(Some(Action::Reset))
                .into(),
        ]
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
        vec![self.preview.to_child(), self.text_window.to_child()]
    }
}

impl<T: BodyTemplate> Draw for TextBody<T>
where
    T::Err: StdError,
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
                let error_text = (error as &dyn StdError)
                    .generate()
                    .style(styles.text.error);
                canvas.render_widget(error_text, error_area);
                text_area
            }
        };
        canvas.draw(
            &self.text_window,
            TextWindowProps::default(),
            text_area,
            true,
        );
    }
}

/// Container for all the traits required for the type param of [TextBody]
trait BodyTemplate: Preview + FromStr {}

impl BodyTemplate for Template {}

impl BodyTemplate for JsonTemplate {}

/// A previewable wrapper of [ValueTemplate] for JSON bodies
#[derive(Clone, Debug, PartialEq)]
struct JsonTemplate(ValueTemplate);

impl FromStr for JsonTemplate {
    type Err = JsonTemplateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ValueTemplate::parse_json(s).map(Self)
    }
}

#[async_trait(?Send)]
impl Preview for JsonTemplate {
    fn display(&self) -> Cow<'_, str> {
        // Convert to serde_json so we can offload formatting
        let json: serde_json::Value = self.0.to_raw_json();
        format!("{json:#}").into()
    }

    fn is_dynamic(&self) -> bool {
        self.0.is_dynamic()
    }

    async fn render_preview<Ctx: Context>(
        &self,
        context: &Ctx,
    ) -> Text<'static> {
        render_json_preview(context, &self.0).await
    }
}

/// Persistent key for text body override template
#[derive(Clone, Debug, PartialEq)]
struct BodyKey(RecipeId);

impl SessionKey for BodyKey {
    // Template is persisted as its source so invalid templates are also
    // persisted
    type Value = String;
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
    use crate::view::{
        context::ViewContext,
        persistent::PersistentStore,
        test_util::{TestComponent, TestHarness, harness},
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
    fn test_edit(#[with(10, 1)] mut harness: TestHarness) {
        let recipe = Recipe {
            body: Some(RecipeBody::Raw("hello!".into())),
            ..Recipe::factory(())
        };
        let mut component = TestComponent::new(
            &mut harness,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        // Check initial state
        assert_eq!(component.body_override(), None);
        harness.assert_buffer_lines([vec![gutter("1"), " hello!  ".into()]]);

        // Edit the template
        edit(&mut component, &mut harness, "hello!", "goodbye!");

        assert_eq!(component.body_override(), Some("goodbye!".into()));
        harness.assert_buffer_lines([vec![
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
            .int(&mut harness)
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
        assert_eq!(component.body_override(), None);
    }

    /// Test edit and provide an invalid template. It should show the template
    /// with the error
    #[rstest]
    fn test_edit_invalid(#[with(20, 5)] mut harness: TestHarness) {
        let recipe = Recipe {
            body: Some(RecipeBody::Raw("init".into())),
            ..Recipe::factory(())
        };
        let mut component = TestComponent::new(
            &mut harness,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        // Open the editor
        edit(&mut component, &mut harness, "init", "{{");

        // We don't have a valid override, so we'll let the HTTP engine use the
        // original template
        assert_eq!(component.body_override(), None);
        harness.assert_buffer_lines([
            vec![gutter("1"), " ".into(), "{{".into()],
            vec![],
            vec![error("{{                  ")],
            vec![error("  ^                 ")],
            vec![error("invalid expression  ")],
        ]);

        // Invalid template is persisted
        let persisted =
            PersistentStore::get_session(&BodyKey(recipe.id.clone()));
        assert_eq!(persisted.as_deref(), Some("{{"));
    }

    /// Test editing a JSON body, which should open a file for the user to edit,
    /// then load the response
    #[rstest]
    fn test_edit_json(#[with(12, 1)] mut harness: TestHarness) {
        let initial_json = json!("hello!");
        let initial_text = initial_json.to_string();
        let override_json = json!("goodbye!");
        let override_text = override_json.to_string();

        let recipe = Recipe {
            body: Some(RecipeBody::json(initial_json.clone()).unwrap()),
            ..Recipe::factory(())
        };
        let mut component = TestComponent::new(
            &mut harness,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        // Check initial state
        assert_eq!(component.body_override(), None);
        harness.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            // Apply syntax highlighting
            Span::from(&initial_text).patch_style(Color::LightGreen),
            "  ".into(),
        ]]);

        // Open the editor
        edit(&mut component, &mut harness, &initial_text, &override_text);

        assert_eq!(component.body_override(), Some(override_json.into()));
        harness.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            // Apply syntax highlighting
            edited(&override_text).patch_style(Color::LightGreen),
        ]]);

        // Persistence store should be updated
        let persisted =
            PersistentStore::get_session(&BodyKey(recipe.id.clone()));
        assert_eq!(persisted, Some(override_text.parse().unwrap()));

        // Reset edited state
        component
            .int(&mut harness)
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
        assert_eq!(component.body_override(), None);
    }

    /// Override template should be loaded from the persistence store on init
    #[rstest]
    fn test_persisted_override(#[with(10, 1)] mut harness: TestHarness) {
        let recipe = Recipe {
            body: Some(RecipeBody::Raw("".into())),
            ..Recipe::factory(())
        };
        harness.set_session(BodyKey(recipe.id.clone()), "hello!".into());

        let component = TestComponent::new(
            &mut harness,
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), &recipe),
        );

        assert_eq!(component.body_override(), Some("hello!".into()));
        harness.assert_buffer_lines([vec![
            gutter("1"),
            " ".into(),
            edited("hello!"),
            "  ".into(),
        ]]);
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
