use crate::{
    collection::{ProfileId, RequestRecipe, RequestRecipeId},
    template::Template,
    tui::view::{
        common::{
            table::Table, tabs::Tabs, template_preview::TemplatePreview,
            text_window::TextWindow, Pane,
        },
        component::primary::PrimaryPane,
        draw::{Draw, DrawContext, Generate},
        event::EventHandler,
        state::{persistence::PersistentKey, StateCell},
        util::layout,
        Component,
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Constraint, Direction, Rect},
    widgets::Paragraph,
};
use serde::{Deserialize, Serialize};
use strum::EnumIter;

/// Display a request recipe
#[derive(Debug)]
pub struct RequestPane {
    tabs: Component<Tabs<Tab>>,
    /// All UI state derived from the recipe is stored together, and reset when
    /// the recipe or profile changes
    recipe_state: StateCell<RecipeStateKey, RecipeState>,
}

impl Default for RequestPane {
    fn default() -> Self {
        Self {
            tabs: Tabs::new(PersistentKey::RequestTab).into(),
            recipe_state: Default::default(),
        }
    }
}

pub struct RequestPaneProps<'a> {
    pub is_selected: bool,
    pub selected_recipe: Option<&'a RequestRecipe>,
    pub selected_profile_id: Option<&'a ProfileId>,
}

/// Template preview state will be recalculated when any of these fields change
#[derive(Debug, PartialEq)]
struct RecipeStateKey {
    selected_profile_id: Option<ProfileId>,
    recipe_id: RequestRecipeId,
    preview_templates: bool,
}

#[derive(Debug)]
struct RecipeState {
    url: TemplatePreview,
    query: Vec<(String, TemplatePreview)>,
    headers: Vec<(String, TemplatePreview)>,
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

impl EventHandler for RequestPane {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        let selected_tab = *self.tabs.selected();
        let mut children = vec![self.tabs.as_child()];
        match selected_tab {
            Tab::Body => {
                // If the body is initialized and present, send events there too
                if let Some(body) = self
                    .recipe_state
                    .get_mut()
                    .and_then(|state| state.body.as_mut())
                {
                    children.push(body.as_child());
                }
            }
            Tab::Query => {}
            Tab::Headers => {}
        }
        children
    }
}

impl<'a> Draw<RequestPaneProps<'a>> for RequestPane {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: RequestPaneProps<'a>,
        area: Rect,
    ) {
        // Render outermost block
        let pane_kind = PrimaryPane::Request;
        let block = Pane {
            title: &pane_kind.to_string(),
            is_focused: props.is_selected,
        };
        let block = block.generate();
        let inner_area = block.inner(area);
        context.frame.render_widget(block, area);

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
            let preview_templates = context.config.preview_templates;
            let recipe_state = self.recipe_state.get_or_update(
                RecipeStateKey {
                    selected_profile_id: props.selected_profile_id.cloned(),
                    recipe_id: recipe.id.clone(),
                    preview_templates: context.config.preview_templates,
                },
                || RecipeState {
                    url: TemplatePreview::new(
                        recipe.url.clone(),
                        props.selected_profile_id.cloned(),
                        preview_templates,
                    ),
                    query: to_template_previews(
                        props.selected_profile_id,
                        &recipe.query,
                        preview_templates,
                    ),
                    headers: to_template_previews(
                        props.selected_profile_id,
                        &recipe.headers,
                        preview_templates,
                    ),
                    body: recipe.body.as_ref().map(|body| {
                        TextWindow::new(TemplatePreview::new(
                            body.clone(),
                            props.selected_profile_id.cloned(),
                            preview_templates,
                        ))
                        .into()
                    }),
                },
            );

            // First line: Method + URL
            context.frame.render_widget(
                Paragraph::new(recipe.method.as_str()),
                method_area,
            );
            context.frame.render_widget(&recipe_state.url, url_area);

            // Navigation tabs
            self.tabs.draw(context, (), tabs_area);

            // Request content
            match self.tabs.selected() {
                Tab::Body => {
                    if let Some(body) = &recipe_state.body {
                        body.draw(context, (), content_area);
                    }
                }
                Tab::Query => context.frame.render_widget(
                    Table {
                        rows: recipe_state
                            .query
                            .iter()
                            .map(|(param, value)| {
                                [param.as_str().into(), value.generate()]
                            })
                            .collect_vec(),
                        header: Some(["Parameter", "Value"]),
                        alternate_row_style: true,
                        ..Default::default()
                    }
                    .generate(),
                    content_area,
                ),
                Tab::Headers => context.frame.render_widget(
                    Table {
                        rows: recipe_state
                            .headers
                            .iter()
                            .map(|(param, value)| {
                                [param.as_str().into(), value.generate()]
                            })
                            .collect_vec(),
                        header: Some(["Header", "Value"]),
                        alternate_row_style: true,
                        ..Default::default()
                    }
                    .generate(),
                    content_area,
                ),
            }
        }
    }
}

/// Convert a map of (string, template) from a recipe into (string, template
/// preview) to kick off the template preview for each value. The output should
/// be stored in state.
fn to_template_previews<'a>(
    profile_id: Option<&ProfileId>,
    iter: impl IntoIterator<Item = (&'a String, &'a Template)>,
    preview_templates: bool,
) -> Vec<(String, TemplatePreview)> {
    iter.into_iter()
        .map(|(k, v)| {
            (
                k.clone(),
                TemplatePreview::new(
                    v.clone(),
                    profile_id.cloned(),
                    preview_templates,
                ),
            )
        })
        .collect()
}
