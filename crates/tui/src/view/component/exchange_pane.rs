use crate::{
    context::TuiContext,
    http::{RequestMetadata, ResponseMetadata},
    view::{
        Generate, RequestState,
        common::{Pane, actions::MenuAction, modal::Modal, tabs::Tabs},
        component::{
            Component, ComponentExt, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            misc::DeleteRequestModal,
            request_view::RequestView,
            response_view::{ResponseBodyView, ResponseHeadersView},
        },
        context::UpdateContext,
        event::{Emitter, Event, OptionEvent, ToEmitter},
        util::{format_byte_size, persistence::PersistedLazy},
    },
};
use derive_more::Display;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::collection::RecipeNodeType;
use std::{error::Error, sync::Arc};
use strum::{EnumCount, EnumIter};

/// Display for a request/response exchange. This allows the user to switch
/// between request and response. This is bound to a particular [RequestState],
/// and should be recreated whenever the selected request changes state, or a
/// new request is selected.
#[derive(Debug, Default)]
pub struct ExchangePane {
    id: ComponentId,
    emitter: Emitter<ExchangePaneEvent>,
    state: State,
}

impl ExchangePane {
    pub fn new(
        selected_request: Option<&RequestState>,
        selected_recipe_kind: Option<RecipeNodeType>,
    ) -> Self {
        Self {
            id: Default::default(),
            emitter: Default::default(),
            state: State::new(selected_request, selected_recipe_kind),
        }
    }
}

impl Component for ExchangePane {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().action(|action, propagate| match action {
            Action::LeftClick => self.emitter.emit(ExchangePaneEvent::Click),
            _ => propagate.set(),
        })
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
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        let input_engine = &TuiContext::get().input_engine;
        let title =
            input_engine.add_hint("Request / Response", Action::SelectResponse);
        let mut block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();

        // If a recipe is selected, history is available so show the hint
        if matches!(self.state, State::Content { .. }) {
            let text = input_engine.add_hint("History", Action::History);
            block = block.title(Line::from(text).alignment(Alignment::Right));
        }
        frame.render_widget(&block, metadata.area());
        let area = block.inner(metadata.area());

        match &self.state {
            // Recipe pane will show a note about how to add a recipe, so we
            // don't need anything here
            State::None => {}
            State::Folder => frame.render_widget(
                "Select a recipe to see its request history",
                area,
            ),
            State::NoHistory => frame.render_widget(
                "No request history for this recipe & profile",
                area,
            ),
            State::Content { metadata, content } => {
                let [metadata_area, content_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(area);

                metadata.draw(frame, (), metadata_area, true);
                content.draw(frame, (), content_area, true);
            }
        }
    }
}

/// Notify parent when this pane is clicked
impl ToEmitter<ExchangePaneEvent> for ExchangePane {
    fn to_emitter(&self) -> Emitter<ExchangePaneEvent> {
        self.emitter
    }
}

