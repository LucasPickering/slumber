use crate::tui::{
    input::Action,
    view::{
        component::{
            primary::PrimaryPane, root::FullscreenMode, Component, Draw, Event,
            UpdateContext, UpdateOutcome,
        },
        state::{FixedSelect, RequestState, StatefulSelect},
        util::{layout, BlockBrick, TabBrick, ToTui},
        DrawContext,
    },
};
use derive_more::Display;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::Line,
    widgets::{Paragraph, Wrap},
};
use strum::EnumIter;

/// Display HTTP response state, which could be in progress, complete, or
/// failed. This can be used in both a paned and fullscreen view.
#[derive(Debug, Default, Display)]
#[display(fmt = "ResponsePane")]
pub struct ResponsePane {
    tabs: StatefulSelect<ResponseTab>,
}

pub struct ResponsePaneProps<'a> {
    pub is_selected: bool,
    pub active_request: Option<&'a RequestState>,
}

#[derive(Copy, Clone, Debug, derive_more::Display, EnumIter, PartialEq)]
enum ResponseTab {
    Body,
    Headers,
}

impl FixedSelect for ResponseTab {}

impl Component for ResponsePane {
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
                // Switch tabs
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
                    Event::ToggleFullscreen(FullscreenMode::Response),
                ),

                _ => UpdateOutcome::Propagate(event),
            },
            _ => UpdateOutcome::Propagate(event),
        }
    }
}

impl<'a> Draw<ResponsePaneProps<'a>> for ResponsePane {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: ResponsePaneProps<'a>,

        chunk: Rect,
    ) {
        // Render outermost block
        let pane_kind = PrimaryPane::Response;
        let block = BlockBrick {
            title: pane_kind.to_string(),
            is_focused: props.is_selected,
        };
        let block = block.to_tui(context);
        let inner_chunk = block.inner(chunk);
        context.frame.render_widget(block, chunk);

        // Don't render anything else unless we have a request state
        if let Some(request_state) = props.active_request {
            let [header_chunk, content_chunk] = layout(
                inner_chunk,
                Direction::Vertical,
                [Constraint::Length(1), Constraint::Min(0)],
            );
            let [header_left_chunk, header_right_chunk] = layout(
                header_chunk,
                Direction::Horizontal,
                [Constraint::Length(20), Constraint::Min(0)],
            );

            // Time-related data. start_time and duration should always be
            // defined together
            if let (Some(start_time), Some(duration)) =
                (request_state.start_time(), request_state.duration())
            {
                context.frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        start_time.to_tui(context),
                        " / ".into(),
                        duration.to_tui(context),
                    ]))
                    .alignment(Alignment::Right),
                    header_right_chunk,
                );
            }

            match &request_state {
                RequestState::Building { .. } => {
                    context.frame.render_widget(
                        Paragraph::new("Initializing request..."),
                        header_left_chunk,
                    );
                }

                // :(
                RequestState::BuildError { error } => {
                    context.frame.render_widget(
                        Paragraph::new(error.to_tui(context))
                            .wrap(Wrap::default()),
                        content_chunk,
                    );
                }

                RequestState::Loading { .. } => {
                    context.frame.render_widget(
                        Paragraph::new("Loading..."),
                        header_left_chunk,
                    );
                }

                RequestState::Response {
                    record,
                    pretty_body,
                } => {
                    let response = &record.response;
                    // Status code
                    context.frame.render_widget(
                        Paragraph::new(response.status.to_string()),
                        header_left_chunk,
                    );

                    // Split the main chunk again to allow tabs
                    let [tabs_chunk, content_chunk] = layout(
                        content_chunk,
                        Direction::Vertical,
                        [Constraint::Length(1), Constraint::Min(0)],
                    );

                    // Navigation tabs
                    let tabs = TabBrick { tabs: &self.tabs };
                    context
                        .frame
                        .render_widget(tabs.to_tui(context), tabs_chunk);

                    // Main content for the response
                    match self.tabs.selected() {
                        ResponseTab::Body => {
                            let body = pretty_body
                                .as_deref()
                                .unwrap_or(response.body.text());
                            context.frame.render_widget(
                                Paragraph::new(body),
                                content_chunk,
                            );
                        }
                        ResponseTab::Headers => {
                            context.frame.render_widget(
                                Paragraph::new(
                                    response.headers.to_tui(context),
                                ),
                                content_chunk,
                            );
                        }
                    };
                }

                // Sadge
                RequestState::RequestError { error } => {
                    context.frame.render_widget(
                        Paragraph::new(error.to_tui(context))
                            .wrap(Wrap::default()),
                        content_chunk,
                    );
                }
            }
        }
    }
}
