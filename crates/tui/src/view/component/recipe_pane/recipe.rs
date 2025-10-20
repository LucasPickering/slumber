use crate::view::{
    Component,
    common::tabs::Tabs,
    component::{
        ComponentExt, ComponentId, Draw, DrawMetadata,
        internal::{Child, ToChild},
        recipe_pane::{
            authentication::AuthenticationDisplay,
            body::RecipeBodyDisplay,
            persistence::RecipeOverrideKey,
            table::{RecipeFieldTable, RecipeFieldTableProps},
            url::UrlDisplay,
        },
    },
    state::fixed_select::FixedSelectState,
    util::persistence::PersistedLazy,
};
use derive_more::Display;
use ratatui::{Frame, layout::Layout, prelude::Constraint, widgets::Paragraph};
use serde::{Deserialize, Serialize};
use slumber_core::{
    collection::{Recipe, RecipeId},
    http::{BuildOptions, HttpMethod},
};
use std::iter;
use strum::{EnumCount, EnumIter};

/// Display a recipe. Not a recipe *node*, this is for genuine bonafide recipe.
/// This maintains internal state specific to a recipe, so it should be
/// recreated every time the recipe/profile changes.
#[derive(Debug)]
pub struct RecipeDisplay {
    id: ComponentId,
    tabs: PersistedLazy<RecipeTabKey, Tabs<Tab>>,
    method: HttpMethod,
    url: UrlDisplay,
    query: RecipeFieldTable<QueryRowKey, QueryRowToggleKey>,
    headers: RecipeFieldTable<HeaderRowKey, HeaderRowToggleKey>,
    body: Option<RecipeBodyDisplay>,
    authentication: Option<AuthenticationDisplay>,
}

impl RecipeDisplay {
    /// Initialize new recipe state. Should be called whenever the recipe or
    /// profile changes
    pub fn new(recipe: &Recipe) -> Self {
        // Disable tabs that have no content
        let disabled_tabs = iter::empty()
            .chain(recipe.body.is_none().then_some(Tab::Body))
            .chain(
                recipe
                    .authentication
                    .is_none()
                    .then_some(Tab::Authentication),
            );
        let tabs = PersistedLazy::new(
            RecipeTabKey,
            Tabs::new(
                FixedSelectState::builder().disabled(disabled_tabs).build(),
            ),
        );

        Self {
            id: ComponentId::default(),
            tabs,
            method: recipe.method,
            url: UrlDisplay::new(recipe.id.clone(), recipe.url.clone()),
            query: RecipeFieldTable::new(
                "Parameter",
                QueryRowKey(recipe.id.clone()),
                recipe.query_iter().enumerate().map(
                    |(i, (param, _, value))| {
                        (
                            param.to_owned(),
                            value.clone(),
                            RecipeOverrideKey::query_param(
                                recipe.id.clone(),
                                i,
                            ),
                            QueryRowToggleKey {
                                recipe_id: recipe.id.clone(),
                                param: param.to_owned(),
                            },
                        )
                    },
                ),
                false,
            ),
            headers: RecipeFieldTable::new(
                "Header",
                HeaderRowKey(recipe.id.clone()),
                recipe.headers.iter().enumerate().map(
                    |(i, (header, value))| {
                        (
                            header.clone(),
                            value.clone(),
                            RecipeOverrideKey::header(recipe.id.clone(), i),
                            HeaderRowToggleKey {
                                recipe_id: recipe.id.clone(),
                                header: header.clone(),
                            },
                        )
                    },
                ),
                false,
            ),
            body: recipe
                .body
                .as_ref()
                .map(|body| RecipeBodyDisplay::new(body, recipe)),
            // Map authentication type
            authentication: recipe.authentication.as_ref().map(
                |authentication| {
                    AuthenticationDisplay::new(
                        recipe.id.clone(),
                        authentication.clone(),
                    )
                },
            ),
        }
    }

    /// Generate a [BuildOptions] instance based on current UI state
    pub fn build_options(&self) -> BuildOptions {
        let url = self.url.override_value();
        let authentication = self.authentication.as_ref().and_then(
            super::authentication::AuthenticationDisplay::override_value,
        );
        let form_fields = self
            .body
            .as_ref()
            .and_then(|body| match body {
                RecipeBodyDisplay::Raw(_) | RecipeBodyDisplay::Json(_) => None,
                RecipeBodyDisplay::Form(form) => {
                    Some(form.to_build_overrides())
                }
            })
            .unwrap_or_default();
        let body = self
            .body
            .as_ref()
            .and_then(super::body::RecipeBodyDisplay::override_value);

        BuildOptions {
            url,
            authentication,
            headers: self.headers.to_build_overrides(),
            query_parameters: self.query.to_build_overrides(),
            form_fields,
            body,
        }
    }
}

impl Component for RecipeDisplay {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        [
            Some(self.tabs.to_child_mut()),
            Some(self.url.to_child_mut()),
            self.body.as_mut().map(ToChild::to_child_mut),
            Some(self.query.to_child_mut()),
            Some(self.headers.to_child_mut()),
            self.authentication.as_mut().map(ToChild::to_child_mut),
        ]
        .into_iter()
        .flatten() // Remove None
        .collect()
    }
}

impl Draw for RecipeDisplay {
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
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
        frame.render_widget(self.url.preview(), url_area);

        // Navigation tabs
        self.tabs.draw(frame, (), tabs_area, true);

        // Recipe content
        match self.tabs.selected() {
            Tab::Url => self.url.draw(frame, (), content_area, true),
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
                    authentication.draw(frame, (), content_area, true);
                }
            }
        }
    }
}

/// Persistence key for selected tab
#[derive(Debug, Default, persisted::PersistedKey, Serialize)]
#[persisted(Tab)]
struct RecipeTabKey;

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
    #[display("URL")]
    Url,
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
