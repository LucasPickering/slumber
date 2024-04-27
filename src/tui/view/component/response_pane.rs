use crate::{
    http::{RequestId, RequestRecord},
    tui::{
        context::TuiContext,
        input::Action,
        message::Message,
        view::{
            common::{
                actions::ActionsModal, header_table::HeaderTable, tabs::Tabs,
                Pane,
            },
            component::record_body::{RecordBody, RecordBodyProps},
            draw::{Draw, Generate, ToStringGenerate},
            event::{Event, EventHandler, EventQueue, Update},
            state::{persistence::PersistentKey, RequestState, StateCell},
            Component,
        },
    },
};
use chrono::Utc;
use derive_more::{Debug, Display};
use ratatui::{
    layout::Layout,
    prelude::{Alignment, Constraint, Rect},
    text::Line,
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
        let title = TuiContext::get()
            .input_engine
            .add_hint("Response", Action::SelectResponse);
        let block = Pane {
            title: &title,
            is_focused: props.is_selected,
        };
        let block = block.generate();
        frame.render_widget(&block, area);
        let area = block.inner(area);

        match props.active_request {
            None | Some(RequestState::BuildError { .. }) => {}
            Some(RequestState::Building { .. }) => {
                frame.render_widget(Paragraph::new("Loading..."), area)
            }
            Some(RequestState::Loading { start_time, .. }) => {
                frame.render_widget(Paragraph::new("Loading..."), area);
                let duration = Utc::now() - start_time;
                frame.render_widget(
                    Paragraph::new(duration.generate())
                        .alignment(Alignment::Right),
                    area,
                );
            }

            Some(RequestState::Response { record }) => self.content.draw(
                frame,
                CompleteResponseContentProps { record },
                area,
            ),

            // Sadge
            Some(RequestState::RequestError { error }) => frame.render_widget(
                Paragraph::new(error.generate()).wrap(Wrap::default()),
                area,
            ),
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
    body: StateCell<RequestId, Component<RecordBody>>,
}

impl Default for CompleteResponseContent {
    fn default() -> Self {
        Self {
            tabs: Tabs::new(PersistentKey::ResponseTab).into(),
            body: Default::default(),
        }
    }
}

struct CompleteResponseContentProps<'a> {
    record: &'a RequestRecord,
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
    fn update(&mut self, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(Action::OpenActions),
                ..
            } => EventQueue::open_modal_default::<ActionsModal<MenuAction>>(),
            Event::Other(ref other) => {
                // Check for an action menu event
                match other.downcast_ref::<MenuAction>() {
                    Some(MenuAction::CopyBody) => {
                        // We need to generate the copy text here because it can
                        // be formatted/queried
                        if let Some(body) =
                            self.body.get().and_then(|body| body.text())
                        {
                            TuiContext::send_message(Message::CopyText(body));
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
                if let Some(body) = self.body.get_mut() {
                    children.push(body.as_child());
                }
            }
            Tab::Headers => {}
        }
        // Tabs goes last, because pane content gets priority
        children.push(self.tabs.as_child());
        children
    }
}

impl<'a> Draw<CompleteResponseContentProps<'a>> for CompleteResponseContent {
    fn draw(
        &self,
        frame: &mut Frame,
        props: CompleteResponseContentProps<'a>,
        area: Rect,
    ) {
        let response = &props.record.response;

        // Split the main area again to allow tabs
        let [header_area, tabs_area, content_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);

        // Metadata
        frame.render_widget(
            Paragraph::new(response.status.to_string()),
            header_area,
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                props.record.response.body.size().to_string_as(false).into(),
                " / ".into(),
                props.record.duration().generate(),
            ]))
            .alignment(Alignment::Right),
            header_area,
        );

        // Navigation tabs
        self.tabs.draw(frame, (), tabs_area);

        // Main content for the response
        match self.tabs.selected() {
            Tab::Body => {
                let body =
                    self.body.get_or_update(props.record.id, Default::default);
                body.draw(
                    frame,
                    RecordBodyProps {
                        body: &response.body,
                    },
                    content_area,
                );
            }

            Tab::Headers => frame.render_widget(
                HeaderTable {
                    headers: &response.headers,
                }
                .generate(),
                content_area,
            ),
        }
    }
}
