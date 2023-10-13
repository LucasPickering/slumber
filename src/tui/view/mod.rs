mod brick;
pub mod component;
mod theme;

use crate::tui::{
    state::AppState,
    view::{
        component::{
            primary::{
                ProfileListPane, RecipeListPane, RequestPane, ResponsePane,
            },
            ErrorPopup, HelpText, NotificationText,
        },
        theme::Theme,
    },
};
use ratatui::prelude::*;
use std::io::Stdout;

type Frame<'a> = ratatui::Frame<'a, CrosstermBackend<Stdout>>;

/// Primary entrypoint for the view
pub struct View;

impl View {
    /// Draw the whole TUI
    pub fn draw(state: &AppState, context: &RenderContext, f: &mut Frame) {
        Draw::draw(&Self, context, state, f, f.size())
    }
}

impl Draw for View {
    type State = AppState;

    fn draw(
        &self,
        context: &RenderContext,
        state: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Create layout
        let [main_chunk, footer_chunk] = layout(
            chunk,
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );
        let [left_chunk, right_chunk] = layout(
            main_chunk,
            Direction::Horizontal,
            [Constraint::Max(40), Constraint::Percentage(50)],
        );

        let [profiles_chunk, recipes_chunk] = layout(
            left_chunk,
            Direction::Vertical,
            [Constraint::Max(16), Constraint::Min(0)],
        );

        let [request_chunk, response_chunk] = layout(
            right_chunk,
            Direction::Vertical,
            [Constraint::Percentage(50), Constraint::Percentage(50)],
        );

        // Main panes
        ProfileListPane.draw(context, state, frame, profiles_chunk);
        RecipeListPane.draw(context, state, frame, recipes_chunk);
        RequestPane.draw(context, state, frame, request_chunk);
        ResponsePane.draw(context, state, frame, response_chunk);

        // Footer
        match state.notification() {
            Some(notification) => NotificationText.draw(
                context,
                notification,
                frame,
                footer_chunk,
            ),
            None => HelpText.draw(context, state, frame, footer_chunk),
        }

        // Render popups last so they go on top
        if let Some(error) = state.error() {
            ErrorPopup.draw(context, error, frame, frame.size());
        }
    }
}

/// Container for rendering the UI
#[derive(Debug, Default)]
pub struct RenderContext {
    theme: Theme,
}

impl RenderContext {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Something that can be drawn into a frame. Generally implementors of this
/// will be empty structs, since [Draw::draw] provides all the context needed
/// to render. You might be tempted to break the state apart and store in each
/// implementor only what it needs, but that gets tricky because the input
/// handler needs to be able to construct these directly. You could try having
/// long-lived components, but then you have to retain references to state
/// across the message phase which would require interior mutability.
pub trait Draw {
    type State;

    fn draw(
        &self,
        context: &RenderContext,
        state: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    );
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
