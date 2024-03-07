mod body;

use crate::{
    http::{RequestRecord, ResponseContent},
    tui::{
        input::Action,
        view::{
            common::{actions::ActionsModal, table::Table, tabs::Tabs, Pane},
            component::{
                primary::PrimaryPane,
                response::body::{
                    ResponseContentBody, ResponseContentBodyProps,
                },
            },
            draw::{Draw, Generate, ToStringGenerate},
            event::{Event, EventHandler, Update, UpdateContext},
            state::{persistence::PersistentKey, RequestState},
            util::layout,
            Component,
        },
    },
};
use derive_more::{Debug, Display};
use itertools::Itertools;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
    Frame,
};
use serde::{Deserialize, Serialize};
use strum::{EnumCount, EnumIter};

/// Display HTTP response state, which could be in progress, complete, or
/// failed. This can be used in both a paned and fullscreen view.
#[derive(Debug, Default)]
pub struct ResponsePane {
    content: Component<CompleteResponseContent>,
}

pub struct ResponsePaneProps<'a> {
    pub is_selected: bool,
    pub active_request: Option<&'a RequestState>,
}

/// Items in the actions popup menu
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
enum MenuAction {
    #[display("Copy Body")]
    CopyBody,
}

impl ToStringGenerate for MenuAction {}

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
                    parsed_body,
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
                            parsed_body: parsed_body.as_deref(),
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
struct CompleteResponseContent {
    #[debug(skip)]
    tabs: Component<Tabs<Tab>>,
    /// Persist the response body to track view state. Update whenever the
    /// loaded request changes
    #[debug(skip)]
    body: Component<ResponseContentBody>,
}

impl Default for CompleteResponseContent {
    fn default() -> Self {
        Self {
            tabs: Tabs::new(PersistentKey::ResponseTab).into(),
            body: Default::default(),
        }
    }
}

struct ResponseContentProps<'a> {
    record: &'a RequestRecord,
    parsed_body: Option<&'a dyn ResponseContent>,
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
    Headers,
}

impl EventHandler for CompleteResponseContent {
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(Action::OpenActions),
                ..
            } => context.open_modal_default::<ActionsModal<MenuAction>>(),
            Event::Other(ref other) => {
                // Check for an action menu event
                match other.downcast_ref::<MenuAction>() {
                    Some(MenuAction::CopyBody) => {
                        if let Some(body) = self.body.text() {
                            context.copy_text(body);
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
        let mut children = vec![];
        match selected_tab {
            Tab::Body => {
                children.push(self.body.as_child());
            }
            Tab::Headers => {}
        }
        // Tabs goes last, because pane content gets priority
        children.push(self.tabs.as_child());
        children
    }
}

impl<'a> Draw<ResponseContentProps<'a>> for CompleteResponseContent {
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
                self.body.draw(
                    frame,
                    ResponseContentBodyProps {
                        record: props.record,
                        parsed_body: props.parsed_body,
                    },
                    content_area,
                );
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
