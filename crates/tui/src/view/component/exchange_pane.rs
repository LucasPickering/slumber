use crate::{
    context::TuiContext,
    http::{RequestMetadata, ResponseMetadata},
    view::{
        common::{tabs::Tabs, Pane},
        component::{
            request_view::RequestView,
            response_view::{ResponseBodyView, ResponseHeadersView},
            Component,
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Emitter, EmitterId, Event, EventHandler, OptionEvent},
        util::persistence::PersistedLazy,
        RequestState,
    },
};
use derive_more::Display;
use persisted::SingletonKey;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{block::Title, Paragraph},
    Frame,
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::{collection::RecipeNodeType, util::format_byte_size};
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display for a request/response exchange. This allows the user to switch
/// between request and response. This is bound to a particular [RequestState],
/// and should be recreated whenever the selected request changes state, or a
/// new request is selected.
#[derive(Debug)]
pub struct ExchangePane {
    emitter_id: EmitterId,
    tabs: Component<PersistedLazy<SingletonKey<Tab>, Tabs<Tab>>>,
    state: State,
}

impl ExchangePane {
    pub fn new(
        selected_request: Option<&RequestState>,
        selected_recipe_kind: Option<RecipeNodeType>,
    ) -> Self {
        Self {
            emitter_id: Default::default(),
            tabs: Default::default(),
            state: State::new(selected_request, selected_recipe_kind),
        }
    }
}

impl EventHandler for ExchangePane {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().action(|action, propagate| match action {
            Action::LeftClick => self.emit(ExchangePaneEvent::Click),
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        match &mut self.state {
            // Tabs won't be visible in these empty states so we don't *need*
            // to return it, but it doesn't matter
            State::None | State::Folder | State::NoHistory => {
                vec![self.tabs.to_child_mut()]
            }

            // Tabs last so the children get priority
            State::Content { content, .. } => {
                vec![content.to_child_mut(), self.tabs.to_child_mut()]
            }
        }
    }
}

impl Draw for ExchangePane {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
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
            block = block.title(Title::from(text).alignment(Alignment::Right));
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
                let [metadata_area, tabs_area, content_area] =
                    Layout::vertical([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Min(0),
                    ])
                    .areas(area);

                metadata.draw(frame, (), metadata_area, true);
                self.tabs.draw(frame, (), tabs_area, true);
                content.draw(
                    frame,
                    ExchangePaneContentProps {
                        selected_tab: self.tabs.data().selected(),
                    },
                    content_area,
                    true,
                );
            }
        }
    }
}

/// Notify parent when this pane is clicked
impl Emitter for ExchangePane {
    type Emitted = ExchangePaneEvent;

    fn id(&self) -> EmitterId {
        self.emitter_id
    }
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

/// Emitted event for the exchange pane component
#[derive(Debug)]
pub enum ExchangePaneEvent {
    Click,
}

/// Inner state for the exchange pane. This contains all the empty states, as
/// well as one variant for the populated state
#[derive(Debug)]
enum State {
    /// Recipe list is empty
    None,
    /// Folder selected
    Folder,
    /// Recipe selected, but it has no request history
    NoHistory,
    /// We have a real bonafide request state available
    Content {
        metadata: Component<ExchangePaneMetadata>,
        content: Component<ExchangePaneContent>,
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
                        request: request_state.request_metadata(),
                        response: request_state.response_metadata(),
                    }
                    .into(),
                    content: ExchangePaneContent::new(request_state).into(),
                }
            }
        }
    }
}

/// Top bar of the exchange pane, above the tabs
#[derive(Debug)]
struct ExchangePaneMetadata {
    request: RequestMetadata,
    response: Option<ResponseMetadata>,
}

impl Draw for ExchangePaneMetadata {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
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

/// Content under the tab bar. Only rendered when a request state is present
#[derive(Debug)]
enum ExchangePaneContent {
    Building,
    BuildError {
        error: Paragraph<'static>,
    },
    Loading {
        request: Component<RequestView>,
    },
    Cancelled,
    Response {
        request: Component<RequestView>,
        response_headers: Component<ResponseHeadersView>,
        response_body: Component<ResponseBodyView>,
    },
    RequestError {
        request: Component<RequestView>,
        error: Paragraph<'static>,
    },
}

struct ExchangePaneContentProps {
    selected_tab: Tab,
}

impl ExchangePaneContent {
    fn new(request_state: &RequestState) -> Self {
        match request_state {
            RequestState::Building { .. } => Self::Building,
            RequestState::BuildError { error } => Self::BuildError {
                error: error.generate(),
            },
            RequestState::Loading { request, .. } => Self::Loading {
                request: RequestView::new(Arc::clone(request)).into(),
            },
            RequestState::Cancelled { .. } => Self::Cancelled,
            RequestState::Response { exchange } => Self::Response {
                request: RequestView::new(Arc::clone(&exchange.request)).into(),
                response_headers: ResponseHeadersView::new(Arc::clone(
                    &exchange.response,
                ))
                .into(),
                response_body: ResponseBodyView::new(
                    exchange.request.recipe_id.clone(),
                    Arc::clone(&exchange.response),
                )
                .into(),
            },
            RequestState::RequestError { error } => Self::RequestError {
                request: RequestView::new(Arc::clone(&error.request)).into(),
                error: error.generate(),
            },
        }
    }
}

impl EventHandler for ExchangePaneContent {
    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        match self {
            Self::Building | Self::BuildError { .. } | Self::Cancelled => {
                vec![]
            }
            Self::Loading { request } => vec![request.to_child_mut()],
            Self::Response {
                request,
                response_headers,
                response_body,
            } => vec![
                request.to_child_mut(),
                response_headers.to_child_mut(),
                response_body.to_child_mut(),
            ],
            Self::RequestError { request, .. } => vec![request.to_child_mut()],
        }
    }
}

impl Draw<ExchangePaneContentProps> for ExchangePaneContent {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ExchangePaneContentProps,
        metadata: DrawMetadata,
    ) {
        let area = metadata.area();
        match self {
            Self::Building => {
                frame.render_widget("Initializing request...", area)
            }
            Self::BuildError { error } => frame.render_widget(error, area),
            Self::Loading { request } => match props.selected_tab {
                Tab::Request => request.draw(frame, (), area, true),
                Tab::Body | Tab::Headers => {
                    frame.render_widget("Loading...", area)
                }
            },
            // Can't show cancelled request here because we might've cancelled
            // during the build
            Self::Cancelled => frame.render_widget("Request cancelled", area),
            Self::Response {
                request,
                response_body,
                response_headers,
            } => match props.selected_tab {
                Tab::Request => request.draw(frame, (), area, true),
                Tab::Body => response_body.draw(frame, (), area, true),
                Tab::Headers => response_headers.draw(frame, (), area, true),
            },
            Self::RequestError { request, error } => match props.selected_tab {
                Tab::Request => request.draw(frame, (), area, true),
                Tab::Body | Tab::Headers => frame.render_widget(error, area),
            },
        }
    }
}
