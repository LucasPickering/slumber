use crate::{
    collection::RecipeNode,
    http::Request,
    tui::{
        context::TuiContext,
        input::Action,
        view::{
            common::{tabs::Tabs, Pane},
            component::{
                primary::PrimaryPane,
                request_view::{RequestView, RequestViewProps},
                response_view::{
                    ResponseBodyView, ResponseBodyViewProps,
                    ResponseHeadersView, ResponseHeadersViewProps,
                },
                Component,
            },
            draw::{Draw, DrawMetadata, Generate},
            event::{Event, EventHandler, Update},
            state::{fixed_select::FixedSelect, persistence::PersistentKey},
            RequestState, ViewContext,
        },
    },
    util::doc_link,
};
use derive_more::Display;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    text::{Line, Text},
    widgets::block::Title,
    Frame,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display for a request/response record. This allows the user to switch
/// between request and response. Parent is responsible for switching between
/// tabs, because switching is done by hotkey and we can't see hotkeys if the
/// pane isn't selected.
#[derive(Debug)]
pub struct RecordPane {
    tabs: Component<Tabs<Tab>>,
    request: Component<RequestView>,
    response_headers: Component<ResponseHeadersView>,
    response_body: Component<ResponseBodyView>,
}

pub struct RecordPaneProps<'a> {
    /// Selected recipe OR folder. Used to decide what placeholder to show
    pub selected_recipe_node: Option<&'a RecipeNode>,
    pub request_state: Option<&'a RequestState>,
}

impl Default for RecordPane {
    fn default() -> Self {
        Self {
            tabs: Tabs::new(PersistentKey::RecordTab).into(),
            request: Default::default(),
            response_headers: Default::default(),
            response_body: Default::default(),
        }
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
impl FixedSelect for Tab {}

impl EventHandler for RecordPane {
    fn update(&mut self, event: Event) -> Update {
        match event.action() {
            Some(Action::LeftClick) => {
                ViewContext::push_event(Event::new_local(PrimaryPane::Record));
            }
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![
            self.request.as_child(),
            self.response_body.as_child(),
            // Tabs last so the children get priority
            self.tabs.as_child(),
        ]
    }
}

impl<'a> Draw<RecordPaneProps<'a>> for RecordPane {
    fn draw(
        &self,
        frame: &mut Frame,
        props: RecordPaneProps<'a>,
        metadata: DrawMetadata,
    ) {
        let input_engine = &TuiContext::get().input_engine;
        let title =
            input_engine.add_hint("Request / Response", Action::SelectResponse);
        let mut block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        // If a recipe is selected, history is available so show the hint
        if matches!(props.selected_recipe_node, Some(RecipeNode::Recipe(_))) {
            let text = input_engine.add_hint("History", Action::History);
            block = block.title(Title::from(text).alignment(Alignment::Right));
        }
        frame.render_widget(&block, metadata.area());
        let area = block.inner(metadata.area());

        // Empty states
        match props.selected_recipe_node {
            None => {
                frame.render_widget(
                    Text::from(vec![
                        "No recipes defined; add one to your collection".into(),
                        doc_link("api/request_collection/request_recipe")
                            .into(),
                    ]),
                    area,
                );
                return;
            }
            Some(RecipeNode::Folder { .. }) => {
                frame.render_widget(
                    "Select a recipe to see its request history",
                    area,
                );
                return;
            }
            Some(RecipeNode::Recipe { .. }) => {}
        }

        // Split out the areas we *may* need
        let [metadata_area, tabs_area, content_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);

        // Draw whatever metadata is available
        if let Some(metadata) =
            props.request_state.and_then(RequestState::request_metadata)
        {
            frame.render_widget(
                Line::from(vec![
                    metadata.start_time.generate(),
                    " / ".into(),
                    metadata.duration.generate(),
                ]),
                metadata_area,
            );
        }
        if let Some(metadata) = props
            .request_state
            .and_then(RequestState::response_metadata)
        {
            frame.render_widget(
                Line::from(vec![
                    metadata.status.generate(),
                    " ".into(),
                    metadata.size.to_string_as(false).into(),
                ])
                .alignment(Alignment::Right),
                metadata_area,
            );
        }

        // Render request/response based on state. Lambas help with code dupe
        let selected_tab = self.tabs.data().selected();
        let render_tabs = |frame| self.tabs.draw(frame, (), tabs_area, true);
        let render_request = |frame, request: &Arc<Request>| {
            self.request.draw(
                frame,
                RequestViewProps {
                    request: Arc::clone(request),
                },
                content_area,
                true,
            )
        };
        match props.request_state {
            None => frame.render_widget(
                "No request history for this recipe & profile",
                area,
            ),
            Some(RequestState::Building { .. }) => {
                frame.render_widget("Initializing request...", area)
            }
            Some(RequestState::BuildError { error, .. }) => {
                frame.render_widget(error.generate(), area)
            }
            Some(RequestState::Loading { request, .. }) => {
                render_tabs(frame);
                match selected_tab {
                    Tab::Request => render_request(frame, request),
                    Tab::Body | Tab::Headers => {
                        frame.render_widget("Loading...", content_area)
                    }
                }
            }
            Some(RequestState::Response { record }) => {
                render_tabs(frame);
                match selected_tab {
                    Tab::Request => render_request(frame, &record.request),
                    Tab::Body => {
                        // Don't draw body if empty, so we don't have to set
                        // up state, and don't offer impossible actions
                        if !record.response.body.bytes().is_empty() {
                            self.response_body.draw(
                                frame,
                                ResponseBodyViewProps {
                                    request_id: record.id,
                                    recipe_id: &record.request.recipe_id,
                                    response: Arc::clone(&record.response),
                                },
                                content_area,
                                true,
                            );
                        } else {
                            frame.render_widget(
                                "No response body",
                                content_area,
                            );
                        }
                    }
                    Tab::Headers => self.response_headers.draw(
                        frame,
                        ResponseHeadersViewProps {
                            response: &record.response,
                        },
                        content_area,
                        true,
                    ),
                }
            }
            Some(RequestState::RequestError { error }) => {
                render_tabs(frame);
                match selected_tab {
                    Tab::Request => render_request(frame, &error.request),
                    Tab::Body | Tab::Headers => {
                        frame.render_widget(error.generate(), content_area)
                    }
                }
            }
        }
    }
}
