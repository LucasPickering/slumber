//! Helper structs and functions for building components

pub mod highlight;
pub mod persistence;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use slumber_core::template::{Prompt, PromptChannel, Prompter, Select};

/// A data structure for representation a yes/no confirmation. This is similar
/// to [Prompt], but it only asks a yes/no question.
#[derive(Debug)]
pub struct Confirm {
    /// Question to ask the user
    pub message: String,
    /// A channel to pass back the user's response
    pub channel: PromptChannel<bool>,
}

/// A prompter that returns a static value; used for template previews, where
/// user interaction isn't possible
#[derive(Debug)]
pub struct PreviewPrompter;

impl Prompter for PreviewPrompter {
    fn prompt(&self, prompt: Prompt) {
        prompt.channel.respond("<prompt>".into())
    }

    fn select(&self, select: Select) {
        select.channel.respond("<select>".into())
    }
}

/// Created a rectangle centered on the given `Rect`.
pub fn centered_rect(
    width: Constraint,
    height: Constraint,
    rect: Rect,
) -> Rect {
    fn buffer(constraint: Constraint, full_size: u16) -> Constraint {
        match constraint {
            Constraint::Percentage(percent) => {
                Constraint::Percentage((100 - percent) / 2)
            }
            Constraint::Length(length) => {
                Constraint::Length((full_size.saturating_sub(length)) / 2)
            }
            // Implement these as needed
            _ => unimplemented!("Other center constraints unsupported"),
        }
    }

    let buffer_x = buffer(width, rect.width);
    let buffer_y = buffer(height, rect.height);
    let columns = Layout::default()
        .direction(Direction::Vertical)
        .constraints([buffer_y, height, buffer_y].as_ref())
        .split(rect);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([buffer_x, width, buffer_x].as_ref())
        .split(columns[1])[1]
}
