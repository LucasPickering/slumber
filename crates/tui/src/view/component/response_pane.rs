use crate::{
    http::{RequestMetadata, ResponseMetadata},
    message::HttpMessage,
    util::TickLoop,
    view::{
        Generate, RequestState, ViewContext,
        common::{actions::MenuItem, fixed_select::FixedSelect, tabs::Tabs},
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            response_view::{ResponseBodyView, ResponseHeadersView},
        },
        context::UpdateContext,
        event::{DeleteTarget, Emitter, Event, EventMatch},
        persistent::PersistentKey,
        util::format_byte_size,
    },
};
use derive_more::Display;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::Style,
    text::{Line, Span, Text},
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::{collection::RecipeNodeType, http::RequestId};
use std::{error::Error, sync::Arc};
use strum::{EnumCount, EnumIter};

/// Display for a response
///
/// This is bound to a particular [RequestState]. It should be recreated
/// whenever the selected request changes state or a new request is selected.
#[derive(Debug)]
pub struct ResponsePane {
    id: ComponentId,
    state: State,
}

impl ResponsePane {
    pub fn new(
        selected_request: Option<&RequestState>,
        selected_recipe_kind: Option<RecipeNodeType>,
    ) -> Self {
        Self {
            id: Default::default(),
            state: State::new(selected_request, selected_recipe_kind),
        }
    }
}

impl Component for ResponsePane {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match &mut self.state {
            State::None | State::Folder | State::NoHistory => {
                vec![]
            }
            State::Content { content, .. } => {
                vec![content.to_child()]
            }
        }
    }
}

impl Draw for ResponsePane {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let area = metadata.area();
        match &self.state {
            // Recipe pane will show a note about how to add a recipe, so we
            // don't need anything here
            State::None => {}
            State::Folder => canvas.render_widget(
                "Select a recipe to see its request history",
                area,
            ),
            State::NoHistory => canvas.render_widget(
                "No request history for this recipe & profile",
                area,
            ),
            State::Content { metadata, content } => {
                let [metadata_area, content_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(area);

                canvas.draw(metadata, (), metadata_area, true);
                canvas.draw(content, (), content_area, true);
            }
        }
    }
}

/// Inner state for the exchange pane. This contains all the empty states, as
/// well as one variant for the populated state
#[derive(Debug, Default)]
#[expect(clippy::large_enum_variant)]
enum State {
    /// Recipe list is empty
    #[default]
    None,
    /// Folder selected
    Folder,
    /// Recipe selected, but it has no request history
    NoHistory,
    /// We have a real bonafide request state available
    Content {
        metadata: ResponsePaneMetadata,
        content: ResponsePaneContent,
    },
}

impl State {
    fn new(
        selected_request: Option<&RequestState>,
        selected_recipe_kind: Option<RecipeNodeType>,
    ) -> Self {
        if let Some(request_state) = selected_request {
            // If we have a request, then there must be a recipe selected
            Self::Content {
                metadata: ResponsePaneMetadata {
                    id: ComponentId::default(),
                    request: request_state.request_metadata(),
                    response: request_state.response_metadata(),
                },
                content: ResponsePaneContent::new(request_state),
            }
        } else {
            // Without a request, show some sort of empty state
            match selected_recipe_kind {
                None => Self::None,
                Some(RecipeNodeType::Folder) => Self::Folder,
                Some(RecipeNodeType::Recipe) => Self::NoHistory,
            }
        }
    }
}

/// Top bar of the exchange pane, above the tabs
#[derive(Debug)]
struct ResponsePaneMetadata {
    id: ComponentId,
    request: RequestMetadata,
    response: Option<ResponseMetadata>,
}

impl Component for ResponsePaneMetadata {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl Draw for ResponsePaneMetadata {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let config = ViewContext::config();
        let styles = ViewContext::styles();
        let area = metadata.area();

        // Request metadata
        canvas.render_widget(
            Line::from(vec![
                self.request.start_time.generate(),
                " / ".into(),
                self.request.duration().generate(),
            ]),
            area,
        );

        // Response metadata
        if let Some(metadata) = self.response {
            canvas.render_widget(
                Line::from(vec![
                    metadata.status.generate(),
                    " ".into(),
                    Span::styled(
                        format_byte_size(metadata.size),
                        // Show some dangerous styling for large bodies, to
                        // indicate that something is different
                        if config.http.is_large(metadata.size) {
                            styles.text.error
                        } else {
                            Style::default()
                        },
                    ),
                ])
                .alignment(Alignment::Right),
                area,
            );
        }
    }
}

/// Persistence key for selected tab
#[derive(Debug, Serialize)]
struct ResponseTabKey;

