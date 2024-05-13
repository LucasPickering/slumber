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
            component::{
                primary::PrimaryPane,
                record_body::{RecordBody, RecordBodyProps},
            },
            draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
            event::{Event, EventHandler, EventQueue, Update},
            state::{persistence::PersistentKey, RequestState, StateCell},
            Component,
        },
    },
};
use derive_more::{Debug, Display};
use ratatui::{
    layout::Layout,
    prelude::{Alignment, Constraint},
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
    fn draw(
        &self,
        frame: &mut Frame,
        props: RequestPaneProps<'a>,
        metadata: DrawMetadata,
    ) {
        // Render outermost block
        let title = TuiContext::get()
            .input_engine
            .add_hint("Request", Action::SelectRequest);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        };
        let block = block.generate();
        let inner_area = block.inner(metadata.area());
        frame.render_widget(block, metadata.area());
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
                    true,
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
        if let Some(action) = event.action() {
            match action {
                Action::LeftClick => {
                    EventQueue::push(Event::new_other(PrimaryPane::Request));
                }
                Action::OpenActions => {
                    EventQueue::open_modal_default::<ActionsModal<MenuAction>>()
                }
                _ => return Update::Propagate(event),
            }
        } else if let Some(menu_action) = event.other::<MenuAction>() {
            match menu_action {
                MenuAction::CopyUrl => {
                    if let Some(state) = self.state.get() {
                        messages_tx.send(Message::CopyText(
                            state.request.url.to_string(),
                        ))
                    }
                }
                MenuAction::CopyBody => {
                    // Copy exactly what the user sees. Currently requests
                    // don't support formatting/querying but that could change
                    if let Some(body) = self
                        .state
                        .get()
                        .and_then(|state| state.body.data().text())
                    {
                        messages_tx.send(Message::CopyText(body));
                    }
                }
            }
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        [
            self.state.get_mut().map(|state| state.body.as_child()),
            // Tabs goes last, because pane content gets priority
            Some(self.tabs.as_child()),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

impl Draw<RenderedRequestProps> for RenderedRequest {
    fn draw(
        &self,
        frame: &mut Frame,
        props: RenderedRequestProps,
        metadata: DrawMetadata,
    ) {
        let state = self.state.get_or_update(props.request.id, || State {
            request: Arc::clone(&props.request),
            body: Default::default(),
        });

        // Split the main area again to allow tabs
        let [tabs_area, content_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(metadata.area());

        // Navigation tabs
        self.tabs.draw(frame, (), tabs_area, true);

        // Main content for the response
        match self.tabs.data().selected() {
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
                        true,
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
