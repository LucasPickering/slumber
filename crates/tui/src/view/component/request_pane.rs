use crate::{
    http::{RequestMetadata, ResponseMetadata},
    message::{HttpMessage, Message},
    util::syntax::SyntaxType,
    view::{
        Generate, RequestState, ViewContext,
        common::{
            actions::MenuItem,
            fixed_select::FixedSelect,
            header_table::HeaderTable,
            tabs::Tabs,
            text_window::{TextWindow, TextWindowProps},
        },
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
        },
        context::UpdateContext,
        event::{DeleteTarget, Emitter, Event, EventMatch},
        persistent::PersistentKey,
        util::{format_byte_size, highlight, view_text},
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
use slumber_core::{
    collection::RecipeNodeType,
    http::{Exchange, RequestBody, RequestId, RequestRecord},
    util::MaybeStr,
};
use std::{error::Error, sync::Arc};
use strum::{EnumCount, EnumIter};

/// Display for a request
///
/// This is bound to a particular [RequestState]. It should be recreated
/// whenever the selected request changes state or a new request is selected.
#[derive(Debug)]
pub struct RequestPane {
    id: ComponentId,
    state: State,
}

impl RequestPane {
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

impl Component for RequestPane {
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

impl Draw for RequestPane {
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
        metadata: RequestPaneMetadata,
        content: RequestPaneContent,
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
                metadata: RequestPaneMetadata {
                    id: ComponentId::default(),
                    request: request_state.request_metadata(),
                    response: request_state.response_metadata(),
                },
                content: RequestPaneContent::new(request_state),
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
struct RequestPaneMetadata {
    id: ComponentId,
    request: RequestMetadata,
    response: Option<ResponseMetadata>,
}

impl Component for RequestPaneMetadata {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl Draw for RequestPaneMetadata {
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
struct RequestTabKey;

impl PersistentKey for RequestTabKey {
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
struct RequestPaneContent {
    id: ComponentId,
    actions_emitter: Emitter<RequestPaneMenuAction>,
    state: RequestPaneContentState,
}

impl RequestPaneContent {
    fn new(request_state: &RequestState) -> Self {
        let state = match request_state {
            RequestState::Building { .. } => RequestPaneContentState::Building,
            RequestState::BuildCancelled { .. } => {
                RequestPaneContentState::BuildCancelled
            }
            RequestState::BuildError { error } => {
                RequestPaneContentState::BuildError {
                    error: (error as &dyn Error).generate(),
                }
            }
            RequestState::Loading { request, .. }
            | RequestState::LoadingCancelled { request, .. }
            | RequestState::Response {
                exchange: Exchange { request, .. },
            } => RequestPaneContentState::Request {
                request: RequestView::new(Arc::clone(request)),
            },
            RequestState::RequestError { error } => {
                RequestPaneContentState::Request {
                    request: RequestView::new(Arc::clone(&error.request)),
                }
            }
        };

        Self {
            id: Default::default(),
            actions_emitter: Default::default(),
            state,
        }
    }

    fn handle_menu_action(&mut self, menu_action: RequestPaneMenuAction) {
        match menu_action {
            // Generally if we get an action the corresponding
            // request/response will be present, but we double check in
            // case the action got delayed in being
            // handled somehow
            RequestPaneMenuAction::CopyUrl => {
                self.state.request().map(RequestView::copy_url);
            }
            RequestPaneMenuAction::ViewBody => {
                self.state.request().map(RequestView::view_body);
            }
            RequestPaneMenuAction::CopyBody => {
                self.state.request().map(RequestView::copy_body);
            }
            RequestPaneMenuAction::ResendRequest => {
                self.state.request().inspect(|request| {
                    ViewContext::push_message(HttpMessage::Resend(
                        request.request_id(),
                    ));
                });
            }
            RequestPaneMenuAction::DeleteRequest => {
                ViewContext::push_message(Event::DeleteRequests(
                    DeleteTarget::Request,
                ));
            }
        }
    }
}

impl Component for RequestPaneContent {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Delete if self.state.request().is_some() => {
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
        let request = self.state.request();
        let has_request = request.is_some();
        let has_request_body = request.is_some_and(RequestView::has_body);

        vec![
            MenuItem::Group {
                name: "Request".into(),
                children: vec![
                    emitter
                        .menu(RequestPaneMenuAction::CopyUrl, "Copy URL")
                        .enable(has_request)
                        .into(),
                    emitter
                        .menu(RequestPaneMenuAction::CopyBody, "Copy Body")
                        .enable(has_request_body)
                        .into(),
                    emitter
                        .menu(RequestPaneMenuAction::ViewBody, "View Body")
                        .enable(has_request_body)
                        .shortcut(Some(Action::View))
                        .into(),
                ],
            },
            emitter
                .menu(RequestPaneMenuAction::ResendRequest, "Resend Request")
                // It's possible the resend fails because the request had no
                // body. Until we have disabled reasons on these menus, that's
                // better because we can show an explanation to the user
                .enable(has_request)
                .into(),
            emitter
                .menu(RequestPaneMenuAction::DeleteRequest, "Delete Request")
                .enable(has_request)
                .shortcut(Some(Action::Delete))
                .into(),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match &mut self.state {
            RequestPaneContentState::Building
            | RequestPaneContentState::BuildCancelled
            | RequestPaneContentState::BuildError { .. } => vec![],
            RequestPaneContentState::Request { request } => {
                vec![request.to_child()]
            }
        }
    }
}

impl Draw for RequestPaneContent {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let area = metadata.area();
        match &self.state {
            RequestPaneContentState::Building => {
                canvas.render_widget("Initializing request...", area);
            }
            RequestPaneContentState::BuildCancelled => {
                canvas.render_widget("Build cancelled", area);
            }
            RequestPaneContentState::BuildError { error } => {
                canvas.render_widget(error, area);
            }
            RequestPaneContentState::Request { request } => {
                canvas.draw(request, (), area, true);
            }
        }
    }
}

/// Various request states that can appear under the tab bar
#[derive(Debug)]
enum RequestPaneContentState {
    Building,
    BuildCancelled,
    BuildError { error: Text<'static> },
    Request { request: RequestView },
}

impl RequestPaneContentState {
    fn request(&self) -> Option<&RequestView> {
        match self {
            Self::Building | Self::BuildCancelled | Self::BuildError { .. } => {
                None
            }
            Self::Request { request } => Some(request),
        }
    }
}

/// Menu actions for the Request pane
#[derive(Copy, Clone, Debug)]
enum RequestPaneMenuAction {
    CopyUrl,
    CopyBody,
    ViewBody,
    ResendRequest,
    DeleteRequest,
}

/// Display rendered HTTP request state. The request could still be in flight,
/// it just needs to have been built successfully.
#[derive(Debug)]
struct RequestView {
    id: ComponentId,
    tabs: Tabs<RequestTabKey, Tab>,
    /// Store pointer to the request, so we can access it in the update step
    request: Arc<RequestRecord>,
    /// Body display. `None` if the request has no body
    body_text_window: Option<TextWindow>,
}

impl RequestView {
    fn new(request: Arc<RequestRecord>) -> Self {
        let text = init_body(&request);
        Self {
            id: ComponentId::default(),
            tabs: Tabs::new(RequestTabKey, FixedSelect::builder()),
            request,
            body_text_window: text.map(TextWindow::new),
        }
    }

    fn request_id(&self) -> RequestId {
        self.request.id
    }

    fn has_body(&self) -> bool {
        self.body_text_window.is_some()
    }

    fn copy_url(&self) {
        ViewContext::push_message(Message::CopyText(
            self.request.url.to_string(),
        ));
    }

    fn view_body(&self) {
        if let Some(text_window) = &self.body_text_window {
            view_text(text_window.text(), self.request.mime());
        }
    }

    fn copy_body(&self) {
        // Copy exactly what the user sees. Currently requests don't support
        // formatting/querying but that could change
        if let Some(text_window) = &self.body_text_window {
            let body = text_window.text().to_string();
            ViewContext::push_message(Message::CopyText(body));
        }
    }
}

impl Component for RequestView {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event.m().action(|action, propagate| match action {
            Action::View => self.view_body(),
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.body_text_window.to_child(), self.tabs.to_child()]
    }
}

impl Draw for RequestView {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let [tabs_area, content_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(metadata.area());
        canvas.draw(&self.tabs, (), tabs_area, true);

        let request = &self.request;
        match self.tabs.selected() {
            Tab::Headers => {
                let [version_area, url_area, headers_area] =
                    Layout::vertical([
                        Constraint::Length(1),
                        Constraint::Length(2),
                        Constraint::Length(request.headers.len() as u16 + 2),
                    ])
                    .areas(content_area);

                // This can get cut off which is jank but there isn't a good
                // fix. User can copy the URL to see the full
                // thing
                canvas.render_widget(
                    format!("{} {}", request.method, request.http_version),
                    version_area,
                );
                canvas.render_widget(request.url.to_string(), url_area);
                canvas.render_widget(
                    HeaderTable {
                        headers: &request.headers,
                    },
                    headers_area,
                );
            }
            Tab::Body => {
                if let Some(text_window) = &self.body_text_window {
                    canvas.draw(
                        text_window,
                        TextWindowProps::default(),
                        content_area,
                        true,
                    );
                } else {
                    // TODO empty state
                }
            }
        }
    }
}

/// Calculate body text, including syntax highlighting. We have to clone the
/// body to prevent a self-reference. Return `None` if the request has no body
fn init_body(request: &RequestRecord) -> Option<Text<'static>> {
    let syntax_type = SyntaxType::from_headers(
        ViewContext::config().mime_overrides(),
        &request.headers,
    );
    match &request.body {
        RequestBody::None => None,
        RequestBody::Stream => Some(Text::raw("<stream>")),
        RequestBody::TooLarge => {
            let config = &ViewContext::config();
            Some(Text::raw(format!(
                "Large body (bodies over {} are not saved)",
                format_byte_size(config.http.large_body_size)
            )))
        }
        RequestBody::Some(bytes) => Some(highlight::highlight_if(
            syntax_type,
            format!("{:#}", MaybeStr(bytes)).into(),
        )),
    }
}
