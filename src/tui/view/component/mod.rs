//! The building blocks of the view

pub mod primary;

use crate::tui::{
    input::{Action, InputManager, InputTarget, Mutator, OutcomeBinding},
    state::{AppState, Notification},
    view::{
        brick::{Brick, ButtonBrick, ToSpan},
        centered_rect, layout, Draw, Frame, RenderContext,
    },
};
use itertools::Itertools;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub struct ErrorPopup;

impl Draw for ErrorPopup {
    type State = anyhow::Error;

    fn draw(
        &self,
        context: &RenderContext,
        error: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Grab a spot in the middle of the screen
        let chunk = centered_rect(60, 20, chunk);
        let block = Block::default().title("Error").borders(Borders::ALL);
        let [content_chunk, footer_chunk] = layout(
            block.inner(chunk),
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );

        frame.render_widget(Clear, chunk);
        frame.render_widget(block, chunk);
        frame.render_widget(
            Paragraph::new(
                error
                    .chain()
                    .enumerate()
                    .map(|(i, err)| {
                        // Add indentation to parent errors
                        format!("{}{err}", if i > 0 { "  " } else { "" }).into()
                    })
                    .collect::<Vec<Line>>(),
            )
            .wrap(Wrap::default()),
            content_chunk,
        );

        // Prompt the user to get out of here
        frame.render_widget(
            Paragraph::new(
                ButtonBrick {
                    text: "OK",
                    is_highlighted: true,
                }
                .to_brick(context),
            )
            .alignment(Alignment::Center),
            footer_chunk,
        );
    }
}

impl InputTarget for ErrorPopup {
    fn actions(&self, _: &AppState) -> Vec<OutcomeBinding> {
        let clear_error: Mutator = &|state| state.clear_error();
        vec![
            OutcomeBinding::new(Action::Interact, clear_error),
            OutcomeBinding::new(Action::Close, clear_error),
        ]
    }
}

pub struct HelpText;

impl Draw for HelpText {
    type State = AppState;

    fn draw(
        &self,
        _: &RenderContext,
        state: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Find all available input bindings
        let input_manager = InputManager::instance();
        let available_actions = input_manager.actions(state);
        let key_binding_text = available_actions
            .into_iter()
            .filter_map(|app| input_manager.binding(app.action))
            .join(" | ");
        frame.render_widget(Paragraph::new(key_binding_text), chunk);
    }
}

pub struct NotificationText;

impl Draw for NotificationText {
    type State = Notification;

    fn draw(
        &self,
        _: &RenderContext,
        notification: &Self::State,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        frame.render_widget(Paragraph::new(notification.to_span()), chunk);
    }
}
