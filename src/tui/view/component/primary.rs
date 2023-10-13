//! Primary pane components

use crate::tui::{
    input::{Action, InputTarget, OutcomeBinding},
    state::{
        AppState, Message, PrimaryPane, RequestState, RequestTab, ResponseTab,
    },
    view::{
        brick::{BlockBrick, Brick, ListBrick, TabBrick, ToSpan, ToText},
        component::Draw,
        layout, Frame, RenderContext,
    },
};
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
};

pub struct ProfileListPane;

impl Draw for ProfileListPane {
    type State = AppState;

    fn draw(
        &self,
        context: &RenderContext,
        state: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let pane_kind = PrimaryPane::ProfileList;
        let list = ListBrick {
            block: BlockBrick {
                title: pane_kind.to_string(),
                is_focused: state.selected_pane().is_selected(&pane_kind),
            },
            list: state.profiles(),
        }
        .to_brick(context);
        frame.render_stateful_widget(
            list,
            chunk,
            &mut state.profiles().state_mut(),
        )
    }
}

impl InputTarget for ProfileListPane {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        vec![
            OutcomeBinding::new(Action::FocusPrevious, &|state| {
                state.selected_pane_mut().previous()
            }),
            OutcomeBinding::new(Action::FocusNext, &|state| {
                state.selected_pane_mut().next()
            }),
            OutcomeBinding::new(Action::Up, &|state| {
                state.profiles_mut().previous()
            }),
            OutcomeBinding::new(Action::Down, &|state| {
                state.profiles_mut().next()
            }),
        ]
    }
}

pub struct RecipeListPane;

impl Draw for RecipeListPane {
    type State = AppState;

    fn draw(
        &self,
        context: &RenderContext,
        state: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let pane_kind = PrimaryPane::RecipeList;
        let list = ListBrick {
            block: BlockBrick {
                title: pane_kind.to_string(),
                is_focused: state.selected_pane().is_selected(&pane_kind),
            },
            list: state.recipes(),
        }
        .to_brick(context);
        frame.render_stateful_widget(
            list,
            chunk,
            &mut state.recipes().state_mut(),
        )
    }
}

impl InputTarget for RecipeListPane {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        vec![
            OutcomeBinding::new(Action::FocusPrevious, &|state| {
                state.selected_pane_mut().previous()
            }),
            OutcomeBinding::new(Action::FocusNext, &|state| {
                state.selected_pane_mut().next()
            }),
            OutcomeBinding::new(Action::Up, &|state| {
                state.recipes_mut().previous()
            }),
            OutcomeBinding::new(Action::Down, &|state| {
                state.recipes_mut().next()
            }),
            OutcomeBinding::new(Action::Interact, &|state| {
                state.messages_tx().send(Message::HttpSendRequest)
            }),
        ]
    }
}

pub struct RequestPane;

impl Draw for RequestPane {
    type State = AppState;

    fn draw(
        &self,
        context: &RenderContext,
        state: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        if let Some(recipe) = state.recipes().selected() {
            // Render outermost block
            let pane_kind = PrimaryPane::Request;
            let block = BlockBrick {
                title: pane_kind.to_string(),
                is_focused: state.selected_pane().is_selected(&pane_kind),
            }
            .to_brick(context);
            let inner_chunk = block.inner(chunk);
            frame.render_widget(block, chunk);
            let [url_chunk, tabs_chunk, content_chunk] = layout(
                inner_chunk,
                Direction::Vertical,
                [
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ],
            );

            // URL
            frame.render_widget(
                Paragraph::new(format!("{} {}", recipe.method, recipe.url)),
                url_chunk,
            );

            // Navigation tabs
            let tabs = TabBrick {
                tabs: state.request_tab(),
            }
            .to_brick(context);
            frame.render_widget(tabs, tabs_chunk);

            // Request content
            let text: Text = match state.request_tab().selected() {
                RequestTab::Body => recipe
                    .body
                    .as_ref()
                    .map(|b| b.to_string())
                    .unwrap_or_default()
                    .into(),
                RequestTab::Query => recipe.query.to_text(),
                RequestTab::Headers => recipe.headers.to_text(),
            };
            frame.render_widget(Paragraph::new(text), content_chunk);
        }
    }
}

