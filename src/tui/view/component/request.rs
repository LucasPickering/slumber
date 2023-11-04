use crate::{
    config::{ProfileId, RequestRecipe, RequestRecipeId},
    template::TemplateString,
    tui::{
        input::Action,
        view::{
            component::{
                primary::PrimaryPane,
                root::FullscreenMode,
                table::{Table, TableProps},
                tabs::Tabs,
                template_preview::TemplatePreview,
                text_window::TextWindow,
                Component, Draw, Event, Update, UpdateContext,
            },
            state::{FixedSelect, StateCell},
            util::{layout, BlockBrick, ToTui},
            DrawContext,
        },
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Constraint, Direction, Rect},
    text::Text,
    widgets::Paragraph,
};
use strum::EnumIter;

/// Display a request recipe
#[derive(Debug, Display, Default)]
#[display(fmt = "RequestPane")]
pub struct RequestPane {
    tabs: Tabs<Tab>,
    /// All UI state derived from the recipe is stored together, and reset when
    /// the recipe or profile changes
    recipe_state: StateCell<(Option<ProfileId>, RequestRecipeId), RecipeState>,
}

pub struct RequestPaneProps<'a> {
    pub is_selected: bool,
    pub selected_recipe: Option<&'a RequestRecipe>,
    pub selected_profile_id: Option<&'a ProfileId>,
}

#[derive(Debug)]
struct RecipeState {
    url: TemplatePreview,
    query: Vec<(String, TemplatePreview)>,
    headers: Vec<(String, TemplatePreview)>,
    body: Option<TextWindow<TemplatePreview>>,
}

#[derive(Copy, Clone, Debug, Default, Display, EnumIter, PartialEq)]
enum Tab {
    #[default]
    Body,
    Query,
    Headers,
}

impl FixedSelect for Tab {}

impl Component for RequestPane {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Toggle fullscreen
            Event::Input {
                action: Some(Action::Fullscreen),
                ..
            } => {
                context.queue_event(Event::ToggleFullscreen(
                    FullscreenMode::Request,
                ));
                Update::Consumed
            }

            _ => Update::Propagate(event),
        }
    }

    fn children(&mut self) -> Vec<&mut dyn Component> {
        let mut children: Vec<&mut dyn Component> = vec![&mut self.tabs];
        // If the body is initialized and present, send events there too
        if let Some(body) = self
            .recipe_state
            .get_mut()
            .and_then(|state| state.body.as_mut())
        {
            children.push(body);
        }
        children
    }
}

impl<'a> Draw<RequestPaneProps<'a>> for RequestPane {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: RequestPaneProps<'a>,
        chunk: Rect,
    ) {
        // Render outermost block
        let pane_kind = PrimaryPane::Request;
        let block = BlockBrick {
            title: pane_kind.to_string(),
            is_focused: props.is_selected,
        };
        let block = block.to_tui(context);
        let inner_chunk = block.inner(chunk);
        context.frame.render_widget(block, chunk);

        // Render request contents
        if let Some(recipe) = props.selected_recipe {
            let [metadata_chunk, tabs_chunk, content_chunk] = layout(
                inner_chunk,
                Direction::Vertical,
                [
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ],
            );

            let [method_chunk, url_chunk] = layout(
                metadata_chunk,
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
                (props.selected_profile_id.cloned(), recipe.id.clone()),
                || RecipeState {
                    url: TemplatePreview::new(
                        context,
                        recipe.url.clone(),
                        props.selected_profile_id.cloned(),
                    ),
                    query: to_template_previews(
                        context,
                        props.selected_profile_id,
                        &recipe.query,
                    ),
                    headers: to_template_previews(
                        context,
                        props.selected_profile_id,
                        &recipe.headers,
                    ),
                    body: recipe.body.as_ref().map(|body| {
                        TextWindow::new(TemplatePreview::new(
                            context,
                            body.clone(),
                            props.selected_profile_id.cloned(),
                        ))
                    }),
                },
            );

            // First line: Method + URL
            context.frame.render_widget(
                Paragraph::new(recipe.method.as_str()),
                method_chunk,
            );
            recipe_state.url.draw(context, (), url_chunk);

            // Navigation tabs
            self.tabs.draw(context, (), tabs_chunk);

            // Request content
            match self.tabs.selected() {
                Tab::Body => {
                    if let Some(body) = &recipe_state.body {
                        body.draw(context, (), content_chunk);
                    }
                }
                Tab::Query => Table.draw(
                    context,
                    TableProps {
                        key_label: "Parameter",
                        value_label: "Value",
                        data: to_table_text(context, &recipe_state.query),
                    },
                    content_chunk,
                ),
                Tab::Headers => Table.draw(
                    context,
                    TableProps {
                        key_label: "Header",
                        value_label: "Value",
                        data: to_table_text(context, &recipe_state.headers),
                    },
                    content_chunk,
                ),
            }
        }
    }
}

/// Convert a map of (string, template) from a recipe into (string, template
/// preview) to kick off the template preview for each value. The output should
/// be stored in state.
fn to_template_previews<'a>(
    context: &DrawContext,
    profile_id: Option<&ProfileId>,
    iter: impl IntoIterator<Item = (&'a String, &'a TemplateString)>,
) -> Vec<(String, TemplatePreview)> {
    iter.into_iter()
        .map(|(k, v)| {
            (
                k.clone(),
                TemplatePreview::new(context, v.clone(), profile_id.cloned()),
            )
        })
        .collect()
}

/// Convert a map of (string, template preview) to (text, text) so it can be
/// displayed in a table.
fn to_table_text<'a>(
    context: &DrawContext,
    iter: impl IntoIterator<Item = &'a (String, TemplatePreview)>,
) -> Vec<(Text<'a>, Text<'a>)> {
    iter.into_iter()
        .map(|(param, value)| (param.as_str().into(), value.to_tui(context)))
        // Collect required to drop reference to context
        .collect_vec()
}
