use crate::view::{
    common::{tabs::Tabs, template_preview::TemplatePreview},
    component::recipe_pane::{
        authentication::AuthenticationDisplay,
        body::RecipeBodyDisplay,
        table::{RecipeFieldTable, RecipeFieldTableProps},
    },
    context::PersistedLazy,
    draw::{Draw, DrawMetadata},
    event::EventHandler,
    Component,
};
use derive_more::Display;
use persisted::SingletonKey;
use ratatui::{layout::Layout, prelude::Constraint, widgets::Paragraph, Frame};
use serde::{Deserialize, Serialize};
use slumber_core::{
    collection::{Method, ProfileId, Recipe, RecipeId},
    http::BuildOptions,
};
use strum::{EnumCount, EnumIter};

/// Display a recipe. Note a recipe *node*, this is for genuine bonafide recipe.
/// This maintains internal state specific to a recipe, so it should be
/// recreated every time the recipe/profile changes.
#[derive(Debug)]
pub struct RecipeDisplay {
    tabs: Component<PersistedLazy<SingletonKey<Tab>, Tabs<Tab>>>,
    url: TemplatePreview,
    method: Method,
    query: Component<RecipeFieldTable<QueryRowKey, QueryRowToggleKey>>,
    headers: Component<RecipeFieldTable<HeaderRowKey, HeaderRowToggleKey>>,
    body: Option<Component<RecipeBodyDisplay>>,
    authentication: Option<Component<AuthenticationDisplay>>,
}

impl RecipeDisplay {
    /// Initialize new recipe state. Should be called whenever the recipe or
    /// profile changes
    pub fn new(
        recipe: &Recipe,
        selected_profile_id: Option<&ProfileId>,
    ) -> Self {
        Self {
            tabs: Default::default(),
            method: recipe.method,
            url: TemplatePreview::new(
                recipe.url.clone(),
                selected_profile_id.cloned(),
                None,
            ),
            query: RecipeFieldTable::new(
                QueryRowKey(recipe.id.clone()),
                selected_profile_id,
                recipe.query.iter().map(|(param, value)| {
                    (
                        param.clone(),
                        value.clone(),
                        QueryRowToggleKey {
                            recipe_id: recipe.id.clone(),
                            param: param.clone(),
                        },
                    )
                }),
            )
            .into(),
            headers: RecipeFieldTable::new(
                HeaderRowKey(recipe.id.clone()),
                selected_profile_id,
                recipe.headers.iter().map(|(header, value)| {
                    (
                        header.clone(),
                        value.clone(),
                        HeaderRowToggleKey {
                            recipe_id: recipe.id.clone(),
                            header: header.clone(),
                        },
                    )
                }),
            )
            .into(),
            body: recipe.body.as_ref().map(|body| {
                RecipeBodyDisplay::new(body, selected_profile_id, &recipe.id)
                    .into()
            }),
            // Map authentication type
            authentication: recipe.authentication.as_ref().map(
                |authentication| {
                    AuthenticationDisplay::new(
                        authentication,
                        selected_profile_id,
                    )
                    .into()
                },
            ),
        }
    }

    /// Generate a [BuildOptions] instance based on current UI state
    pub fn build_options(&self) -> BuildOptions {
        let disabled_form_fields = self
            .body
            .as_ref()
            .and_then(|body| match body.data() {
                RecipeBodyDisplay::Raw { .. } => None,
                RecipeBodyDisplay::Form(form) => {
                    Some(form.data().to_disabled_indexes())
                }
            })
            .unwrap_or_default();

        BuildOptions {
            disabled_headers: self.headers.data().to_disabled_indexes(),
            disabled_query_parameters: self.query.data().to_disabled_indexes(),
            disabled_form_fields,
        }
    }

    /// Does the recipe have a body defined?
    pub fn has_body(&self) -> bool {
        self.body.is_some()
    }
}

impl EventHandler for RecipeDisplay {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        [
            Some(self.tabs.as_child()),
            self.body.as_mut().map(Component::as_child),
            Some(self.query.as_child()),
            Some(self.headers.as_child()),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

impl Draw for RecipeDisplay {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        // Render request contents
        let method = self.method.to_string();

        let [metadata_area, tabs_area, content_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(metadata.area());

        let [method_area, url_area] = Layout::horizontal(
            // Method gets just as much as it needs, URL gets the rest
            [Constraint::Max(method.len() as u16 + 1), Constraint::Min(0)],
        )
        .areas(metadata_area);

        // First line: Method + URL
        frame.render_widget(Paragraph::new(method), method_area);
        frame.render_widget(&self.url, url_area);

        // Navigation tabs
        self.tabs.draw(frame, (), tabs_area, true);

        // Request content
        match self.tabs.data().selected() {
            Tab::Body => {
                if let Some(body) = &self.body {
                    body.draw(frame, (), content_area, true);
                }
            }
            Tab::Query => self.query.draw(
                frame,
                RecipeFieldTableProps {
                    key_header: "Parameter",
                    value_header: "Value",
                },
                content_area,
                true,
            ),
            Tab::Headers => self.headers.draw(
                frame,
                RecipeFieldTableProps {
                    key_header: "Header",
                    value_header: "Value",
                },
                content_area,
                true,
            ),
            Tab::Authentication => {
                if let Some(authentication) = &self.authentication {
                    authentication.draw(frame, (), content_area, true)
                }
            }
        }
    }
}

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Display,
    EnumCount,
    EnumIter,
    PartialEq,
    Serialize,
    Deserialize,
)]
enum Tab {
    #[default]
    Body,
    Query,
    Headers,
    Authentication,
}

/// Persistence key for selected query param, per recipe. Value is the query
/// param name
#[derive(Debug, Serialize, persisted::PersistedKey)]
#[persisted(Option<String>)]
struct QueryRowKey(RecipeId);

/// Persistence key for toggle state for a single query param in the table
#[derive(Debug, Serialize, persisted::PersistedKey)]
#[persisted(bool)]
struct QueryRowToggleKey {
    recipe_id: RecipeId,
    param: String,
}

/// Persistence key for selected header, per recipe. Value is the header name
#[derive(Debug, Serialize, persisted::PersistedKey)]
#[persisted(Option<String>)]
struct HeaderRowKey(RecipeId);

/// Persistence key for toggle state for a single header in the table
#[derive(Debug, Serialize, persisted::PersistedKey)]
#[persisted(bool)]
struct HeaderRowToggleKey {
    recipe_id: RecipeId,
    header: String,
}
