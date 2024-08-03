use crate::view::{
    common::{
        template_preview::TemplatePreview,
        text_window::{TextWindow, TextWindowProps},
    },
    component::recipe_pane::recipe::{to_table, PersistedTable, RowState},
    context::PersistedLazy,
    draw::{Draw, DrawMetadata, Generate},
    event::EventHandler,
    state::select::SelectState,
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
        /// Needed for syntax highlighting
        content_type: Option<ContentType>,
        preview: TemplatePreview,
        text_window: Component<TextWindow>,
    },
    Form(Component<PersistedTable<FormRowKey, FormRowToggleKey>>),
}

impl RecipeBodyDisplay {
    /// Build a component to display the body, based on the body type
    pub fn new(
        body: &RecipeBody,
        selected_profile_id: Option<ProfileId>,
        recipe_id: &RecipeId,
    ) -> Self {
        match body {
            RecipeBody::Raw(body) => Self::Raw {
                // Hypothetically we could grab the content type from the
                // Content-Type header above and plumb it down here, but more
                // effort than it's worth IMO. This gives users a solid reason
                // to use !json anyway
                content_type: None,
                preview: TemplatePreview::new(
                    body.clone(),
                    selected_profile_id,
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
                    content_type: Some(ContentType::Json),
                    preview: TemplatePreview::new(
                        template,
                        selected_profile_id,
                    ),
                    text_window: Component::default(),
                }
            }
            RecipeBody::FormUrlencoded(fields)
            | RecipeBody::FormMultipart(fields) => {
                let form_items = fields
                    .iter()
                    .map(|(field, value)| {
                        RowState::new(
                            field.clone(),
                            TemplatePreview::new(
                                value.clone(),
                                selected_profile_id.clone(),
                            ),
                            FormRowToggleKey {
                                recipe_id: recipe_id.clone(),
                                field: field.clone(),
                            },
                        )
                    })
                    .collect();
                let select = SelectState::builder(form_items)
                    .on_toggle(RowState::toggle)
                    .build();
                Self::Form(
                    PersistedLazy::new(FormRowKey(recipe_id.clone()), select)
                        .into(),
                )
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
                content_type,
                preview,
                text_window,
            } => text_window.draw(
                frame,
                TextWindowProps {
                    text: preview.generate(),
                    content_type: *content_type,
                    has_search_box: false,
                },
                metadata.area(),
                true,
            ),
            RecipeBodyDisplay::Form(form) => form.draw(
                frame,
                to_table(form.data(), ["", "Field", "Value"]).generate(),
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
