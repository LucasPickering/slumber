use crate::view::{
    common::{
        table::{Table, ToggleRow},
        tabs::Tabs,
        template_preview::TemplatePreview,
    },
    component::recipe_pane::{
        authentication::AuthenticationDisplay, body::RecipeBodyDisplay,
    },
    context::{Persisted, PersistedKey, PersistedLazy},
    draw::{Draw, DrawMetadata, Generate},
    event::EventHandler,
    state::select::SelectState,
    Component,
};
use derive_more::Display;
use itertools::Itertools;
use persisted::SingletonKey;
use ratatui::{
    layout::Layout,
    prelude::Constraint,
    widgets::{Paragraph, Row, TableState},
    Frame,
};
use serde::{Deserialize, Serialize};
use slumber_core::{
    collection::{HasId, Method, ProfileId, Recipe, RecipeId},
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
    query: Component<PersistedTable<QueryRowKey, QueryRowToggleKey>>,
    headers: Component<PersistedTable<HeaderRowKey, HeaderRowToggleKey>>,
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
        let query_items = recipe
            .query
            .iter()
            .map(|(param, value)| {
                RowState::new(
                    param.clone(),
                    TemplatePreview::new(
                        value.clone(),
                        selected_profile_id.cloned(),
                    ),
                    QueryRowToggleKey {
                        recipe_id: recipe.id.clone(),
                        param: param.clone(),
                    },
                )
            })
            .collect();
        let header_items = recipe
            .headers
            .iter()
            .map(|(header, value)| {
                RowState::new(
                    header.clone(),
                    TemplatePreview::new(
                        value.clone(),
                        selected_profile_id.cloned(),
                    ),
                    HeaderRowToggleKey {
                        recipe_id: recipe.id.clone(),
                        header: header.clone(),
                    },
                )
            })
            .collect();

        Self {
            tabs: Default::default(),
            method: recipe.method,
            url: TemplatePreview::new(
                recipe.url.clone(),
                selected_profile_id.cloned(),
            ),
            query: PersistedLazy::new(
                QueryRowKey(recipe.id.clone()),
                SelectState::builder(query_items)
                    .on_toggle(RowState::toggle)
                    .build(),
            )
            .into(),
            headers: PersistedLazy::new(
                HeaderRowKey(recipe.id.clone()),
                SelectState::builder(header_items)
                    .on_toggle(RowState::toggle)
                    .build(),
            )
            .into(),
            body: recipe.body.as_ref().map(|body| {
                RecipeBodyDisplay::new(
                    body,
                    selected_profile_id.cloned(),
                    &recipe.id,
                )
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
        /// Convert select state into the set of disabled keys
        fn to_disabled_indexes<K: PersistedKey<Value = bool>>(
            select_state: &SelectState<RowState<K>, TableState>,
        ) -> Vec<usize> {
            select_state
                .items()
                .iter()
                .enumerate()
                .filter(|(_, row)| !*row.value.enabled)
                .map(|(i, _)| i)
                .collect()
        }

        let disabled_form_fields = self
            .body
            .as_ref()
            .and_then(|body| match body.data() {
                RecipeBodyDisplay::Raw { .. } => None,
                RecipeBodyDisplay::Form(form) => {
                    Some(to_disabled_indexes(form.data()))
                }
            })
            .unwrap_or_default();

        BuildOptions {
            disabled_headers: to_disabled_indexes(self.headers.data()),
            disabled_query_parameters: to_disabled_indexes(self.query.data()),
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
                to_table(self.query.data(), ["", "Parameter", "Value"])
                    .generate(),
                content_area,
                true,
            ),
            Tab::Headers => self.headers.draw(
                frame,
                to_table(self.headers.data(), ["", "Header", "Value"])
                    .generate(),
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

/// A table of toggleable key:value rows. This has two persisted states:
/// - Track key of selected row (one entry per table)
/// - Track toggle state of each row (one entry per row)
pub type PersistedTable<RowSelectKey, RowToggleKey> = PersistedLazy<
    RowSelectKey,
    SelectState<RowState<RowToggleKey>, TableState>,
>;

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

/// One row in the query/header table. Generic param is the persistence key to
/// use for toggle state
#[derive(Debug)]
pub struct RowState<K: PersistedKey<Value = bool>> {
    key: String,
    value: TemplatePreview,
    enabled: Persisted<K>,
}

impl<K: PersistedKey<Value = bool>> RowState<K> {
    pub fn new(key: String, value: TemplatePreview, persisted_key: K) -> Self {
        Self {
            key,
            value,
            enabled: Persisted::new(persisted_key, true),
        }
    }

    pub fn toggle(&mut self) {
        *self.enabled.borrow_mut() ^= true;
    }
}

/// Needed for SelectState persistence
impl<K: PersistedKey<Value = bool>> HasId for RowState<K> {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.key
    }

    fn set_id(&mut self, id: Self::Id) {
        self.key = id;
    }
}

/// Needed for SelectState persistence
impl<K> PartialEq<RowState<K>> for String
where
    K: PersistedKey<Value = bool>,
{
    fn eq(&self, row_state: &RowState<K>) -> bool {
        self == &row_state.key
    }
}

/// Convert table select state into a renderable table
pub fn to_table<'a, K: PersistedKey<Value = bool>>(
    state: &'a SelectState<RowState<K>, TableState>,
    header: [&'a str; 3],
) -> Table<'a, 3, Row<'a>> {
    Table {
        rows: state
            .items()
            .iter()
            .map(|item| {
                let item = &item.value;
                ToggleRow::new(
                    [item.key.as_str().into(), item.value.generate()],
                    *item.enabled,
                )
                .generate()
            })
            .collect_vec(),
        header: Some(header),
        column_widths: &[
            Constraint::Min(3),
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ],
        ..Default::default()
    }
}
