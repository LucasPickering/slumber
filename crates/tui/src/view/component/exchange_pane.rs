use crate::{
    http::{RequestMetadata, ResponseMetadata},
    message::HttpMessage,
    util::TickLoop,
    view::{
        Generate, RequestState, ViewContext,
        common::{
            Pane, actions::MenuItem, fixed_select::FixedSelect, tabs::Tabs,
        },
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            request_view::RequestView,
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

/// Display for a request/response exchange. This allows the user to switch
/// between request and response. This is bound to a particular [RequestState],
/// and should be recreated whenever the selected request changes state, or a
/// new request is selected.
#[derive(Debug)]
pub struct ExchangePane {
    id: ComponentId,
    state: State,
}

impl ExchangePane {
    pub fn new(
        selected_request: Option<&RequestState>,
        selected_recipe_kind: Option<RecipeNodeType>,
    ) -> Self {
        Self {
            id: Default::default(),
            state: State::new(selected_request, selected_recipe_kind),
        }
    }

    /// Get the ID of the displayed request
    pub fn request_id(&self) -> Option<RequestId> {
        match &self.state {
            State::None | State::Folder | State::NoHistory => None,
            State::Content { metadata, .. } => Some(metadata.request.id),
        }
    }
}

impl Component for ExchangePane {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match &mut self.state {
            State::None | State::Folder | State::NoHistory => {
                vec![]
            }
            State::Content { content, .. } => {
                vec![content.to_child_mut()]
            }
        }
    }
}

impl Draw for ExchangePane {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let title = ViewContext::add_binding_hint(
            "Request / Response",
            Action::SelectBottomPane,
        );
        let mut block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();

        // If a recipe is selected, history is available so show the hint
        if matches!(self.state, State::Content { .. }) {
            let text =
                ViewContext::add_binding_hint("History", Action::History);
            block = block.title(Line::from(text).alignment(Alignment::Right));
        }
        canvas.render_widget(&block, metadata.area());
        let area = block.inner(metadata.area());

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
        metadata: ExchangePaneMetadata,
        content: ExchangePaneContent,
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
                metadata: ExchangePaneMetadata {
                    id: ComponentId::default(),
                    request: request_state.request_metadata(),
                    response: request_state.response_metadata(),
                },
                content: ExchangePaneContent::new(request_state),
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
struct ExchangePaneMetadata {
    id: ComponentId,
    request: RequestMetadata,
    response: Option<ResponseMetadata>,
}

impl Component for ExchangePaneMetadata {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl Draw for ExchangePaneMetadata {
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
struct ExchangeTabKey;

impl PersistentKey for ExchangeTabKey {
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
    Request,
    #[default]
    Body,
    Headers,
}

/// Content under the tab bar. Only rendered when a request state is present
#[derive(Debug)]
struct ExchangePaneContent {
    id: ComponentId,
    actions_emitter: Emitter<ExchangePaneMenuAction>,
    tabs: Tabs<ExchangeTabKey, Tab>,
    state: ExchangePaneContentState,
    /// In-progress requests spawn a task that periodically updates the UI.
    /// This ensures the timer is ticked correctly. There should never be more
    /// than one of these tick loops running at a time.
    ///
    /// We have to hang onto this because it cancels the task on drop.
    _tick_loop: Option<TickLoop>,
}

impl ExchangePaneContent {
    fn new(request_state: &RequestState) -> Self {
        let state = match request_state {
            RequestState::Building { .. } => ExchangePaneContentState::Building,
            RequestState::BuildCancelled { .. } => {
                ExchangePaneContentState::BuildCancelled
            }
            RequestState::BuildError { error } => {
                ExchangePaneContentState::BuildError {
                    error: (error as &dyn Error).generate(),
                }
            }
            RequestState::Loading { request, .. } => {
                ExchangePaneContentState::Loading {
                    request: RequestView::new(Arc::clone(request)),
                }
            }
            RequestState::LoadingCancelled { request, .. } => {
                ExchangePaneContentState::LoadingCancelled {
                    request: RequestView::new(Arc::clone(request)),
                }
            }
            RequestState::Response { exchange } => {
                ExchangePaneContentState::Response {
                    request: RequestView::new(Arc::clone(&exchange.request)),
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
                ExchangePaneContentState::RequestError {
                    request: RequestView::new(Arc::clone(&error.request)),
                    error: (error as &dyn Error).generate(),
                }
            }
        };

        // If request is building or loading, spawn a task that will send empty
        // messages to the main loop periodically. This ensures the loading
        // timer will update.
        let tick_loop = if matches!(
            state,
            ExchangePaneContentState::Building
                | ExchangePaneContentState::Loading { .. }
        ) {
            Some(TickLoop::new(&ViewContext::messages_tx()))
        } else {
            None
        };

        Self {
            id: Default::default(),
            actions_emitter: Default::default(),
            tabs: Tabs::new(ExchangeTabKey, FixedSelect::builder()),
            state,
            _tick_loop: tick_loop,
        }
    }

    fn handle_menu_action(&mut self, menu_action: ExchangePaneMenuAction) {
        match menu_action {
            // Generally if we get an action the corresponding
            // request/response will be present, but we double check in
            // case the action got delayed in being
            // handled somehow
            ExchangePaneMenuAction::CopyUrl => {
                self.state.request().map(RequestView::copy_url);
            }
            ExchangePaneMenuAction::ViewRequestBody => {
                self.state.request().map(RequestView::view_body);
            }
            ExchangePaneMenuAction::CopyRequestBody => {
                self.state.request().map(RequestView::copy_body);
            }
            ExchangePaneMenuAction::CopyResponseBody => {
                self.state.response().map(ResponseBodyView::copy_body);
            }
            ExchangePaneMenuAction::ViewResponseBody => {
                self.state.response().map(ResponseBodyView::view_body);
            }
            ExchangePaneMenuAction::SaveResponseBody => {
                self.state
                    .response()
                    .map(ResponseBodyView::save_response_body);
            }
            ExchangePaneMenuAction::ResendRequest => {
                self.state.request().inspect(|request| {
                    ViewContext::send_message(HttpMessage::Resend(
                        request.request_id(),
                    ));
                });
            }
            ExchangePaneMenuAction::DeleteRequest => {
                ViewContext::push_event(Event::DeleteRequests(
                    DeleteTarget::Request,
                ));
            }
        }
    }
}

impl Component for ExchangePaneContent {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Delete if self.state.request().is_some() => {
                    // Root handles deletion so it can show a confirm modal
                    ViewContext::push_event(Event::DeleteRequests(
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
        let request = self.state.request();
        let has_request = request.is_some();
        let has_request_body = request.is_some_and(RequestView::has_body);
        let has_response_body = match self.state {
            ExchangePaneContentState::Building
            | ExchangePaneContentState::BuildCancelled
            | ExchangePaneContentState::BuildError { .. }
            | ExchangePaneContentState::Loading { .. }
            | ExchangePaneContentState::LoadingCancelled { .. }
            | ExchangePaneContentState::RequestError { .. } => false,
            ExchangePaneContentState::Response { .. } => true,
        };
        let selected_tab = self.tabs.selected();

        vec![
            MenuItem::Group {
                name: "Request".into(),
                children: vec![
                    emitter
                        .menu(ExchangePaneMenuAction::CopyUrl, "Copy URL")
                        .enable(has_request)
                        .into(),
                    emitter
                        .menu(
                            ExchangePaneMenuAction::CopyRequestBody,
                            "Copy Body",
                        )
                        .enable(has_request_body)
                        .into(),
                    emitter
                        .menu(
                            ExchangePaneMenuAction::ViewRequestBody,
                            "View Body",
                        )
                        .enable(has_request_body)
                        .shortcut(
                            (selected_tab == Tab::Request)
                                .then_some(Action::View),
                        )
                        .into(),
                ],
            },
            MenuItem::Group {
                name: "Response".into(),
                children: vec![
                    emitter
                        .menu(
                            ExchangePaneMenuAction::CopyResponseBody,
                            "Copy Body",
                        )
                        .enable(has_response_body)
                        .into(),
                    emitter
                        .menu(
                            ExchangePaneMenuAction::ViewResponseBody,
                            "View Body",
                        )
                        .enable(has_response_body)
                        .shortcut(
                            (selected_tab == Tab::Body).then_some(Action::View),
                        )
                        .into(),
                    emitter
                        .menu(
                            ExchangePaneMenuAction::SaveResponseBody,
                            "Save Body as File",
                        )
                        .enable(has_response_body)
                        .into(),
                ],
            },
            emitter
                .menu(ExchangePaneMenuAction::ResendRequest, "Resend Request")
                // It's possible the resend fails because the request had no
                // body. Until we have disabled reasons on these menus, that's
                // better because we can show an explanation to the user
                .enable(has_request)
                .into(),
            emitter
                .menu(ExchangePaneMenuAction::DeleteRequest, "Delete Request")
                .enable(has_request)
                .shortcut(Some(Action::Delete))
                .into(),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        // Add tab content
        let mut children = match &mut self.state {
            ExchangePaneContentState::Building
            | ExchangePaneContentState::BuildCancelled
            | ExchangePaneContentState::BuildError { .. } => vec![],
            ExchangePaneContentState::Loading { request }
            | ExchangePaneContentState::LoadingCancelled { request } => {
                vec![request.to_child_mut()]
            }
            ExchangePaneContentState::Response {
                request,
                response_headers,
                response_body,
            } => vec![
                request.to_child_mut(),
                response_headers.to_child_mut(),
                response_body.to_child_mut(),
            ],
            ExchangePaneContentState::RequestError { request, .. } => {
                vec![request.to_child_mut()]
            }
        };

        // Content before tabs so the query text box gets priority on left/right
        // arrow keys
        children.push(self.tabs.to_child_mut());
        children
    }
}

impl Draw for ExchangePaneContent {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let [tabs_area, content_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(metadata.area());
        canvas.draw(&self.tabs, (), tabs_area, true);
        match &self.state {
            ExchangePaneContentState::Building => {
                canvas.render_widget("Initializing request...", content_area);
            }
            ExchangePaneContentState::BuildCancelled => {
                canvas.render_widget("Build cancelled", content_area);
            }
            ExchangePaneContentState::BuildError { error } => {
                canvas.render_widget(error, content_area);
            }
            ExchangePaneContentState::Loading { request } => {
                match self.tabs.selected() {
                    Tab::Request => {
                        canvas.draw(request, (), content_area, true);
                    }
                    Tab::Body | Tab::Headers => {
                        canvas.render_widget("Loading...", content_area);
                    }
                }
            }
            ExchangePaneContentState::LoadingCancelled { request } => {
                match self.tabs.selected() {
                    Tab::Request => {
                        canvas.draw(request, (), content_area, true);
                    }
                    Tab::Body | Tab::Headers => {
                        canvas.render_widget("Request cancelled", content_area);
                    }
                }
            }
            ExchangePaneContentState::Response {
                request,
                response_body,
                response_headers,
            } => match self.tabs.selected() {
                Tab::Request => canvas.draw(request, (), content_area, true),
                Tab::Body => canvas.draw(response_body, (), content_area, true),
                Tab::Headers => {
                    canvas.draw(response_headers, (), content_area, true);
                }
            },
            ExchangePaneContentState::RequestError { request, error } => {
                match self.tabs.selected() {
                    Tab::Request => {
                        canvas.draw(request, (), content_area, true);
                    }
                    Tab::Body | Tab::Headers => {
                        canvas.render_widget(error, content_area);
                    }
                }
            }
        }
    }
}

/// Various request states that can appear under the tab bar
#[derive(Debug)]
enum ExchangePaneContentState {
    Building,
    BuildCancelled,
    BuildError {
        error: Text<'static>,
    },
    Loading {
        request: RequestView,
    },
    LoadingCancelled {
        request: RequestView,
    },
    Response {
        request: RequestView,
        response_headers: ResponseHeadersView,
        response_body: ResponseBodyView,
    },
    RequestError {
        request: RequestView,
        error: Text<'static>,
    },
}

impl ExchangePaneContentState {
    fn request(&self) -> Option<&RequestView> {
        match self {
            Self::Building | Self::BuildCancelled | Self::BuildError { .. } => {
                None
            }
            Self::Loading { request }
            | Self::LoadingCancelled { request }
            | Self::Response { request, .. }
            | Self::RequestError { request, .. } => Some(request),
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
enum ExchangePaneMenuAction {
    CopyUrl,
    CopyRequestBody,
    ViewRequestBody,
    CopyResponseBody,
    ViewResponseBody,
    SaveResponseBody,
    ResendRequest,
    DeleteRequest,
}
