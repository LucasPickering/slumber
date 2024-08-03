use crate::{
    context::TuiContext,
    view::{
        common::{tabs::Tabs, Pane},
        component::{
            primary::PrimaryPane,
            request_view::{RequestView, RequestViewProps},
            response_view::{
                ResponseBodyView, ResponseBodyViewProps, ResponseHeadersView,
                ResponseHeadersViewProps,
            },
            Component,
        },
        context::PersistedLazy,
        draw::{Draw, DrawMetadata, Generate},
        event::{Event, EventHandler, Update},
        RequestState, ViewContext,
    },
};
use derive_more::Display;
use persisted::SingletonKey;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    text::Line,
    widgets::block::Title,
    Frame,
};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::{
    collection::RecipeNodeDiscriminants, http::RequestRecord,
    util::format_byte_size,
};
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display for a request/response exchange. This allows the user to switch
/// between request and response. Parent is responsible for switching between
/// tabs, because switching is done by hotkey and we can't see hotkeys if the
/// pane isn't selected.
#[derive(Debug, Default)]
pub struct ExchangePane {
    tabs: Component<PersistedLazy<SingletonKey<Tab>, Tabs<Tab>>>,
    request: Component<RequestView>,
    response_headers: Component<ResponseHeadersView>,
    response_body: Component<ResponseBodyView>,
}

pub struct ExchangePaneProps<'a> {
    /// Do we have a recipe, folder, or neither selected? Used to determine
    /// placeholder
    pub selected_recipe_kind: Option<RecipeNodeDiscriminants>,
    pub request_state: Option<&'a RequestState>,
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

impl EventHandler for ExchangePane {
    fn update(&mut self, event: Event) -> Update {
        match event.action() {
            Some(Action::LeftClick) => {
                ViewContext::push_event(Event::new_local(
                    PrimaryPane::Exchange,
                ));
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

impl<'a> Draw<ExchangePaneProps<'a>> for ExchangePane {
    fn draw(
        &self,
        frame: &mut Frame,
        props: ExchangePaneProps<'a>,
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
        if matches!(
            props.selected_recipe_kind,
            Some(RecipeNodeDiscriminants::Recipe)
        ) {
            let text = input_engine.add_hint("History", Action::History);
            block = block.title(Title::from(text).alignment(Alignment::Right));
        }
        frame.render_widget(&block, metadata.area());
        let area = block.inner(metadata.area());

        // Empty states
        match props.selected_recipe_kind {
            None => {
                return;
            }
            Some(RecipeNodeDiscriminants::Folder) => {
                frame.render_widget(
                    "Select a recipe to see its request history",
                    area,
                );
                return;
            }
            Some(RecipeNodeDiscriminants::Recipe) => {}
        }

        // Split out the areas we *may* need
        let [metadata_area, tabs_area, content_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);

        // Draw timing metadata
        if let Some(metadata) =
            props.request_state.map(RequestState::request_metadata)
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
        // Draw response metadata
        if let Some(metadata) = props
            .request_state
            .and_then(RequestState::response_metadata)
        {
            frame.render_widget(
                Line::from(vec![
                    metadata.status.generate(),
                    " ".into(),
                    format_byte_size(metadata.size).into(),
                ])
                .alignment(Alignment::Right),
                metadata_area,
            );
        }

        // Render request/response based on state. Lambdas help with code dupe
        let selected_tab = self.tabs.data().selected();
        let render_tabs = |frame| self.tabs.draw(frame, (), tabs_area, true);
        let render_request = |frame, request: &Arc<RequestRecord>| {
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
                frame.render_widget("Initializing request...", content_area)
            }
            Some(RequestState::BuildError { error, .. }) => {
                frame.render_widget(error.generate(), content_area)
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
            Some(RequestState::Response { exchange }) => {
                render_tabs(frame);
                match selected_tab {
                    Tab::Request => render_request(frame, &exchange.request),
                    Tab::Body => {
                        // Don't draw body if empty, so we don't have to set
                        // up state, and don't offer impossible actions
                        if !exchange.response.body.bytes().is_empty() {
                            self.response_body.draw(
                                frame,
                                ResponseBodyViewProps {
                                    request_id: exchange.id,
                                    recipe_id: &exchange.request.recipe_id,
                                    response: Arc::clone(&exchange.response),
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
                            response: &exchange.response,
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
