use crate::{
    collection::{ProfileId, Recipe, RecipeId},
    http::RecipeOptions,
    template::Template,
    tui::{
        input::Action,
        view::{
            common::{
                actions::ActionsModal,
                table::{Table, ToggleRow},
                tabs::Tabs,
                template_preview::TemplatePreview,
                text_window::TextWindow,
                Pane,
            },
            component::primary::PrimaryPane,
            draw::{Draw, Generate, ToStringGenerate},
            event::{Event, EventHandler, Update, UpdateContext},
            state::{
                persistence::{Persistable, Persistent, PersistentKey},
                select::{Dynamic, SelectState},
                StateCell,
            },
            util::layout,
            Component,
        },
    },
    util::ResultExt,
};
use anyhow::Context;
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Constraint, Direction, Rect},
    widgets::{Paragraph, Row, TableState},
    Frame,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use strum::EnumIter;
use url::Url;

/// Display a request recipe
#[derive(Debug)]
pub struct RecipePane {
    tabs: Component<Tabs<Tab>>,
    /// All UI state derived from the recipe is stored together, and reset when
    /// the recipe or profile changes
    recipe_state: StateCell<RecipeStateKey, RecipeState>,
}

impl Default for RecipePane {
    fn default() -> Self {
        Self {
            tabs: Tabs::new(PersistentKey::RecipeTab).into(),
            recipe_state: Default::default(),
        }
    }
}

pub struct RequestPaneProps<'a> {
    pub is_selected: bool,
    pub selected_recipe: Option<&'a Recipe>,
    pub selected_profile_id: Option<&'a ProfileId>,
}

/// Template preview state will be recalculated when any of these fields change
#[derive(Debug, PartialEq)]
struct RecipeStateKey {
    selected_profile_id: Option<ProfileId>,
    recipe_id: RecipeId,
}

#[derive(Debug)]
struct RecipeState {
    url: TemplatePreview,
    query: Component<Persistent<SelectState<Dynamic, RowState, TableState>>>,
    headers: Component<Persistent<SelectState<Dynamic, RowState, TableState>>>,
    body: Option<Component<TextWindow<TemplatePreview>>>,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Display,
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
}

/// One row in the query/header table
#[derive(Debug)]
struct RowState {
    key: String,
    value: TemplatePreview,
    enabled: Persistent<bool>,
}

/// Items in the actions popup menu
#[derive(Copy, Clone, Debug, Default, Display, EnumIter, PartialEq)]
enum MenuAction {
    #[default]
    #[display("Copy URL")]
    CopyUrl,
    #[display("Copy Body")]
    CopyBody,
}

impl RecipePane {
    /// Generate a [RecipeOptions] instance based on current UI state
    pub fn recipe_options(&self) -> RecipeOptions {
        if let Some(state) = self.recipe_state.get() {
            /// Convert select state into the set of disabled keys
            fn to_disabled_set(
                select_state: &SelectState<Dynamic, RowState, TableState>,
            ) -> HashSet<String> {
                select_state
                    .items()
                    .iter()
                    .filter(|row| !*row.enabled)
                    .map(|row| row.key.clone())
                    .collect()
            }

            RecipeOptions {
                disabled_headers: to_disabled_set(&state.headers),
                disabled_query_parameters: to_disabled_set(&state.query),
            }
        } else {
            // Shouldn't be possible, because state is initialized on first
            // render
            RecipeOptions::default()
        }
    }

    /// Get the full URL (including query params) of the request, for copypasta
    fn url(&self) -> Option<String> {
        self.recipe_state.get().and_then(|state| {
            // Parse the base URL
            let mut url = Url::parse(&state.url.to_copy_text())
                .context("Error parsing URL")
                .traced()
                .ok()?;

            // Add additional query params. We need to make sure to append, in
            // case the base URL includes some query params already
            {
                let mut query_pairs = url.query_pairs_mut();
                for row in state.query.items() {
                    // Don't include disabled params
                    if *row.enabled {
                        query_pairs
                            .append_pair(&row.key, &row.value.to_copy_text());
                    }
                }
            }

            Some(url.to_string())
        })
    }

    /// Get the full *prettified* body of the request, for copypasta
    fn body(&self) -> Option<String> {
        self.recipe_state.get().and_then(|state| {
            state
                .body
                .as_ref()
                .map(|body| body.inner().text().to_copy_text())
        })
    }
}

impl EventHandler for RecipePane {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match &event {
            Event::Input {
                action: Some(Action::OpenActions),
                ..
            } => context.open_modal_default::<ActionsModal<MenuAction>>(),
            Event::Other(callback) => {
                match callback.downcast_ref::<MenuAction>() {
                    Some(MenuAction::CopyUrl) => {
                        if let Some(url) = self.url() {
                            context.copy_text(url);
                        }
                    }
                    Some(MenuAction::CopyBody) => {
                        if let Some(body) = self.body() {
                            context.copy_text(body);
                        }
                    }
                    None => return Update::Propagate(event),
                }
            }
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        let selected_tab = *self.tabs.selected();
        let mut children = vec![self.tabs.as_child()];

        // Send events to the tab pane as well
        if let Some(state) = self.recipe_state.get_mut() {
            match selected_tab {
                Tab::Body => {
                    if let Some(body) = state.body.as_mut() {
                        children.push(body.as_child());
                    }
                }
                Tab::Query => children.push(state.query.as_child()),
                Tab::Headers => children.push(state.headers.as_child()),
            }
        }

