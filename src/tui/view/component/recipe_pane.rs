use crate::{
    collection::{Authentication, ProfileId, Recipe, RecipeBody, RecipeId},
    http::BuildOptions,
    template::Template,
    tui::{
        context::TuiContext,
        input::Action,
        view::{
            common::{
                actions::ActionsModal,
                table::{Table, ToggleRow},
                tabs::Tabs,
                template_preview::TemplatePreview,
                text_window::{TextWindow, TextWindowProps},
                Pane,
            },
            component::primary::PrimaryPane,
            draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
            event::{Event, EventHandler, Update},
            state::{
                fixed_select::FixedSelect,
                persistence::{Persistable, Persistent, PersistentKey},
                select::SelectState,
                StateCell,
            },
            Component, ViewContext,
        },
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    layout::Layout,
    prelude::Constraint,
    widgets::{Paragraph, Row, TableState},
    Frame,
};
use serde::{Deserialize, Serialize};
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

#[derive(Clone)]
pub struct RecipePaneProps<'a> {
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
    body: Option<Component<RecipeBodyDisplay>>,
    authentication: Option<Component<AuthenticationDisplay>>,
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
impl FixedSelect for Tab {}

/// Items in the actions popup menu. This is also used by the recipe list
/// component, so the action is handled in the parent.
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum RecipeMenuAction {
    #[display("Copy URL")]
    CopyUrl,
    #[display("Copy Body")]
    CopyBody,
    #[display("Copy as cURL")]
    CopyCurl,
}

impl ToStringGenerate for RecipeMenuAction {}

impl RecipePane {
    /// Generate a [BuildOptions] instance based on current UI state
    pub fn build_options(&self) -> BuildOptions {
        if let Some(state) = self.recipe_state.get() {
            /// Convert select state into the set of disabled indexes
            fn to_disabled_indexes(
                select_state: &SelectState<RowState, TableState>,
            ) -> Vec<usize> {
                select_state
                    .items()
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| !*row.enabled)
                    .map(|(i, _)| i)
                    .collect()
            }

            let disabled_form_fields = state
                .body
                .as_ref()
                .and_then(|body| match body.data() {
                    RecipeBodyDisplay::Raw(_) => None,
                    RecipeBodyDisplay::Form(form) => {
                        Some(to_disabled_indexes(form.data()))
                    }
                })
                .unwrap_or_default();

            BuildOptions {
                disabled_headers: to_disabled_indexes(state.headers.data()),
                disabled_query_parameters: to_disabled_indexes(
                    state.query.data(),
                ),
                disabled_form_fields,
            }
        } else {
            // Shouldn't be possible, because state is initialized on first
            // render
            BuildOptions::default()
        }
    }
}

impl EventHandler for RecipePane {
    fn update(&mut self, event: Event) -> Update {
        if let Some(action) = event.action() {
            match action {
                Action::LeftClick => {
                    ViewContext::push_event(Event::new_local(
                        PrimaryPane::Recipe,
                    ));
                }
                Action::OpenActions => ViewContext::open_modal_default::<
                    ActionsModal<RecipeMenuAction>,
                >(),
                _ => return Update::Propagate(event),
            }
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        let mut children = vec![self.tabs.as_child()];

        // Send events to the tab pane as well
        if let Some(state) = self.recipe_state.get_mut() {
            children.extend(
                [
                    state.body.as_mut().map(Component::as_child),
                    Some(state.query.as_child()),
                    Some(state.headers.as_child()),
                ]
                .into_iter()
                .flatten(),
            );
        }

        children
    }
}

impl<'a> Draw<RecipePaneProps<'a>> for RecipePane {
    fn draw(
        &self,
        frame: &mut Frame,
        props: RecipePaneProps<'a>,
        metadata: DrawMetadata,
    ) {
        // Render outermost block
        let title = TuiContext::get()
            .input_engine
            .add_hint("Recipe", Action::SelectRecipe);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        };
        let block = block.generate();
        let inner_area = block.inner(metadata.area());
        frame.render_widget(block, metadata.area());

