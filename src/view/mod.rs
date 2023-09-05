mod component;
mod theme;

use crate::{
    http::ResponseState,
    input::InputHandler,
    state::AppState,
    view::{
        component::{BlockComponent, Component, ListComponent},
        theme::Theme,
    },
};
use ratatui::{prelude::*, widgets::*};
use std::{any::Any, fmt::Debug, io::Stdout, ops::Deref};

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
            [Constraint::Percentage(100), Constraint::Min(1)],
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

static PANE_TAB_ORDER: &[&dyn Pane] = &[
    &EnvironmentListPane,
    &RecipeListPane,
    &RequestPane,
    &ResponsePane,
];

/// A pane is a top-level UI element, which can hold focus and handle input
/// events. Panes can be cycled through by the user, and focus is mutually
/// exclusive between them. Panes of the same type are considered equal: there
/// can be multiple instances of the same Pane implementor, but they refer to
/// the same piece of the UI.
pub trait Pane: Any + Sync + InputHandler {
    /// Convert a reference into a boxed value. Feels icky but also it works
    fn clone_kinda(&self) -> Box<dyn Pane>;

    /// Is this the same pane as the given one? Panes are singleton-esque, so
    /// this just needs to check that the types are the same
    fn equals(&self, other: &dyn Pane) -> bool {
        self.type_id() == other.type_id()
    }

    /// Get the tab index of this pane
    fn tab_index(&self) -> usize {
        // Search the global list of panes
        PANE_TAB_ORDER
            .iter()
            .position(|p| self.equals(*p))
            .expect("Pane is not defined in tab order list")
    }

    /// Get the previous pane in the tab sequence
    fn previous(&self) -> Box<dyn Pane> {
        // Turn underflow into custom wrapping
        let new_index = self
            .tab_index()
            .checked_sub(1)
            .unwrap_or(PANE_TAB_ORDER.len() - 1);
        PANE_TAB_ORDER[new_index].clone_kinda()
    }

    /// Get the next pane in the tab sequence
    fn next(&self) -> Box<dyn Pane> {
        // Wrap to beginning, no need to worry about overflow here
        let new_index = (self.tab_index() + 1) % PANE_TAB_ORDER.len();
        PANE_TAB_ORDER[new_index].clone_kinda()
    }
}

#[derive(Debug)]
pub struct EnvironmentListPane;

impl Draw for EnvironmentListPane {
    fn draw(
        &self,
        renderer: &Renderer,
        f: &mut Frame,
        chunk: Rect,
        state: &mut AppState,
    ) {
        let list = ListComponent {
            block: BlockComponent {
                title: "Environments",
                is_focused: state.is_focused(self),
            },
            list: &state.environments,
        }
        .render(renderer);
        f.render_stateful_widget(list, chunk, &mut state.environments.state)
    }
}

impl Pane for EnvironmentListPane {
    fn clone_kinda(&self) -> Box<dyn Pane> {
        Box::new(Self)
    }
}

#[derive(Debug)]
pub struct RecipeListPane;

impl Draw for RecipeListPane {
    fn draw(
        &self,
        renderer: &Renderer,
        f: &mut Frame,
        chunk: Rect,
        state: &mut AppState,
    ) {
        let list = ListComponent {
            block: BlockComponent {
                title: "Recipes",
                is_focused: state.is_focused(self),
            },
            list: &state.recipes,
        }
        .render(renderer);
        f.render_stateful_widget(list, chunk, &mut state.recipes.state)
    }
}

impl Pane for RecipeListPane {
    fn clone_kinda(&self) -> Box<dyn Pane> {
        Box::new(Self)
    }
}

#[derive(Debug)]
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
            let block = BlockComponent {
                title: "Request",
                is_focused: state.is_focused(self),
            }
            .render(renderer);

            let mut lines: Vec<Line> =
                vec![format!("{} {}", recipe.method, recipe.url).into()];

            // Add request body if present
            if let Some(body) = &recipe.body {
                lines.extend(body.lines().map(Line::from));
            }

            let paragraph = Paragraph::new(lines).block(block);
            f.render_widget(paragraph, chunk);
        }
    }
}

impl Pane for RequestPane {
    fn clone_kinda(&self) -> Box<dyn Pane> {
        Box::new(Self)
    }
}

#[derive(Debug)]
pub struct ResponsePane;

impl Draw for ResponsePane {
    fn draw(
        &self,
        renderer: &Renderer,
        f: &mut Frame,
        chunk: Rect,
        state: &mut AppState,
    ) {
        let block = BlockComponent {
            title: "Response",
            is_focused: state.is_focused(self),
        }
        .render(renderer);

        let get_text = || -> Option<String> {
            // Check if a request is running/complete
            let request = state.active_request.as_ref()?;
            // Try to access the response. If it's locked, don't block
            let response = request.response.try_read().ok()?;
            match response.deref() {
                // Request hasn't launched yet
                ResponseState::None => None,
                ResponseState::Loading => Some("Loading...".into()),
                ResponseState::Complete { content, .. } => {
                    // TODO show status/headers somehow
                    Some(content.clone())
                }
                ResponseState::Error(error) => Some(error.to_string()),
            }
        };

        let paragraph =
            Paragraph::new(get_text().unwrap_or_default()).block(block);
        f.render_widget(paragraph, chunk);
    }
}

impl Pane for ResponsePane {
    fn clone_kinda(&self) -> Box<dyn Pane> {
        Box::new(Self)
    }
}

pub struct HelpText;

impl Draw for HelpText {
    fn draw(&self, _: &Renderer, f: &mut Frame, chunk: Rect, _: &mut AppState) {
        let block = Block::default();
        let paragraph = Paragraph::new(
            "<q> Exit | <space> Send request | <tab>/<shift+tab> Switch panes \
            | ↑↓ Navigate",
        )
        .block(block);
        f.render_widget(paragraph, chunk);
    }
}
