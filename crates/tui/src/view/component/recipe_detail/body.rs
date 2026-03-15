use crate::{
    message::RecipeCopyTarget,
    view::{
        Component,
        component::{
            Canvas, ComponentId, Draw, DrawMetadata,
            editable_template::EditableTemplate,
            internal::{Child, ToChild},
            recipe_detail::table::{
                RecipeTable, RecipeTableKind, RecipeTableProps,
            },
        },
        persistent::SessionKey,
        util::preview::{JsonTemplate, Preview, StreamTemplate},
    },
};
use indexmap::IndexMap;
use slumber_core::{
    collection::{Recipe, RecipeBody, RecipeId},
    http::{BodyOverride, BuildFieldOverride},
};
use slumber_template::Template;

/// Render recipe body. The variant is based on the incoming body type, and
/// determines the representation
#[derive(Debug)]
pub struct RecipeBodyDisplay(Inner);

impl RecipeBodyDisplay {
    /// Build a component to display the body, based on the body type. This
    /// takes in the full recipe as well as the body so we can guarantee the
    /// body is not `None`.
    pub fn new(body: &RecipeBody, recipe: &Recipe) -> Self {
        fn build<T: Preview>(
            template: T,
            recipe: &Recipe,
        ) -> EditableTemplate<BodyKey, T> {
            EditableTemplate::builder(
                "Body",
                BodyKey(recipe.id.clone()),
                template.clone(),
            )
            .copy_target(RecipeCopyTarget::Body)
            .mime(recipe.mime())
            .window_mode(true)
            .build()
        }

        match body {
            RecipeBody::Raw(body) => {
                Self(Inner::Raw(build(body.clone(), recipe)))
            }
            RecipeBody::Stream(body) => {
                Self(Inner::Stream(build(StreamTemplate(body.clone()), recipe)))
            }
            RecipeBody::Json(json) => {
                Self(Inner::Json(build(JsonTemplate(json.clone()), recipe)))
            }
            RecipeBody::FormUrlencoded(fields) => {
                Self(Inner::Form(Self::form_table(&recipe.id, fields)))
            }
            RecipeBody::FormMultipart(fields) => {
                // TODO make this support streaming again
                Self(Inner::Form(Self::form_table(&recipe.id, fields)))
            }
        }
    }

    fn form_table(
        recipe_id: &RecipeId,
        fields: &IndexMap<String, Template>,
    ) -> RecipeTable<FormTableKind> {
        RecipeTable::new(
            "Field",
            recipe_id.clone(),
            fields
                .iter()
                .map(|(field, value)| (field.clone(), value.clone())),
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
            Inner::Stream(inner) => inner
                .override_template()
                .cloned()
                .map(|template| BodyOverride::Raw(template.0)),
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
            Inner::Raw(_) | Inner::Stream(_) | Inner::Json(_) => None,
            Inner::Form(form) => Some(form.to_build_overrides()),
        }
    }
}

