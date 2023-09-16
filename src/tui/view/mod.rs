mod component;
mod theme;

use crate::{
    http::{Response, ResponseState},
    tui::{
        state::{AppState, PrimaryPane, RequestTab, ResponseTab},
        view::{
            component::{
                BlockComponent, Component, ListComponent, TabComponent, ToText,
            },
            theme::Theme,
        },
    },
};
use ratatui::{prelude::*, widgets::*};
use std::{fmt::Debug, io::Stdout};

type Frame<'a> = ratatui::Frame<'a, CrosstermBackend<Stdout>>;

/// Container for rendering the UI
#[derive(Debug, Default)]
pub struct Renderer {
    theme: Theme,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn draw_main(&self, f: &mut Frame, state: &mut AppState) {
        // Create layout
        let [main_chunk, help_chunk] = layout(
            f.size(),
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );
        let [left_chunk, right_chunk] = layout(
            main_chunk,
            Direction::Horizontal,
            [Constraint::Max(40), Constraint::Percentage(50)],
        );

        let [environments_chunk, recipes_chunk] = layout(
            left_chunk,
            Direction::Vertical,
            [Constraint::Max(16), Constraint::Min(0)],
        );

        let [request_chunk, response_chunk] = layout(
            right_chunk,
            Direction::Vertical,
            [Constraint::Percentage(50), Constraint::Percentage(50)],
        );

        HelpText.draw(self, f, help_chunk, state);
        EnvironmentListPane.draw(self, f, environments_chunk, state);
        RecipeListPane.draw(self, f, recipes_chunk, state);
        RequestPane.draw(self, f, request_chunk, state);
        ResponsePane.draw(self, f, response_chunk, state);

        // TODO move this into a component or something
        if let Some(error) = &state.error {
            let block = Block::default().title("Error").borders(Borders::ALL);
            let area = centered_rect(60, 20, f.size());
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(format!("{error:#}"))
                    .block(block)
                    .wrap(Wrap::default()),
                area,
            );
        }
    }
}

/// Helper for building a layout with a fixed number of constraints
fn layout<const N: usize>(
    area: Rect,
    direction: Direction,
    constraints: [Constraint; N],
) -> [Rect; N] {
    Layout::default()
        .direction(direction)
        .constraints(constraints)
        .split(area)
        .as_ref()
        .try_into()
        // Should be unreachable
        .expect("Chunk length does not match constraint length")
}

/// Something that can be drawn into a frame
pub trait Draw {
    fn draw(
        &self,
        renderer: &Renderer,
        f: &mut Frame,
        chunk: Rect,
        state: &mut AppState,
    );
}

pub struct EnvironmentListPane;

impl Draw for EnvironmentListPane {
    fn draw(
        &self,
        renderer: &Renderer,
        f: &mut Frame,
        chunk: Rect,
        state: &mut AppState,
    ) {
        let pane_kind = PrimaryPane::EnvironmentList;
        let list = ListComponent {
            block: BlockComponent {
                title: pane_kind.to_string(),
                is_focused: state.focused_pane.is_selected(&pane_kind),
            },
            list: &state.environments,
        }
        .render(renderer);
        f.render_stateful_widget(list, chunk, &mut state.environments.state)
    }
}

pub struct RecipeListPane;

impl Draw for RecipeListPane {
    fn draw(
        &self,
        renderer: &Renderer,
        f: &mut Frame,
        chunk: Rect,
        state: &mut AppState,
    ) {
        let pane_kind = PrimaryPane::RecipeList;
        let list = ListComponent {
            block: BlockComponent {
                title: pane_kind.to_string(),
                is_focused: state.focused_pane.is_selected(&pane_kind),
            },
            list: &state.recipes,
        }
        .render(renderer);
        f.render_stateful_widget(list, chunk, &mut state.recipes.state)
    }
}

pub struct RequestPane;

impl Draw for RequestPane {
    fn draw(
        &self,
        renderer: &Renderer,
        f: &mut Frame,
        chunk: Rect,
        state: &mut AppState,
    ) {
        if let Some(recipe) = state.recipes.selected() {
            // Render outermost block
            let pane_kind = PrimaryPane::Request;
            let block = BlockComponent {
                title: pane_kind.to_string(),
                is_focused: state.focused_pane.is_selected(&pane_kind),
            }
            .render(renderer);
            let inner_chunk = block.inner(chunk);
            f.render_widget(block, chunk);
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
            f.render_widget(
                Paragraph::new(format!("{} {}", recipe.method, recipe.url)),
                url_chunk,
            );

            // Navigation tabs
            let tabs = TabComponent {
                tabs: &state.request_tab,
            }
            .render(renderer);
            f.render_widget(tabs, tabs_chunk);

            // Request content
            let text: Text = match state.request_tab.selected() {
                RequestTab::Body => recipe
                    .body
                    .as_ref()
                    .map(|b| b.to_string())
                    .unwrap_or_default()
                    .into(),
                RequestTab::Query => recipe.query.to_text(),
                RequestTab::Headers => recipe.headers.to_text(),
            };
            f.render_widget(Paragraph::new(text), content_chunk);
        }
    }
}

pub struct ResponsePane;

impl Draw for ResponsePane {
    fn draw(
        &self,
        renderer: &Renderer,
        f: &mut Frame,
        chunk: Rect,
        state: &mut AppState,
    ) {
        // Render outermost block
        let pane_kind = PrimaryPane::Response;
        let block = BlockComponent {
            title: pane_kind.to_string(),
            is_focused: state.focused_pane.is_selected(&pane_kind),
        }
        .render(renderer);
        let inner_chunk = block.inner(chunk);
        f.render_widget(block, chunk);
        let [tabs_chunk, content_chunk] = layout(
            inner_chunk,
            Direction::Vertical,
            [Constraint::Length(1), Constraint::Min(0)],
        );

        // Navigation tabs
        let tabs = TabComponent {
            tabs: &state.response_tab,
        }
        .render(renderer);
        f.render_widget(tabs, tabs_chunk);

        // Response content/metadata
        let get_text = || -> Option<Text> {
            // Check if a request is running/complete
            let response = state.get_response()?;
            match response {
                ResponseState::Loading => Some("Loading...".into()),
                ResponseState::Success(Response {
                    headers, content, ..
                }) => {
                    // TODO show status
                    Some(match state.response_tab.selected() {
                        ResponseTab::Body => content.clone().into(),
                        ResponseTab::Headers => headers.to_text(),
                    })
                }
                ResponseState::Error(error) => Some(error.to_string().into()),
            }
        };
        f.render_widget(
            Paragraph::new(get_text().unwrap_or_default()),
            content_chunk,
        );
    }
}

pub struct HelpText;

impl Draw for HelpText {
    fn draw(&self, _: &Renderer, f: &mut Frame, chunk: Rect, _: &mut AppState) {
        let block = Block::default();
        let paragraph = Paragraph::new(
            "<q> Exit | <space> Send request | <tab>/<shift+tab> Switch panes \
            | ←↑↓→  Navigate",
        )
        .block(block);
        f.render_widget(paragraph, chunk);
    }
}

/// helper function to create a centered rect using up certain percentage of the
/// available rect `r`
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}