impl PersistentKey for ResponseTabKey {
    type Value = Tab;
}

#[derive(
    Copy,
    Clone,
    Debug,
    Display,
    Default,
    EnumCount,
    EnumIter,
    PartialEq,
    Serialize,
    Deserialize,
)]
enum Tab {
    Headers,
    #[default]
    Body,
}

/// Content under the tab bar. Only rendered when a request state is present
#[derive(Debug)]
struct ResponsePaneContent {
    id: ComponentId,
    actions_emitter: Emitter<ResponsePaneMenuAction>,
    state: ResponsePaneContentState,
    /// In-progress requests spawn a task that periodically updates the UI.
    /// This ensures the timer is ticked correctly. There should never be more
    /// than one of these tick loops running at a time.
    ///
    /// We have to hang onto this because it cancels the task on drop.
    _tick_loop: Option<TickLoop>,
}

impl ResponsePaneContent {
    fn new(request_state: &RequestState) -> Self {
        let state = match request_state {
            RequestState::Building { .. } => ResponsePaneContentState::Building,
            RequestState::BuildCancelled { .. } => {
                ResponsePaneContentState::BuildCancelled
            }
            RequestState::BuildError { error } => {
                ResponsePaneContentState::BuildError {
                    error: (error as &dyn Error).generate(),
                }
            }
            RequestState::Loading { request, .. } => {
                ResponsePaneContentState::Loading {
                    request_id: request.id,
                }
            }
            RequestState::LoadingCancelled { request, .. } => {
                ResponsePaneContentState::LoadingCancelled {
                    request_id: request.id,
                }
            }
            RequestState::Response { exchange } => {
                ResponsePaneContentState::Response {
                    request_id: exchange.id,
                    tabs: Tabs::new(ResponseTabKey, FixedSelect::builder()),
                    response_headers: ResponseHeadersView::new(Arc::clone(
                        &exchange.response,
                    )),
                    response_body: ResponseBodyView::new(
                        exchange.request.recipe_id.clone(),
                        Arc::clone(&exchange.response),
                    ),
                }
            }
            RequestState::RequestError { error } => {
                ResponsePaneContentState::RequestError {
                    request_id: error.request.id,
                    error: (error as &dyn Error).generate(),
                }
            }
        };

        // If request is building or loading, spawn a task that will send empty
        // messages to the main loop periodically. This ensures the loading
        // timer will update.
        let tick_loop = if matches!(
            state,
            ResponsePaneContentState::Building
                | ResponsePaneContentState::Loading { .. }
        ) {
            Some(TickLoop::new(&ViewContext::messages_tx()))
        } else {
            None
        };

        Self {
            id: Default::default(),
            actions_emitter: Default::default(),
            state,
            _tick_loop: tick_loop,
        }
    }

    fn handle_menu_action(&mut self, menu_action: ResponsePaneMenuAction) {
        match menu_action {
            // Generally if we get an action the corresponding
            // request/response will be present, but we double check in
            // case the action got delayed in being
            // handled somehow
            ResponsePaneMenuAction::CopyBody => {
                self.state.response().map(ResponseBodyView::copy_body);
            }
            ResponsePaneMenuAction::ViewBody => {
                self.state.response().map(ResponseBodyView::view_body);
            }
            ResponsePaneMenuAction::SaveBody => {
                self.state
                    .response()
                    .map(ResponseBodyView::save_response_body);
            }
            ResponsePaneMenuAction::ResendRequest => {
                if let Some(id) = self.state.request_id() {
                    ViewContext::push_message(HttpMessage::Resend(id));
                }
            }
            ResponsePaneMenuAction::DeleteRequest => {
                ViewContext::push_message(Event::DeleteRequests(
                    DeleteTarget::Request,
                ));
            }
        }
    }
}

