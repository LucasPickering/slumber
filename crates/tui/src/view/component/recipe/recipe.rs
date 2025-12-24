use crate::view::{
    Component,
    common::{fixed_select::FixedSelect, tabs::Tabs},
    component::{
        Canvas, ComponentId, Draw, DrawMetadata,
        internal::{Child, ToChild},
        override_template::TemplateOverrideKey,
        recipe::{
            authentication::AuthenticationDisplay,
            body::RecipeBodyDisplay,
            table::{RecipeTable, RecipeTableKey, RecipeTableProps},
            url::UrlDisplay,
        },
    },
    persistent::PersistentKey,
};
use derive_more::Display;
use ratatui::{layout::Layout, prelude::Constraint, widgets::Paragraph};
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
    tabs: Tabs<RecipeTabKey, Tab>,
    method: HttpMethod,
    url: UrlDisplay,
    query: RecipeTable<QueryKey>,
    headers: RecipeTable<HeaderKey>,
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
        let tabs = Tabs::new(
            RecipeTabKey,
            FixedSelect::builder().disabled(disabled_tabs),
        );

        Self {
            id: ComponentId::default(),
            tabs,
            method: recipe.method,
            url: UrlDisplay::new(recipe.id.clone(), recipe.url.clone()),
            query: RecipeTable::new(
                "Parameter",
                recipe.id.clone(),
                recipe
                    .query_iter()
                    .map(|(param, _, value)| (param.to_owned(), value.clone())),
                false,
            ),
            headers: RecipeTable::new(
                "Header",
                recipe.id.clone(),
                recipe
                    .headers
                    .iter()
                    .map(|(header, value)| (header.clone(), value.clone())),
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
        vec![
            self.url.to_child_mut(),
            self.body.to_child_mut(),
            self.query.to_child_mut(),
            self.headers.to_child_mut(),
            self.authentication.to_child_mut(),
            // Tabs last so edit text boxes can use left/right if needed
            self.tabs.to_child_mut(),
        ]
    }
}

impl Draw for RecipeDisplay {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
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
        canvas.render_widget(Paragraph::new(method), method_area);
        canvas.draw(self.url.preview(), (), url_area, false);

        // Navigation tabs
        canvas.draw(&self.tabs, (), tabs_area, true);

        // Recipe content
        match self.tabs.selected() {
            Tab::Url => canvas.draw(&self.url, (), content_area, true),
            Tab::Body => {
                if let Some(body) = &self.body {
                    canvas.draw(body, (), content_area, true);
                }
            }
            Tab::Query => canvas.draw(
                &self.query,
                RecipeTableProps {
                    key_header: "Parameter",
                    value_header: "Value",
                },
                content_area,
                true,
            ),
            Tab::Headers => canvas.draw(
                &self.headers,
                RecipeTableProps {
                    key_header: "Header",
                    value_header: "Value",
                },
                content_area,
                true,
            ),
            Tab::Authentication => {
                if let Some(authentication) = &self.authentication {
                    canvas.draw(authentication, (), content_area, true);
                }
            }
        }
    }
}

/// Persistence key for selected tab
#[derive(Debug, Default, Serialize)]
struct RecipeTabKey;

impl PersistentKey for RecipeTabKey {
    type Value = Tab;
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
    #[display("URL")]
    Url,
    Body,
    Query,
    Headers,
    Authentication,
}

/// [RecipeTableKey] implementation for the query parameter table
#[derive(Debug)]
struct QueryKey;

impl RecipeTableKey for QueryKey {
    type SelectKey = QueryRowKey;
    type ToggleKey = QueryRowToggleKey;

    fn select_key(recipe_id: RecipeId) -> Self::SelectKey {
        QueryRowKey(recipe_id)
    }

    fn toggle_key(recipe_id: RecipeId, key: String) -> Self::ToggleKey {
        QueryRowToggleKey {
            recipe_id,
            param: key,
        }
    }

    fn override_key(recipe_id: RecipeId, index: usize) -> TemplateOverrideKey {
        TemplateOverrideKey::query_param(recipe_id, index)
    }
}

/// Persistence key for selected query param, per recipe. Value is the query
/// param name
#[derive(Debug, Serialize)]
struct QueryRowKey(RecipeId);

impl PersistentKey for QueryRowKey {
    type Value = String;
}

/// Persistence key for toggle state for a single query param in the table
#[derive(Debug, Serialize)]
struct QueryRowToggleKey {
    recipe_id: RecipeId,
    param: String,
}

impl PersistentKey for QueryRowToggleKey {
    type Value = bool;
}

/// [RecipeTableKey] implementation for the header table
#[derive(Debug)]
struct HeaderKey;

impl RecipeTableKey for HeaderKey {
    type SelectKey = HeaderRowKey;
    type ToggleKey = HeaderRowToggleKey;

    fn select_key(recipe_id: RecipeId) -> Self::SelectKey {
        HeaderRowKey(recipe_id)
    }

    fn toggle_key(recipe_id: RecipeId, key: String) -> Self::ToggleKey {
        HeaderRowToggleKey {
            recipe_id,
            header: key,
        }
    }

    fn override_key(recipe_id: RecipeId, index: usize) -> TemplateOverrideKey {
        TemplateOverrideKey::header(recipe_id, index)
    }
}

/// Persistence key for selected header, per recipe. Value is the header name
#[derive(Debug, Serialize)]
struct HeaderRowKey(RecipeId);

impl PersistentKey for HeaderRowKey {
    type Value = String;
}

/// Persistence key for toggle state for a single header in the table
#[derive(Debug, Serialize)]
struct HeaderRowToggleKey {
    recipe_id: RecipeId,
    header: String,
}

impl PersistentKey for HeaderRowToggleKey {
    type Value = bool;
}