        children
    }
}

impl<'a> Draw<RequestPaneProps<'a>> for RecipePane {
    fn draw(&self, frame: &mut Frame, props: RequestPaneProps<'a>, area: Rect) {
        // Render outermost block
        let pane_kind = PrimaryPane::Request;
        let block = Pane {
            title: &pane_kind.to_string(),
            is_focused: props.is_selected,
        };
        let block = block.generate();
        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // Render request contents
        if let Some(recipe) = props.selected_recipe {
            let [metadata_area, tabs_area, content_area] = layout(
                inner_area,
                Direction::Vertical,
                [
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ],
            );

            let [method_area, url_area] = layout(
                metadata_area,
                Direction::Horizontal,
                // Method gets just as much as it needs, URL gets the rest
                [
                    Constraint::Max(recipe.method.len() as u16 + 1),
                    Constraint::Min(0),
                ],
            );

            // Whenever the recipe or profile changes, generate a preview for
            // each templated value. Almost anything that could change the
            // preview will either involve changing one of those two things, or
            // would require reloading the whole collection which will reset
            // UI state.
            let recipe_state = self.recipe_state.get_or_update(
                RecipeStateKey {
                    selected_profile_id: props.selected_profile_id.cloned(),
                    recipe_id: recipe.id.clone(),
                },
                || RecipeState::new(recipe, props.selected_profile_id),
            );

            // First line: Method + URL
            frame.render_widget(
                Paragraph::new(recipe.method.as_str()),
                method_area,
            );
            frame.render_widget(&recipe_state.url, url_area);

            // Navigation tabs
            self.tabs.draw(frame, (), tabs_area);

            // Request content
            match self.tabs.selected() {
                Tab::Body => {
                    if let Some(body) = &recipe_state.body {
                        body.draw(frame, (), content_area);
                    }
                }
                Tab::Query => frame.render_stateful_widget(
                    to_table(&recipe_state.query, ["Parameter", "Value", ""])
                        .generate(),
                    content_area,
                    &mut recipe_state.query.state_mut(),
                ),
                Tab::Headers => frame.render_stateful_widget(
                    to_table(&recipe_state.headers, ["Header", "Value", ""])
                        .generate(),
                    content_area,
                    &mut recipe_state.headers.state_mut(),
                ),
            }
        }
    }
}

impl RecipeState {
    /// Initialize new recipe state. Should be called whenever the recipe or
    /// profile changes
    fn new(recipe: &Recipe, selected_profile_id: Option<&ProfileId>) -> Self {
        let query_items = recipe
            .query
            .iter()
            .map(|(param, value)| {
                RowState::new(
                    param.clone(),
                    value.clone(),
                    selected_profile_id.cloned(),
                    PersistentKey::RecipeQuery {
                        recipe: recipe.id.clone(),
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
                    value.clone(),
                    selected_profile_id.cloned(),
                    PersistentKey::RecipeHeader {
                        recipe: recipe.id.clone(),
                        header: header.clone(),
                    },
                )
            })
            .collect();

        Self {
            url: TemplatePreview::new(
                recipe.url.clone(),
                selected_profile_id.cloned(),
            ),
            query: Persistent::new(
                PersistentKey::RecipeSelectedQuery(recipe.id.clone()),
                SelectState::new(query_items).on_submit(RowState::on_submit),
            )
            .into(),
            headers: Persistent::new(
                PersistentKey::RecipeSelectedHeader(recipe.id.clone()),
                SelectState::new(header_items).on_submit(RowState::on_submit),
            )
            .into(),
            body: recipe.body.as_ref().map(|body| {
                TextWindow::new(TemplatePreview::new(
                    body.clone(),
                    selected_profile_id.cloned(),
                ))
                .into()
            }),
        }
    }
}

impl RowState {
    fn new(
        key: String,
        value: Template,
        selected_profile_id: Option<ProfileId>,
        persistent_key: PersistentKey,
    ) -> Self {
        Self {
            key,
            value: TemplatePreview::new(value, selected_profile_id),
            enabled: Persistent::new(
                persistent_key,
                // Value itself is the container, so just pass a default value
                true,
            ),
        }
    }

    /// Toggle row state on submit
    fn on_submit(_: &mut UpdateContext, row: &mut Self) {
        *row.enabled ^= true;
    }
}

/// Convert table select state into a renderable table
fn to_table<'a>(
    state: &'a SelectState<Dynamic, RowState, TableState>,
    header: [&'a str; 3],
) -> Table<'a, 3, Row<'a>> {
    Table {
        rows: state
            .items()
            .iter()
            .map(|item| {
                ToggleRow::new(
                    [item.key.as_str().into(), item.value.generate()],
                    *item.enabled,
                )
                .generate()
            })
            .collect_vec(),
        header: Some(header),
        column_widths: &[
            Constraint::Percentage(50),
            Constraint::Percentage(50),
            Constraint::Min(3),
        ],
        ..Default::default()
    }
}

/// This impl persists just which row is *selected*
impl Persistable for RowState {
    type Persisted = String;

    fn get_persistent(&self) -> &Self::Persisted {
        &self.key
    }
}

impl PartialEq<RowState> for String {
    fn eq(&self, other: &RowState) -> bool {
        self == &other.key
    }
}

impl ToStringGenerate for MenuAction {}