impl Component for ResponsePaneContent {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Delete if self.state.request_id().is_some() => {
                    // Root handles deletion so it can show a confirm modal
                    ViewContext::push_message(Event::DeleteRequests(
                        DeleteTarget::Request,
                    ));
                }
                _ => propagate.set(),
            })
            .emitted(self.actions_emitter, |menu_action| {
                self.handle_menu_action(menu_action);
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        let emitter = self.actions_emitter;
        let has_request = self.state.request_id().is_some();
        let has_response_body = match self.state {
            ResponsePaneContentState::Building
            | ResponsePaneContentState::BuildCancelled
            | ResponsePaneContentState::BuildError { .. }
            | ResponsePaneContentState::Loading { .. }
            | ResponsePaneContentState::LoadingCancelled { .. }
            | ResponsePaneContentState::RequestError { .. } => false,
            // All responses have a body
            ResponsePaneContentState::Response { .. } => true,
        };

        vec![
            MenuItem::Group {
                name: "Response".into(),
                children: vec![
                    emitter
                        .menu(ResponsePaneMenuAction::CopyBody, "Copy Body")
                        .enable(has_response_body)
                        .into(),
                    emitter
                        .menu(ResponsePaneMenuAction::ViewBody, "View Body")
                        .enable(has_response_body)
                        .shortcut(Some(Action::View))
                        .into(),
                    emitter
                        .menu(
                            ResponsePaneMenuAction::SaveBody,
                            "Save Body as File",
                        )
                        .enable(has_response_body)
                        .into(),
                ],
            },
            emitter
                .menu(ResponsePaneMenuAction::ResendRequest, "Resend Request")
                // It's possible the resend fails because the request had no
                // body. Until we have disabled reasons on these menus, that's
                // better because we can show an explanation to the user
                .enable(has_request)
                .into(),
            emitter
                .menu(ResponsePaneMenuAction::DeleteRequest, "Delete Request")
                .enable(has_request)
                .shortcut(Some(Action::Delete))
                .into(),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match &mut self.state {
            ResponsePaneContentState::Building
            | ResponsePaneContentState::BuildCancelled
            | ResponsePaneContentState::BuildError { .. }
            | ResponsePaneContentState::Loading { .. }
            | ResponsePaneContentState::LoadingCancelled { .. }
            | ResponsePaneContentState::RequestError { .. } => vec![],
            ResponsePaneContentState::Response {
                tabs,
                response_headers,
                response_body,
                ..
            } => vec![
                // Tabs go last so the query text box eats left/right first
                response_headers.to_child(),
                response_body.to_child(),
                tabs.to_child(),
            ],
        }
    }
}

impl Draw for ResponsePaneContent {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let area = metadata.area();
        match &self.state {
            ResponsePaneContentState::Building => {
                canvas.render_widget("Initializing request...", area);
            }
            ResponsePaneContentState::BuildCancelled => {
                canvas.render_widget("Build cancelled", area);
            }
            ResponsePaneContentState::BuildError { error } => {
                canvas.render_widget(error, area);
            }
            ResponsePaneContentState::Loading { .. } => {
                canvas.render_widget("Loading...", area);
            }
            ResponsePaneContentState::LoadingCancelled { .. } => {
                canvas.render_widget("Request cancelled", area);
            }
            ResponsePaneContentState::Response {
                tabs,
                response_body,
                response_headers,
                ..
            } => {
                let [tabs_area, content_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(area);
                canvas.draw(tabs, (), tabs_area, true);
                match tabs.selected() {
                    Tab::Body => {
                        canvas.draw(response_body, (), content_area, true);
                    }
                    Tab::Headers => {
                        canvas.draw(response_headers, (), content_area, true);
                    }
                }
            }
            ResponsePaneContentState::RequestError { error, .. } => {
                canvas.render_widget(error, area);
            }
        }
    }
}

/// Various request states that can appear under the tab bar
#[derive(Debug)]
enum ResponsePaneContentState {
    Building,
    BuildCancelled,
    BuildError {
        error: Text<'static>,
    },
    Loading {
        request_id: RequestId,
    },
    LoadingCancelled {
        request_id: RequestId,
    },
    Response {
        request_id: RequestId,
        tabs: Tabs<ResponseTabKey, Tab>,
        response_headers: ResponseHeadersView,
        response_body: ResponseBodyView,
    },
    RequestError {
        request_id: RequestId,
        error: Text<'static>,
    },
}

impl ResponsePaneContentState {
    fn request_id(&self) -> Option<RequestId> {
        match self {
            ResponsePaneContentState::Building
            | ResponsePaneContentState::BuildCancelled
            | ResponsePaneContentState::BuildError { .. } => None,
            ResponsePaneContentState::Loading { request_id }
            | ResponsePaneContentState::LoadingCancelled { request_id }
            | ResponsePaneContentState::Response { request_id, .. }
            | ResponsePaneContentState::RequestError { request_id, .. } => {
                Some(*request_id)
            }
        }
    }

    fn response(&self) -> Option<&ResponseBodyView> {
        match self {
            Self::Building
            | Self::BuildCancelled
            | Self::BuildError { .. }
            | Self::Loading { .. }
            | Self::LoadingCancelled { .. }
            | Self::RequestError { .. } => None,
            Self::Response { response_body, .. } => Some(response_body),
        }
    }
}

/// Items in the actions popup menu for the Body
#[derive(Copy, Clone, Debug)]
enum ResponsePaneMenuAction {
    CopyBody,
    ViewBody,
    SaveBody,
    ResendRequest,
    DeleteRequest,
}