        // Render request contents
        if let Some(recipe) = props.selected_recipe {
            let method = recipe.method.to_string();

            let [metadata_area, tabs_area, content_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(inner_area);

            let [method_area, url_area] = Layout::horizontal(
                // Method gets just as much as it needs, URL gets the rest
                [Constraint::Max(method.len() as u16 + 1), Constraint::Min(0)],
            )
            .areas(metadata_area);

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
            self.tabs.draw(frame, (), tabs_area, true);

            // Request content
            match self.tabs.data().selected() {
                Tab::Body => {
                    if let Some(body) = &recipe_state.body {
                        body.draw(frame, (), content_area, true);
                    }
                }
                Tab::Query => recipe_state.query.draw(
                    frame,
                    to_table(
                        recipe_state.query.data(),
                        ["", "Parameter", "Value"],
                    )
                    .generate(),
                    content_area,
                    true,
                ),
                Tab::Headers => recipe_state.headers.draw(
                    frame,
                    to_table(
                        recipe_state.headers.data(),
                        ["", "Header", "Value"],
                    )
                    .generate(),
                    content_area,
                    true,
                ),
                Tab::Authentication => {
                    if let Some(authentication) = &recipe_state.authentication {
                        authentication.draw(frame, (), content_area, true)
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
                    TemplatePreview::new(
                        value.clone(),
                        selected_profile_id.cloned(),
                    ),
                    PersistentKey::RecipeQuery {
                        recipe_id: recipe.id.clone(),
                        param: param.clone(),
                        value: value.clone(),
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
                SelectState::builder(query_items)
                    .on_submit(RowState::on_submit)
                    .build(),
            )
            .into(),
            headers: Persistent::new(
                PersistentKey::RecipeSelectedHeader(recipe.id.clone()),
                SelectState::builder(header_items)
                    .on_submit(RowState::on_submit)
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
}

/// Display authentication settings
type AuthenticationDisplay = Authentication<TemplatePreview>;

impl AuthenticationDisplay {
    fn new(
        authentication: &Authentication<Template>,
        selected_profile_id: Option<&ProfileId>,
    ) -> Self {
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
    }
}

impl Draw for AuthenticationDisplay {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
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
                frame.render_widget(table.generate(), metadata.area())
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
                frame.render_widget(table.generate(), metadata.area())
            }
        }
    }
}

/// Render recipe body. The variant is based on the incoming body type, and
/// determines the representation
#[derive(Debug)]
enum RecipeBodyDisplay {
    Raw(Component<TextWindow<TemplatePreview>>),
    Form(Component<Persistent<SelectState<RowState, TableState>>>),
}

impl RecipeBodyDisplay {
    fn new(
        body: &RecipeBody,
        selected_profile_id: Option<ProfileId>,
        recipe_id: &RecipeId,
    ) -> Self {
        match body {
            RecipeBody::Raw(body) => Self::Raw(
                TextWindow::new(TemplatePreview::new(
                    body.clone(),
                    selected_profile_id,
                ))
                .into(),
            ),
            RecipeBody::Json(value) => {
                // We want to pretty-print the JSON body. We *could* map from
                // JsonBody<Template> -> JsonBody<TemplatePreview> then
                // stringify that on every render, but then we'd have to
                // implement JSON pretty printing ourselves. The easier method
                // is to just turn this whole JSON struct into a single string
                // (with unrendered templates), then parse that back as one big
                // template. If it's stupid but it works, it's not stupid.
                let value: serde_json::Value =
                    value.map_ref(|template| template.to_string()).into();
                let stringified = format!("{value:#}");
                // This template is composed valid templates, surrounded by JSON
                // syntax. In no world should that result in an invalid template
                let template = Template::parse(stringified)
                    .expect("Unexpected template parse failure");
                Self::Raw(
                    TextWindow::new(TemplatePreview::new(
                        template,
                        selected_profile_id,
                    ))
                    .into(),
                )
            }
            RecipeBody::FormUrlencoded(fields) => {
                let form_items = fields
                    .iter()
                    .map(|(field, value)| {
                        RowState::new(
                            field.clone(),
                            TemplatePreview::new(
                                value.clone(),
                                selected_profile_id.clone(),
                            ),
                            PersistentKey::RecipeFormField {
                                recipe: recipe_id.clone(),
                                field: field.clone(),
                            },
                        )
                    })
                    .collect();
                let select = SelectState::builder(form_items)
                    .on_submit(RowState::on_submit)
                    .build();
                Self::Form(
                    Persistent::new(
                        PersistentKey::RecipeSelectedFormField(
                            recipe_id.clone(),
                        ),
                        select,
                    )
                    .into(),
                )
            }
        }
    }
}

impl EventHandler for RecipeBodyDisplay {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        match self {
            RecipeBodyDisplay::Raw(preview) => vec![preview.as_child()],
            RecipeBodyDisplay::Form(form) => vec![form.as_child()],
        }
    }
}

impl Draw for RecipeBodyDisplay {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        match self {
            RecipeBodyDisplay::Raw(preview) => preview.draw(
                frame,
                TextWindowProps::default(),
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

/// One row in the query/header table
#[derive(Debug)]
struct RowState {
    key: String,
    value: TemplatePreview,
    enabled: Persistent<bool>,
}

impl RowState {
    fn new(
        key: String,
        value: TemplatePreview,
        persistent_key: PersistentKey,
    ) -> Self {
        Self {
            key,
            value,
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

/// Make row selectable by key, for persistence loading
impl PartialEq<RowState> for String {
    fn eq(&self, other: &RowState) -> bool {
        self == &other.key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::Factory,
        tui::{
            test_util::{harness, TestHarness},
            view::test_util::TestComponent,
        },
    };
    use crossterm::event::KeyCode;
    use rstest::rstest;

    /// Query params have special persistence because the keys aren't unique
    #[rstest]
    #[test]
    fn test_persist_query(harness: TestHarness) {
        let recipe = Recipe {
            query: vec![
                ("sudo".into(), "yes".into()),
                ("speed".into(), "slow".into()),
                ("speed".into(), "fast".into()),
            ],
            ..Recipe::factory(())
        };

        let mut component = TestComponent::new(
            harness,
            RecipePane::default(),
            RecipePaneProps {
                selected_recipe: Some(&recipe),
                selected_profile_id: None,
            },
        );

        // Check initial state
        {
            component.send_key(KeyCode::Right).assert_empty();
            let data = component.data();
            let state = data.recipe_state.get().unwrap();
            let query = state.query.data();
            assert_eq!(data.tabs.data().selected(), &Tab::Query);
            assert_eq!(query.selected_index(), Some(0));
            assert!(*query.items()[0].enabled);
            assert!(*query.items()[1].enabled);
            assert!(*query.items()[2].enabled);
        }

        // Disable the 2nd row
        {
            component.send_key(KeyCode::Down).assert_empty();
            component.send_key(KeyCode::Down).assert_empty();
            component.send_key(KeyCode::Enter).assert_empty();
            let state = component.data().recipe_state.get().unwrap();
            let query = state.query.data();
            assert_eq!(query.selected_index(), Some(2));
            assert!(*query.items()[0].enabled);
            assert!(*query.items()[1].enabled);
            assert!(!*query.items()[2].enabled);
        }

        // Dropping the component here will persist the values
        let harness = component.into_harness();
        // Rebuild the component, persisted values should be loaded
        let component = TestComponent::new(
            harness,
            RecipePane::default(),
            RecipePaneProps {
                selected_recipe: Some(&recipe),
                selected_profile_id: None,
            },
        );

        {
            let state = component.data().recipe_state.get().unwrap();
            let query = state.query.data();
            // This is actually a bug - it should be row 2, but we fell back to
            // the first instance of the param. Good enough for now though
            assert_eq!(query.selected_index(), Some(1));
            assert!(*query.items()[0].enabled);
            assert!(*query.items()[1].enabled);
            assert!(!*query.items()[2].enabled);
        }
    }
}
