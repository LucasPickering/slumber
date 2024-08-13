use crate::view::{
    common::{
        template_preview::TemplatePreview,
        text_window::{TextWindow, TextWindowProps},
    },
    component::recipe_pane::table::{RecipeFieldTable, RecipeFieldTableProps},
    draw::{Draw, DrawMetadata},
    event::EventHandler,
    Component,
};
use ratatui::Frame;
use serde::Serialize;
use slumber_core::{
    collection::{ProfileId, RecipeBody, RecipeId},
    http::content_type::ContentType,
};

/// Render recipe body. The variant is based on the incoming body type, and
/// determines the representation
#[derive(Debug)]
pub enum RecipeBodyDisplay {
    Raw {
        preview: TemplatePreview,
        text_window: Component<TextWindow>,
    },
    Form(Component<RecipeFieldTable<FormRowKey, FormRowToggleKey>>),
}

impl RecipeBodyDisplay {
    /// Build a component to display the body, based on the body type
    pub fn new(
        body: &RecipeBody,
        selected_profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> Self {
        match body {
            RecipeBody::Raw(body) => Self::Raw {
                preview: TemplatePreview::new(
                    body.clone(),
                    selected_profile_id.cloned(),
                    // Hypothetically we could grab the content type from the
                    // Content-Type header above and plumb it down here, but
                    // more effort than it's worth IMO. This gives users a
                    // solid reason to use !json anyway
                    None,
                ),
                text_window: Component::default(),
            },
            RecipeBody::Json(value) => {
                // We want to pretty-print the JSON body. We *could* map from
                // JsonBody<Template> -> JsonBody<TemplatePreview> then
                // stringify that on every render, but then we'd have to
                // implement JSON pretty printing ourselves. The easier method
                // is to just turn this whole JSON struct into a single string
                // (with unrendered templates), then parse that back as one big
                // template. If it's stupid but it works, it's not stupid.
                let value: serde_json::Value = value
                    .map_ref(|template| template.display().to_string())
                    .into();
                let stringified = format!("{value:#}");
                // This template is made of valid templates, surrounded by JSON
                // syntax. In no world should that result in an invalid template
                let template = stringified
                    .parse()
                    .expect("Unexpected template parse failure");
                Self::Raw {
                    preview: TemplatePreview::new(
                        template,
                        selected_profile_id.cloned(),
                        Some(ContentType::Json),
                    ),
                    text_window: Component::default(),
                }
            }
            RecipeBody::FormUrlencoded(fields)
            | RecipeBody::FormMultipart(fields) => {
                let inner = RecipeFieldTable::new(
                    FormRowKey(recipe_id.clone()),
                    selected_profile_id.cloned(),
                    fields.iter().map(|(field, value)| {
                        (
                            field.clone(),
                            value.clone(),
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
}

impl EventHandler for RecipeBodyDisplay {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        match self {
            RecipeBodyDisplay::Raw { text_window, .. } => {
                vec![text_window.as_child()]
            }
            RecipeBodyDisplay::Form(form) => vec![form.as_child()],
        }
    }
}

impl Draw for RecipeBodyDisplay {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        match self {
            RecipeBodyDisplay::Raw {
                preview,
                text_window,
            } => text_window.draw(
                frame,
                TextWindowProps {
                    // Do *not* call generate, because that clones the text and
                    // we only need a reference
                    text: &preview.text(),
                    margins: Default::default(),
                },
                metadata.area(),
                true,
            ),
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