impl InputTarget for RequestPane {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        vec![
            OutcomeBinding::new(Action::FocusPrevious, &|state| {
                state.selected_pane_mut().previous()
            }),
            OutcomeBinding::new(Action::FocusNext, &|state| {
                state.selected_pane_mut().next()
            }),
            OutcomeBinding::new(Action::Left, &|state| {
                state.request_tab_mut().previous()
            }),
            OutcomeBinding::new(Action::Right, &|state| {
                state.request_tab_mut().next()
            }),
        ]
    }
}

pub struct ResponsePane;

impl Draw for ResponsePane {
    type State = AppState;

    fn draw(
        &self,
        context: &RenderContext,
        state: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Render outermost block
        let pane_kind = PrimaryPane::Response;
        let block = BlockBrick {
            title: pane_kind.to_string(),
            is_focused: state.selected_pane().is_selected(&pane_kind),
        }
        .to_brick(context);
        let inner_chunk = block.inner(chunk);
        frame.render_widget(block, chunk);

        // Don't render anything else unless we have a request state
        if let Some(request_state) = state.active_request() {
            let [header_chunk, content_chunk] = layout(
                inner_chunk,
                Direction::Vertical,
                [Constraint::Length(1), Constraint::Min(0)],
            );
            let [header_left_chunk, header_right_chunk] = layout(
                header_chunk,
                Direction::Horizontal,
                [Constraint::Length(20), Constraint::Min(0)],
            );

            // Time-related data
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    request_state.start_time().to_span(),
                    " / ".into(),
                    request_state.duration().to_span(),
                ]))
                .alignment(Alignment::Right),
                header_right_chunk,
            );

            match &request_state {
                RequestState::Loading { .. } => {
                    frame.render_widget(
                        Paragraph::new("Loading..."),
                        header_left_chunk,
                    );
                }

                RequestState::Response {
                    record,
                    pretty_body,
                } => {
                    let response = &record.response;
                    // Status code
                    frame.render_widget(
                        Paragraph::new(response.status.to_string()),
                        header_left_chunk,
                    );

                    // Split the main chunk again to allow tabs
                    let [tabs_chunk, content_chunk] = layout(
                        content_chunk,
                        Direction::Vertical,
                        [Constraint::Length(1), Constraint::Min(0)],
                    );

                    // Navigation tabs
                    let tabs = TabBrick {
                        tabs: state.response_tab(),
                    }
                    .to_brick(context);
                    frame.render_widget(tabs, tabs_chunk);

                    // Main content for the response
                    let tab_text = match state.response_tab().selected() {
                        // Render the pretty body if it's available, otherwise
                        // fall back to the regular one
                        ResponseTab::Body => pretty_body
                            .as_deref()
                            .unwrap_or(response.body.as_str())
                            .into(),
                        ResponseTab::Headers => response.headers.to_text(),
                    };
                    frame
                        .render_widget(Paragraph::new(tab_text), content_chunk);
                }

                RequestState::Error { error, .. } => {
                    frame.render_widget(
                        Paragraph::new(error.to_string()).wrap(Wrap::default()),
                        content_chunk,
                    );
                }
            }
        }
    }
}

impl InputTarget for ResponsePane {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        vec![
            OutcomeBinding::new(Action::FocusPrevious, &|state| {
                state.selected_pane_mut().previous()
            }),
            OutcomeBinding::new(Action::FocusNext, &|state| {
                state.selected_pane_mut().next()
            }),
            OutcomeBinding::new(Action::Left, &|state| {
                state.response_tab_mut().previous()
            }),
            OutcomeBinding::new(Action::Right, &|state| {
                state.response_tab_mut().next()
            }),
        ]
    }
}