impl Component for RecipeBodyDisplay {
    fn id(&self) -> ComponentId {
        match &self.0 {
            Inner::Raw(text_body) => text_body.id(),
            Inner::Stream(text_body) => text_body.id(),
            Inner::Json(text_body) => text_body.id(),
            Inner::Form(table) => table.id(),
        }
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        let child = match &mut self.0 {
            Inner::Raw(text_body) => text_body.to_child(),
            Inner::Stream(text_body) => text_body.to_child(),
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
            Inner::Stream(inner) => {
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
    Raw(EditableTemplate<BodyKey, Template>),
    /// A raw text or binary body where data is streamed from a source (e.g. a
    /// file)
    Stream(EditableTemplate<BodyKey, StreamTemplate>),
    /// A body declared with the `json` type. This is presented as text so it
    /// uses the same internal type as `Raw`, but the distinction allows us to
    /// parse and generate an override body correctly
    Json(EditableTemplate<BodyKey, JsonTemplate>),
    Form(RecipeTable<FormTableKind>),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message::Message,
        view::{
            context::ViewContext,
            persistent::PersistentStore,
            test_util::{TestComponent, TestHarness, harness},
        },
    };
    use indexmap::indexmap;
    use ratatui::{
        style::{Color, Styled},
        text::{Line, Span},
    };
    use rstest::rstest;
    use serde_json::json;
    use slumber_core::{
        collection::{Collection, Profile},
        render::TemplateContext,
        test_util::by_id,
    };
    use slumber_util::{Factory, assert_matches};
    use std::{fs, sync::Arc};
    use terminput::KeyCode;

    /// Test preview rendering
    ///
    /// Edge cases of each template type is tested within the preview module.
    /// This just tests that each preview is called correctly, particularly
    /// around streamed values.
    #[rstest]
    #[case::raw(RecipeBody::Raw(
        "{{ string }} {{ stream }}".into()),
        [
            "1 hello! stream!                            ".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
        ],
    )]
    #[case::stream(RecipeBody::Stream(
        "{{ string }} {{ stream }}".into()),
        [
            "1 hello! <command `echo -n stream!`>        ".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
        ],
    )]
    #[case::json(
        RecipeBody::Json(vec![
            ("number", "{{ number }}"),
            ("string", "{{ string }}"),
            ("stream", "{{ stream }}"),
        ].into()),
        [
            "1 {                                         ".into(),
            r#"2   "number": 4,"#.into(),
            r#"3   "string": "hello!","#.into(),
            // Streams are eagerly resolved
            r#"4   "stream": "stream!""#.into(),
            "5 }".into(),
        ]
    )]
    #[case::form_urlencoded(
        RecipeBody::FormUrlencoded(IndexMap::from_iter([
            ("f".into(), "{{ string }} {{ stream }}".into()),
        ])),
        [
            "    Field Value                             ".into(),
            // Streams are eagerly resolved
            "[x] f     hello! stream!".into(),
            "".into(),
            "".into(),
            "".into(),
        ]
    )]
    #[case::form_multipart(
        RecipeBody::FormMultipart(IndexMap::from_iter([
            ("f".into(), "{{ string }} {{ stream }}".into()),
        ])),
        [
            "    Field Value".into(),
            // Streams are *not* eagerly resolved
            "[x] f     hello! <command `echo -n stream!`>".into(),
            "".into(),
            "".into(),
            "".into(),
        ]
    )]
    #[tokio::test]
    async fn test_preview(
        #[case] body: RecipeBody,
        #[case] expected: impl IntoIterator<Item = Line<'static>>,
    ) {
        let collection = Collection {
            profiles: by_id([Profile {
                data: indexmap! {
                    "number".into() => 4.into(),
                    "string".into() => "hello!".into(),
                    "stream".into() => "{{ command(['echo', '-n', 'stream!']) }}".into()
                },
                ..Profile::factory(())
            }]),
            recipes: by_id([Recipe {
                body: Some(body),
                ..Recipe::factory(())
            }])
            .into(),
            ..Collection::factory(())
        };
        let mut harness = TestHarness::with_size(collection, 44, 5);
        let recipe = harness.collection.first_recipe();
        let component =
            RecipeBodyDisplay::new(recipe.body.as_ref().unwrap(), recipe);
        let mut component = TestComponent::new(&mut harness, component);

        // Render the previews
        let callback = assert_matches!(
            component.int(&mut harness).drain_draw().into_propagated(),
            [Message::TemplatePreview { callback }] => callback
        );
        let collection = Arc::clone(&harness.collection);
        let context = TemplateContext {
            selected_profile: Some(collection.first_profile_id().clone()),
            collection,
            ..TemplateContext::factory(())
        };
        callback(context).await;
        component.int(&mut harness).drain_draw().assert().empty();

        harness.assert_buffer_lines_unstyled(expected);
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
