use crate::{
    collection::{Authentication, ProfileId, Recipe, RecipeId},
    http::RecipeOptions,
    template::Template,
    tui::{
        context::TuiContext,
        input::Action,
        message::{Message, RequestConfig},
        view::{
            common::{
                actions::ActionsModal,
                table::{Table, ToggleRow},
                tabs::Tabs,
                template_preview::TemplatePreview,
                text_window::TextWindow,
                Pane,
            },
            draw::{Draw, Generate, ToStringGenerate},
            event::{Event, EventHandler, EventQueue, Update},
            state::{
                persistence::{Persistable, Persistent, PersistentKey},
                select::SelectState,
                StateCell,
            },
            util::layout,
            Component,
        },
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Constraint, Direction, Rect},
    widgets::{Paragraph, Row, TableState},
    Frame,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use strum::{EnumCount, EnumIter};

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

pub struct RecipePaneProps<'a> {
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
    query: Component<Persistent<SelectState<RowState, TableState>>>,
    headers: Component<Persistent<SelectState<RowState, TableState>>>,
    body: Option<Component<TextWindow<TemplatePreview>>>,
    authentication: Option<Component<AuthenticationDisplay>>,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Display,
    EnumCount,
    EnumIter,
    PartialEq,
    Serialize,
    Deserialize,
)]
enum Tab {
    Body,
    Query,
    Headers,
    Authentication,
}

/// One row in the query/header table
#[derive(Debug)]
struct RowState {
    key: String,
    value: TemplatePreview,
    enabled: Persistent<bool>,
}

/// Items in the actions popup menu
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
#[allow(clippy::enum_variant_names)]
enum MenuAction {
    #[display("Copy URL")]
    CopyUrl,
    // TODO disable this if request doesn't have body
    #[display("Copy Body")]
    CopyBody,
    #[display("Copy as cURL")]
    CopyCurl,
}

impl ToStringGenerate for MenuAction {}

impl RecipePane {
    /// Generate a [RecipeOptions] instance based on current UI state
    pub fn recipe_options(&self) -> RecipeOptions {
        if let Some(state) = self.recipe_state.get() {
            /// Convert select state into the set of disabled keys
            fn to_disabled_set(
                select_state: &SelectState<RowState, TableState>,
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

    fn handle_menu_action(&mut self, action: MenuAction) {
        // Should always be initialized after first render
        let key = self
            .recipe_state
            .key()
            .expect("Request state not initialized");
        let request_config = RequestConfig {
            profile_id: key.selected_profile_id.clone(),
            recipe_id: key.recipe_id.clone(),
            options: self.recipe_options(),
        };
        let message = match action {
            MenuAction::CopyUrl => Message::CopyRequestUrl(request_config),
            MenuAction::CopyBody => Message::CopyRequestBody(request_config),
            MenuAction::CopyCurl => Message::CopyRequestCurl(request_config),
        };
        TuiContext::send_message(message);
    }
}

impl EventHandler for RecipePane {
    fn update(&mut self, event: Event) -> Update {
        match &event {
            Event::Input {
                action: Some(Action::OpenActions),
                ..
            } => EventQueue::open_modal_default::<ActionsModal<MenuAction>>(),
            Event::Other(callback) => {
                match callback.downcast_ref::<MenuAction>() {
                    Some(action) => {
                        self.handle_menu_action(*action);
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
                Tab::Authentication => {}
            }
        }

        children
    }
}

impl<'a> Draw<RecipePaneProps<'a>> for RecipePane {
    fn draw(&self, frame: &mut Frame, props: RecipePaneProps<'a>, area: Rect) {
        // Render outermost block
        let title = TuiContext::get()
            .input_engine
            .add_hint("Recipe", Action::SelectRecipe);
        let block = Pane {
            title: &title,
            is_focused: props.is_selected,
        };
        let block = block.generate();
        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // Render request contents
        if let Some(recipe) = props.selected_recipe {
            let method = recipe.method.to_string();

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
                [Constraint::Max(method.len() as u16 + 1), Constraint::Min(0)],
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
            frame.render_widget(Paragraph::new(method), method_area);
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
                    to_table(&recipe_state.query, ["", "Parameter", "Value"])
                        .generate(),
                    content_area,
                    &mut recipe_state.query.state_mut(),
                ),
                Tab::Headers => frame.render_stateful_widget(
                    to_table(&recipe_state.headers, ["", "Header", "Value"])
                        .generate(),
                    content_area,
                    &mut recipe_state.headers.state_mut(),
                ),
                Tab::Authentication => {
                    if let Some(authentication) = &recipe_state.authentication {
                        authentication.draw(frame, (), content_area)
                    }
                }
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
            // Map authentication type
            authentication: recipe.authentication.as_ref().map(
                |authentication| {
                    match authentication {
                        Authentication::Basic { username, password } => {
                            AuthenticationDisplay::Basic {
                                username: TemplatePreview::new(
                                    username.clone(),
                                    selected_profile_id.cloned(),
                                ),
                                password: password.clone().map(|password| {
                                    TemplatePreview::new(
                                        password,
                                        selected_profile_id.cloned(),
                                    )
                                }),
                            }
                        }
                        Authentication::Bearer(token) => {
                            AuthenticationDisplay::Bearer(TemplatePreview::new(
                                token.clone(),
                                selected_profile_id.cloned(),
                            ))
                        }
                    }
                    .into() // Convert to Component
                },
            ),
        }
    }
}

/// Display authentication settings. This is basically the underlying
/// [Authentication] type, but the templates have been rendered
#[derive(Debug)]
enum AuthenticationDisplay {
    Basic {
        username: TemplatePreview,
        password: Option<TemplatePreview>,
    },
    Bearer(TemplatePreview),
}

impl Draw for AuthenticationDisplay {
    fn draw(&self, frame: &mut Frame, _: (), area: Rect) {
        match self {
            AuthenticationDisplay::Basic { username, password } => {
                let table = Table {
                    rows: vec![
                        ["Type".into(), "Bearer".into()],
                        ["Username".into(), username.generate()],
                        [
                            "Password".into(),
                            password
                                .as_ref()
                                .map(Generate::generate)
                                .unwrap_or_default(),
                        ],
                    ],
                    column_widths: &[Constraint::Length(8), Constraint::Min(0)],
                    ..Default::default()
                };
                frame.render_widget(table.generate(), area)
            }
            AuthenticationDisplay::Bearer(token) => {
                let table = Table {
                    rows: vec![
                        ["Type".into(), "Bearer".into()],
                        ["Token".into(), token.generate()],
                    ],
                    column_widths: &[Constraint::Length(5), Constraint::Min(0)],
                    ..Default::default()
                };
                frame.render_widget(table.generate(), area)
            }
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
    fn on_submit(row: &mut Self) {
        *row.enabled ^= true;
    }
}

/// Convert table select state into a renderable table
fn to_table<'a>(
    state: &'a SelectState<RowState, TableState>,
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
            Constraint::Min(3),
            Constraint::Percentage(50),
            Constraint::Percentage(50),
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
