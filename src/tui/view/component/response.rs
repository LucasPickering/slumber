use crate::{
    http::{RequestId, RequestRecord},
    tui::{
        input::Action,
        view::{
            common::{
                actions::ActionsModal, table::Table, tabs::Tabs,
                text_window::TextWindow, Pane,
            },
            component::primary::PrimaryPane,
            draw::{Draw, Generate, ToStringGenerate},
            event::{Event, EventHandler, Update, UpdateContext},
            state::{persistence::PersistentKey, RequestState, StateCell},
            util::layout,
            Component,
        },
    },
};
use derive_more::Display;
use itertools::Itertools;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
    Frame,
};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use strum::EnumIter;

/// Display HTTP response state, which could be in progress, complete, or
/// failed. This can be used in both a paned and fullscreen view.
#[derive(Debug, Default)]
pub struct ResponsePane {
    content: Component<ResponseContent>,
}

pub struct ResponsePaneProps<'a> {
    pub is_selected: bool,
    pub active_request: Option<&'a RequestState>,
}

/// Items in the actions popup menu
#[derive(Copy, Clone, Debug, Default, Display, EnumIter, PartialEq)]
enum MenuAction {
    #[default]
    #[display("Copy Body")]
    CopyBody,
}

impl EventHandler for ResponsePane {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.content.as_child()]
    }
}

impl<'a> Draw<ResponsePaneProps<'a>> for ResponsePane {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ResponsePaneProps<'a>,
        area: Rect,
    ) {
        // Render outermost block
        let pane_kind = PrimaryPane::Response;
        let block = Pane {
            title: &pane_kind.to_string(),
            is_focused: props.is_selected,
        };
        let block = block.generate();
        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // Don't render anything else unless we have a request state
        if let Some(request_state) = props.active_request {
            let [header_area, content_area] = layout(
                inner_area,
                Direction::Vertical,
                [Constraint::Length(1), Constraint::Min(0)],
            );
            let [header_left_area, header_right_area] = layout(
                header_area,
                Direction::Horizontal,
                // The longest canonical status reason in reqwest is 31 chars
                [Constraint::Length(3 + 1 + 31), Constraint::Min(0)],
            );

            // Time-related data. start_time and duration should always be
            // defined together
            if let (Some(start_time), Some(duration)) =
                (request_state.start_time(), request_state.duration())
            {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        start_time.generate(),
                        " / ".into(),
                        duration.generate(),
                    ]))
                    .alignment(Alignment::Right),
                    header_right_area,
                );
            }

            match &request_state {
                RequestState::Building { .. } => {
                    frame.render_widget(
                        Paragraph::new("Initializing request..."),
                        header_left_area,
                    );
                }

                // :(
                RequestState::BuildError { error } => {
                    frame.render_widget(
                        Paragraph::new(error.generate()).wrap(Wrap::default()),
                        content_area,
                    );
                }

                RequestState::Loading { .. } => {
                    frame.render_widget(
                        Paragraph::new("Loading..."),
                        header_left_area,
                    );
                }

                RequestState::Response {
                    record,
                    pretty_body,
                } => {
                    let response = &record.response;
                    // Status code
                    frame.render_widget(
                        Paragraph::new(response.status.to_string()),
                        header_left_area,
                    );

                    self.content.draw(
                        frame,
                        ResponseContentProps {
                            record,
                            pretty_body: pretty_body.as_deref(),
                        },
                        content_area,
                    );
                }

                // Sadge
                RequestState::RequestError { error } => {
                    frame.render_widget(
                        Paragraph::new(error.generate()).wrap(Wrap::default()),
                        content_area,
                    );
                }
            }
        }
    }
}

/// Display response success state (tab container)
#[derive(Debug)]
struct ResponseContent {
    tabs: Component<Tabs<Tab>>,
    /// Persist the response body to track view state. Update whenever the
    /// loaded request changes
    body: StateCell<RequestId, Component<TextWindow<String>>>,
}

impl Default for ResponseContent {
    fn default() -> Self {
        Self {
            tabs: Tabs::new(PersistentKey::ResponseTab).into(),
            body: Default::default(),
        }
    }
}

struct ResponseContentProps<'a> {
    record: &'a RequestRecord,
    pretty_body: Option<&'a str>,
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
    Headers,
}

impl EventHandler for ResponseContent {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match &event {
            Event::Input {
                action: Some(Action::OpenActions),
                ..
            } => context.open_modal_default::<ActionsModal<MenuAction>>(),
            Event::Other(callback) => {
                match callback.downcast_ref::<MenuAction>() {
                    Some(MenuAction::CopyBody) => {
                        if let Some(body) = self.body.get() {
                            context.copy_text(body.inner().text().to_owned())
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
        match selected_tab {
            Tab::Body => {
                if let Some(body) = self.body.get_mut() {
                    children.push(body.as_child());
                }
            }
            Tab::Headers => {}
        }
        children
    }
}

impl<'a> Draw<ResponseContentProps<'a>> for ResponseContent {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ResponseContentProps<'a>,
        area: Rect,
    ) {
        let response = &props.record.response;

        // Split the main area again to allow tabs
        let [tabs_area, content_area] = layout(
            area,
            Direction::Vertical,
            [Constraint::Length(1), Constraint::Min(0)],
        );

        // Navigation tabs
        self.tabs.draw(frame, (), tabs_area);

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
                    TextWindow::new(body).into()
                });
                body.deref().draw(frame, (), content_area)
            }
            Tab::Headers => frame.render_widget(
                Table {
                    rows: response
                        .headers
                        .iter()
                        .map(|(k, v)| {
                            [Text::from(k.as_str()), v.generate().into()]
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

impl ToStringGenerate for MenuAction {}
