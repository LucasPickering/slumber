use crate::{
    http::{Request, RequestId},
    tui::{
        context::TuiContext,
        input::Action,
        message::{Message, MessageSender},
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
use derive_more::{Debug, Display};
use ratatui::{
    layout::Layout,
    prelude::{Alignment, Constraint, Rect},
    widgets::{Paragraph, Wrap},
    Frame,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display HTTP request state, which could be in progress, complete, or
/// failed. This can be used in both a paned and fullscreen view.
#[derive(Debug, Default)]
pub struct RequestPane {
    content: Component<RenderedRequest>,
}

pub struct RequestPaneProps<'a> {
    pub is_selected: bool,
    pub active_request: Option<&'a RequestState>,
}

/// Items in the actions popup menu
#[derive(Copy, Clone, Debug, Display, EnumCount, EnumIter, PartialEq)]
enum MenuAction {
    #[display("Copy URL")]
    CopyUrl,
    #[display("Copy Body")]
    CopyBody,
}

impl ToStringGenerate for MenuAction {}

impl EventHandler for RequestPane {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.content.as_child()]
    }
}

impl<'a> Draw<RequestPaneProps<'a>> for RequestPane {
    fn draw(&self, frame: &mut Frame, props: RequestPaneProps<'a>, area: Rect) {
        // Render outermost block
        let title = TuiContext::get()
            .input_engine
            .add_hint("Request", Action::SelectRequest);
        let block = Pane {
            title: &title,
            is_focused: props.is_selected,
        };
        let block = block.generate();
        let inner_area = block.inner(area);
        frame.render_widget(block, area);
        let area = inner_area; // Shadow to make sure we use the right area

        // Don't render anything else unless we have a request state
        if let Some(request_state) = props.active_request {
            // Time goes in the top-right,
            let [time_area, _] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                    .areas(inner_area);

            // Request metadata
            if let Some(metadata) = request_state.metadata() {
                frame.render_widget(
                    Paragraph::new(metadata.start_time.generate())
                        .alignment(Alignment::Right),
                    time_area,
                );
            }

            // 3/5 branches render the request, so we want to de-dupe that code
            // a bit while rendering something else for the other states
            let request: Option<&Arc<Request>> = match &request_state {
                RequestState::Building { .. } => {
                    frame.render_widget(
                        Paragraph::new("Initializing request..."),
                        area,
                    );
                    None
                }

                // :(
                RequestState::BuildError { error } => {
                    frame.render_widget(
                        Paragraph::new(error.generate()).wrap(Wrap::default()),
                        area,
                    );
                    None
                }
                RequestState::Loading { request, .. } => Some(request),
                RequestState::Response { record, .. } => Some(&record.request),
                RequestState::RequestError { error } => Some(&error.request),
            };

            if let Some(request) = request {
                self.content.draw(
                    frame,
                    RenderedRequestProps {
                        request: Arc::clone(request),
                    },
                    area,
                )
            }
        }
    }
}

/// Content once a request has successfully been rendered/sent
#[derive(Debug)]
struct RenderedRequest {
    #[debug(skip)]
    tabs: Component<Tabs<Tab>>,
    #[debug(skip)]
    state: StateCell<RequestId, State>,
}

/// Inner state, which should be reset when request changes
struct State {
    /// Store pointer to the request, so we can access it in the update step
    request: Arc<Request>,
    /// Persist the request body to track view state. Update whenever the
    /// loaded request changes
    body: Component<RecordBody>,
}

impl Default for RenderedRequest {
    fn default() -> Self {
        Self {
            tabs: Tabs::new(PersistentKey::RequestTab).into(),
            state: Default::default(),
        }
    }
}

struct RenderedRequestProps {
    request: Arc<Request>,
}

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Display,
    EnumCount,
    EnumIter,
    PartialEq,
    Serialize,
    Deserialize,
)]
enum Tab {
    #[default]
    #[display("URL")]
    Url,
    Body,
    Headers,
}

impl EventHandler for RenderedRequest {
    fn update(&mut self, messages_tx: &MessageSender, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(Action::OpenActions),
                ..
            } => EventQueue::open_modal_default::<ActionsModal<MenuAction>>(),
            Event::Other(ref other) => {
                // Check for an action menu event
                match other.downcast_ref::<MenuAction>() {
                    Some(MenuAction::CopyUrl) => {
                        if let Some(state) = self.state.get() {
                            messages_tx.send(Message::CopyText(
                                state.request.url.to_string(),
                            ))
                        }
                    }
                    Some(MenuAction::CopyBody) => {
                        // Copy exactly what the user sees. Currently requests
                        // don't support formatting/querying but that could
                        // change
                        if let Some(body) =
                            self.state.get().and_then(|state| state.body.text())
                        {
                            messages_tx.send(Message::CopyText(body));
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
            Tab::Url | Tab::Headers => {}
            Tab::Body => {
                if let Some(state) = self.state.get_mut() {
                    children.push(state.body.as_child());
                }
            }
        }
        // Tabs goes last, because pane content gets priority
        children.push(self.tabs.as_child());
        children
    }
}

impl Draw<RenderedRequestProps> for RenderedRequest {
    fn draw(&self, frame: &mut Frame, props: RenderedRequestProps, area: Rect) {
        let state = self.state.get_or_update(props.request.id, || State {
            request: Arc::clone(&props.request),
            body: Default::default(),
        });

        // Split the main area again to allow tabs
        let [tabs_area, content_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(area);

        // Navigation tabs
        self.tabs.draw(frame, (), tabs_area);

        // Main content for the response
        match self.tabs.selected() {
            Tab::Url => {
                frame.render_widget(
                    Paragraph::new(props.request.url.to_string())
                        .wrap(Wrap::default()),
                    content_area,
                );
            }
            Tab::Body => {
                if let Some(body) = &props.request.body {
                    state.body.draw(
                        frame,
                        RecordBodyProps { body },
                        content_area,
                    );
                }
            }
            Tab::Headers => frame.render_widget(
                HeaderTable {
                    headers: &props.request.headers,
                }
                .generate(),
                content_area,
            ),
        }
    }
}
