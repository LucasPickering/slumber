use crate::{
    http::{RequestId, RequestRecord},
    tui::{
        input::Action,
        view::{
            component::{
                primary::PrimaryPane,
                root::FullscreenMode,
                table::{Table, TableProps},
                tabs::Tabs,
                text_window::TextWindow,
                Component, Draw, Event, Update, UpdateContext,
            },
            state::{FixedSelect, RequestState, StateCell},
            util::{layout, BlockBrick, ToTui},
            DrawContext,
        },
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::Line,
    widgets::{Paragraph, Wrap},
};
use std::ops::Deref;
use strum::EnumIter;

/// Display HTTP response state, which could be in progress, complete, or
/// failed. This can be used in both a paned and fullscreen view.
#[derive(Debug, Default, Display)]
#[display(fmt = "ResponsePane")]
pub struct ResponsePane {
    content: ResponseContent,
}

pub struct ResponsePaneProps<'a> {
    pub is_selected: bool,
    pub active_request: Option<&'a RequestState>,
}

#[derive(Copy, Clone, Debug, Default, Display, EnumIter, PartialEq)]
enum Tab {
    #[default]
    Body,
    Headers,
}

impl FixedSelect for Tab {}

impl Component for ResponsePane {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Toggle fullscreen
            Event::Input {
                action: Some(Action::Fullscreen),
                ..
            } => {
                context.queue_event(Event::ToggleFullscreen(
                    FullscreenMode::Response,
                ));
                Update::Consumed
            }

            _ => Update::Propagate(event),
        }
    }

    fn children(&mut self) -> Vec<&mut dyn Component> {
        vec![&mut self.content]
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
                // The longest canonical status reason in reqwest is 31 chars
                [Constraint::Length(3 + 1 + 31), Constraint::Min(0)],
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

                    self.content.draw(
                        context,
                        ResponseContentProps {
                            record,
                            pretty_body: pretty_body.as_deref(),
                        },
                        content_chunk,
                    );
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

/// Display response success state (tab container)
#[derive(Debug, Default, Display)]
#[display(fmt = "ResponsePane")]
struct ResponseContent {
    tabs: Tabs<Tab>,
    /// Persist the response body to track view state. Update whenever the
    /// loaded request changes
    body: StateCell<RequestId, TextWindow<String>>,
}

struct ResponseContentProps<'a> {
    record: &'a RequestRecord,
    pretty_body: Option<&'a str>,
}

impl Component for ResponseContent {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Toggle fullscreen
            Event::Input {
                action: Some(Action::Fullscreen),
                ..
            } => {
                context.queue_event(Event::ToggleFullscreen(
                    FullscreenMode::Response,
                ));
                Update::Consumed
            }

            _ => Update::Propagate(event),
        }
    }

    fn children(&mut self) -> Vec<&mut dyn Component> {
        let mut children: Vec<&mut dyn Component> = vec![&mut self.tabs];
        if let Some(body) = self.body.get_mut() {
            children.push(body);
        }
        children
    }
}

impl<'a> Draw<ResponseContentProps<'a>> for ResponseContent {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: ResponseContentProps<'a>,
        chunk: Rect,
    ) {
        let response = &props.record.response;

        // Split the main chunk again to allow tabs
        let [tabs_chunk, content_chunk] = layout(
            chunk,
            Direction::Vertical,
            [Constraint::Length(1), Constraint::Min(0)],
        );

        // Navigation tabs
        self.tabs.draw(context, (), tabs_chunk);

        // Main content for the response
        match self.tabs.selected() {
            Tab::Body => {
                let body = self.body.get_or_update(props.record.id, || {
                    // Use the pretty body if available. If not,
                    // fall back to the ugly one
                    let body = props
                        .pretty_body
                        .unwrap_or_else(|| response.body.text())
                        .to_owned();
                    TextWindow::new(body)
                });
                body.deref().draw(context, (), content_chunk)
            }
            Tab::Headers => Table.draw(
                context,
                TableProps {
                    key_label: "Header",
                    value_label: "Value",
                    data: response
                        .headers
                        .iter()
                        .map(|(k, v)| {
                            (k.as_str().into(), v.to_tui(context).into())
                        })
                        // Collect required to close context ref
                        .collect_vec(),
                },
                content_chunk,
            ),
        }
    }
}
