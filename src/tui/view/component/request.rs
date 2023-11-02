use crate::{
    config::RequestRecipe,
    tui::{
        input::Action,
        view::{
            component::{
                primary::PrimaryPane,
                root::FullscreenMode,
                tabs::Tabs,
                text_window::{TextWindow, TextWindowProps},
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
    text::Text,
    widgets::Paragraph,
};
use strum::EnumIter;

/// Display a request recipe
#[derive(Debug, Display, Default)]
#[display(fmt = "RequestPane")]
pub struct RequestPane {
    tabs: Tabs<Tab>,
    text_window: TextWindow,
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
            // Toggle fullscreen
            Event::Input {
                action: Some(Action::Fullscreen),
                ..
            } => UpdateOutcome::Propagate(Event::ToggleFullscreen(
                FullscreenMode::Request,
            )),

            // Reset content state when tab changes
            Event::TabChanged => {
                self.text_window.reset();
                UpdateOutcome::Consumed
            }

            _ => UpdateOutcome::Propagate(event),
        }
    }

    fn children(&mut self) -> Vec<&mut dyn Component> {
        vec![&mut self.tabs, &mut self.text_window]
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
            let text: Text = match self.tabs.selected() {
                Tab::Body => {
                    recipe.body.as_deref().map(Text::from).unwrap_or_default()
                }
                Tab::Query => recipe.query.to_tui(context),
                Tab::Headers => recipe.headers.to_tui(context),
            };
            self.text_window.draw(
                context,
                TextWindowProps { text },
                content_chunk,
            );
        }
    }
}
