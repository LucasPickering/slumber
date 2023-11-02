use crate::{
    config::RequestRecipe,
    tui::{
        input::Action,
        view::{
            component::{
                primary::PrimaryPane, root::FullscreenMode, tabs::Tabs,
                Component, Draw, Event, UpdateContext, UpdateOutcome,
            },
            state::FixedSelect,
            util::{layout, BlockBrick, ToTui},
            DrawContext,
        },
    },
};
use derive_more::Display;
use ratatui::{
    prelude::{Constraint, Direction, Rect},
    widgets::Paragraph,
};
use strum::EnumIter;

/// Display a request recipe
#[derive(Debug, Display, Default)]
#[display(fmt = "RequestPane")]
pub struct RequestPane {
    tabs: Tabs<Tab>,
}

pub struct RequestPaneProps<'a> {
    pub is_selected: bool,
    pub selected_recipe: Option<&'a RequestRecipe>,
}

#[derive(
    Copy, Clone, Debug, Default, derive_more::Display, EnumIter, PartialEq,
)]
enum Tab {
    #[default]
    Body,
    Query,
    Headers,
}

impl FixedSelect for Tab {}

impl Component for RequestPane {
    fn update(
        &mut self,
        _context: &mut UpdateContext,
        event: Event,
    ) -> UpdateOutcome {
        match event {
            Event::Input {
                action: Some(Action::Fullscreen),
                ..
            } => UpdateOutcome::Propagate(Event::ToggleFullscreen(
                FullscreenMode::Request,
            )),
            _ => UpdateOutcome::Propagate(event),
        }
    }

    fn focused_child(&mut self) -> Option<&mut dyn Component> {
        Some(&mut self.tabs)
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
            let [url_chunk, tabs_chunk, content_chunk] = layout(
                inner_chunk,
                Direction::Vertical,
                [
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ],
            );

            // URL
            context.frame.render_widget(
                Paragraph::new(format!("{} {}", recipe.method, recipe.url)),
                url_chunk,
            );

            // Navigation tabs
            self.tabs.draw(context, (), tabs_chunk);

            // Request content
            match self.tabs.selected() {
                Tab::Body => {
                    if let Some(body) = recipe.body.as_deref() {
                        context
                            .frame
                            .render_widget(Paragraph::new(body), content_chunk);
                    }
                }
                Tab::Query => {
                    context.frame.render_widget(
                        Paragraph::new(recipe.query.to_tui(context)),
                        content_chunk,
                    );
                }
                Tab::Headers => {
                    context.frame.render_widget(
                        Paragraph::new(recipe.headers.to_tui(context)),
                        content_chunk,
                    );
                }
            }
        }
    }
}