/// Emitted event for the exchange pane component
#[derive(Debug)]
pub enum ExchangePaneEvent {
    Click,
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
        match (selected_request, selected_recipe_kind) {
            (_, None) => Self::None,
            (_, Some(RecipeNodeType::Folder)) => Self::Folder,
            (None, Some(RecipeNodeType::Recipe)) => Self::NoHistory,
            (Some(request_state), Some(RecipeNodeType::Recipe)) => {
                Self::Content {
                    metadata: ExchangePaneMetadata {
                        id: ComponentId::default(),
                        request: request_state.request_metadata(),
                        response: request_state.response_metadata(),
                    },
                    content: ExchangePaneContent::new(request_state),
                }
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
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        let tui_context = TuiContext::get();
        let config = &tui_context.config;
        let styles = &tui_context.styles;
        let area = metadata.area();

        // Request metadata
        frame.render_widget(
            Line::from(vec![
                self.request.start_time.generate(),
                " / ".into(),
                self.request.duration().generate(),
            ]),
            area,
        );

        // Response metadata
        if let Some(metadata) = self.response {
            frame.render_widget(
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
#[derive(Debug, Default, persisted::PersistedKey, Serialize)]
#[persisted(Tab)]
struct ExchangeTabKey;

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
    tabs: PersistedLazy<ExchangeTabKey, Tabs<Tab>>,
    state: ExchangePaneContentState,
}

impl ExchangePaneContent {
    fn new(request_state: &RequestState) -> Self {
        let state = match request_state {
            RequestState::Building { .. } => ExchangePaneContentState::Building,
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
            RequestState::Cancelled { .. } => {
                ExchangePaneContentState::Cancelled
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
        Self {
            id: ComponentId::default(),
            actions_emitter: Default::default(),
            tabs: Default::default(),
            state,
        }
    }
}

impl Component for ExchangePaneContent {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::Delete => {
                    if let Some(request) = self.state.request() {
                        // Show a confirmation modal
                        DeleteRequestModal::new(request.id()).open();
                    }
                }
                _ => propagate.set(),
            })
            .emitted(self.actions_emitter, |menu_action| {
                match menu_action {
                    // Generally if we get an action the corresponding
                    // request/response will be present, but we double check in
                    // case the action got delayed in being
                    // handled somehow
                    ExchangePaneMenuAction::CopyUrl => {
                        if let Some(request) = self.state.request() {
                            request.copy_url();
                        }
                    }
                    ExchangePaneMenuAction::ViewRequestBody => {
                        if let Some(request) = self.state.request() {
                            request.view_body();
                        }
                    }
                    ExchangePaneMenuAction::CopyRequestBody => {
                        if let Some(request) = self.state.request() {
                            request.copy_body();
                        }
                    }
                    ExchangePaneMenuAction::CopyResponseBody => {
                        if let Some(response) = self.state.response() {
                            response.copy_body();
                        }
                    }
                    ExchangePaneMenuAction::ViewResponseBody => {
                        if let Some(response) = self.state.response() {
                            response.view_body();
                        }
                    }
                    ExchangePaneMenuAction::SaveResponseBody => {
                        if let Some(response) = self.state.response() {
                            response.save_response_body();
                        }
                    }
                    ExchangePaneMenuAction::DeleteRequest => {
                        if let Some(request) = self.state.request() {
                            // Show a confirmation modal
                            DeleteRequestModal::new(request.id()).open();
                        }
                    }
                }
            })
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        let emitter = self.actions_emitter;
        let request = self.state.request();
        let has_request = request.is_some();
        let has_request_body = request.is_some_and(RequestView::has_body);
        let has_response_body = match self.state {
            ExchangePaneContentState::Building
            | ExchangePaneContentState::BuildError { .. }
            | ExchangePaneContentState::Cancelled
            | ExchangePaneContentState::Loading { .. }
            | ExchangePaneContentState::RequestError { .. } => false,
            ExchangePaneContentState::Response { .. } => true,
        };
        let selected_tab = self.tabs.selected();

        vec![
            emitter
                .menu(ExchangePaneMenuAction::CopyUrl, "Copy URL")
                .enable(has_request),
            emitter
                .menu(
                    ExchangePaneMenuAction::CopyRequestBody,
                    "Copy Request Body",
                )
                .enable(has_request_body),
            emitter
                .menu(
                    ExchangePaneMenuAction::ViewRequestBody,
                    "View Request Body",
                )
                .enable(has_request_body)
                .shortcut(
                    (selected_tab == Tab::Request).then_some(Action::View),
                ),
            emitter
                .menu(
                    ExchangePaneMenuAction::CopyResponseBody,
                    "Copy Response Body",
                )
                .enable(has_response_body),
            emitter
                .menu(
                    ExchangePaneMenuAction::ViewResponseBody,
                    "View Response Body",
                )
                .enable(has_response_body)
                .shortcut((selected_tab == Tab::Body).then_some(Action::View)),
            emitter
                .menu(
                    ExchangePaneMenuAction::SaveResponseBody,
                    "Save Response Body as File",
                )
                .enable(has_response_body),
            emitter
                .menu(ExchangePaneMenuAction::DeleteRequest, "Delete Request")
                .enable(has_request)
                .shortcut(Some(Action::Delete)),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match &mut self.state {
            // Tabs last so the children get priority
            ExchangePaneContentState::Building
            | ExchangePaneContentState::BuildError { .. }
            | ExchangePaneContentState::Cancelled => {
                vec![self.tabs.to_child_mut()]
            }
            ExchangePaneContentState::Loading { request } => {
                vec![request.to_child_mut(), self.tabs.to_child_mut()]
            }
            ExchangePaneContentState::Response {
                request,
                response_headers,
                response_body,
            } => vec![
                request.to_child_mut(),
                response_headers.to_child_mut(),
                response_body.to_child_mut(),
                self.tabs.to_child_mut(),
            ],
            ExchangePaneContentState::RequestError { request, .. } => {
                vec![request.to_child_mut(), self.tabs.to_child_mut()]
            }
        }
    }
}

impl Draw for ExchangePaneContent {
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        let [tabs_area, content_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(metadata.area());
        self.tabs.draw(frame, (), tabs_area, true);
        match &self.state {
            ExchangePaneContentState::Building => {
                frame.render_widget("Initializing request...", content_area);
            }
            ExchangePaneContentState::BuildError { error } => {
                frame.render_widget(error, content_area);
            }
            ExchangePaneContentState::Loading { request } => {
                match self.tabs.selected() {
                    Tab::Request => request.draw(frame, (), content_area, true),
                    Tab::Body | Tab::Headers => {
                        frame.render_widget("Loading...", content_area);
                    }
                }
            }
            // Can't show cancelled request here because we might've cancelled
            // during the build
            ExchangePaneContentState::Cancelled => {
                frame.render_widget("Request cancelled", content_area);
            }
            ExchangePaneContentState::Response {
                request,
                response_body,
                response_headers,
            } => match self.tabs.selected() {
                Tab::Request => request.draw(frame, (), content_area, true),
                Tab::Body => response_body.draw(frame, (), content_area, true),
                Tab::Headers => {
                    response_headers.draw(frame, (), content_area, true);
                }
            },
            ExchangePaneContentState::RequestError { request, error } => {
                match self.tabs.selected() {
                    Tab::Request => request.draw(frame, (), content_area, true),
                    Tab::Body | Tab::Headers => {
                        frame.render_widget(error, content_area);
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
    BuildError {
        error: Paragraph<'static>,
    },
    Loading {
        request: RequestView,
    },
    Cancelled,
    Response {
        request: RequestView,
        response_headers: ResponseHeadersView,
        response_body: ResponseBodyView,
    },
    RequestError {
        request: RequestView,
        error: Paragraph<'static>,
    },
}

impl ExchangePaneContentState {
    fn request(&self) -> Option<&RequestView> {
        match self {
            Self::Building | Self::BuildError { .. } | Self::Cancelled => None,
            Self::Loading { request }
            | Self::Response { request, .. }
            | Self::RequestError { request, .. } => Some(request),
        }
    }

    fn response(&self) -> Option<&ResponseBodyView> {
        match self {
            Self::Building
            | Self::BuildError { .. }
            | Self::Cancelled
            | Self::Loading { .. }
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
    DeleteRequest,
}
