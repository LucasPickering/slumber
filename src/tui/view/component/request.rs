use crate::{
    config::RequestRecipe,
    tui::{
        input::Action,
        view::{
            component::{
                primary::PrimaryPane, root::FullscreenMode, Component, Draw,
                Event, UpdateContext, UpdateOutcome,
            },
            state::{FixedSelect, StatefulSelect},
            util::{layout, BlockBrick, TabBrick, ToTui},
            Frame, RenderContext,
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
    tabs: StatefulSelect<RequestTab>,
}

pub struct RequestPaneProps<'a> {
    pub is_selected: bool,
    pub selected_recipe: Option<&'a RequestRecipe>,
}

#[derive(Copy, Clone, Debug, derive_more::Display, EnumIter, PartialEq)]
enum RequestTab {
    Body,
    Query,
    Headers,
}

impl FixedSelect for RequestTab {}

impl Component for RequestPane {
    fn update(
        &mut self,
        _context: &mut UpdateContext,
        event: Event,
    ) -> UpdateOutcome {
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Left => {
                    self.tabs.previous();
                    UpdateOutcome::Consumed
                }
                Action::Right => {
                    self.tabs.next();
                    UpdateOutcome::Consumed
                }

                // Enter fullscreen
                Action::Fullscreen => UpdateOutcome::Propagate(
                    Event::ToggleFullscreen(FullscreenMode::Request),
                ),

                _ => UpdateOutcome::Propagate(event),
            },
            _ => UpdateOutcome::Propagate(event),
        }
    }
}

impl<'a> Draw<RequestPaneProps<'a>> for RequestPane {
    fn draw(
        &self,
        context: &RenderContext,
        props: RequestPaneProps<'a>,
        frame: &mut Frame,
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
        frame.render_widget(block, chunk);

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
            frame.render_widget(
                Paragraph::new(format!("{} {}", recipe.method, recipe.url)),
                url_chunk,
            );

            // Navigation tabs
            let tabs = TabBrick { tabs: &self.tabs };
            frame.render_widget(tabs.to_tui(context), tabs_chunk);

            // Request content
            match self.tabs.selected() {
                RequestTab::Body => {
                    if let Some(body) = recipe.body.as_deref() {
                        frame
                            .render_widget(Paragraph::new(body), content_chunk);
                    }
                }
                RequestTab::Query => {
                    frame.render_widget(
                        Paragraph::new(recipe.query.to_tui(context)),
                        content_chunk,
                    );
                }
                RequestTab::Headers => {
                    frame.render_widget(
                        Paragraph::new(recipe.headers.to_tui(context)),
                        content_chunk,
                    );
                }
            }
        }
    }
}
