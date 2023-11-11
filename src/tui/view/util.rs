//! Helper structs and functions for building components

use crate::{
    collection::{Profile, RequestRecipe},
    http::{RequestBuildError, RequestError},
    template::{Prompt, Prompter},
    tui::view::{
        state::{Notification, StatefulList},
        DrawContext,
    },
};
use chrono::{DateTime, Duration, Local, Utc};
use itertools::Itertools;
use ratatui::{
    prelude::*,
    text::{Span, Text},
    widgets::{Block, Borders, List, ListItem},
};
use reqwest::header::HeaderValue;

/// A helper for building a UI. It can be converted into some UI element to be
/// drawn.
pub trait ToTui {
    type Output<'this>
    where
        Self: 'this;

    /// Build a UI element
    fn to_tui<'a>(&'a self, context: &DrawContext) -> Self::Output<'a>;
}

/// A container with a title and border
pub struct BlockBrick {
    pub title: String,
    pub is_focused: bool,
}

impl ToTui for BlockBrick {
    type Output<'this> = Block<'this> where Self: 'this;

    fn to_tui(&self, context: &DrawContext) -> Self::Output<'_> {
        Block::default()
            .borders(Borders::ALL)
            .border_style(context.theme.pane_border_style(self.is_focused))
            .title(self.title.as_str())
    }
}

/// A piece of text that looks interactable
pub struct ButtonBrick<'a> {
    pub text: &'a str,
    pub is_highlighted: bool,
}

impl<'a> ToTui for ButtonBrick<'a> {
    type Output<'this> = Text<'this> where Self: 'this;

    fn to_tui(&self, context: &DrawContext) -> Self::Output<'_> {
        Text::styled(self.text, context.theme.list_highlight_style)
    }
}

/// A list with a border and title. Each item has to be convertible to text
pub struct ListBrick<'a, T: ToTui<Output<'a> = Span<'a>>> {
    pub block: BlockBrick,
    pub list: &'a StatefulList<T>,
}

impl<'a, T: ToTui<Output<'a> = Span<'a>>> ToTui for ListBrick<'a, T> {
    type Output<'this> = List<'this> where Self: 'this;

    fn to_tui(&self, context: &DrawContext) -> Self::Output<'_> {
        let block = self.block.to_tui(context);

        // Convert each list item into text
        let items: Vec<ListItem<'_>> = self
            .list
            .items
            .iter()
            .map(|i| ListItem::new(i.to_tui(context)))
            .collect();

        List::new(items)
            .block(block)
            .highlight_style(context.theme.list_highlight_style)
    }
}

/// Yes or no?
pub struct Checkbox {
    pub checked: bool,
}

impl ToTui for Checkbox {
    type Output<'this> = Text<'this>;

    fn to_tui<'a>(&'a self, _context: &DrawContext) -> Self::Output<'a> {
        if self.checked {
            "[x]".into()
        } else {
            "[ ]".into()
        }
    }
}

impl ToTui for String {
    /// Use `Text` because a string can be multiple lines
    type Output<'this> = Text<'this> where Self: 'this;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        self.as_str().into()
    }
}

impl ToTui for Profile {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        self.name().to_owned().into()
    }
}

impl ToTui for RequestRecipe {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        format!("[{}] {}", self.method, self.name()).into()
    }
}

impl ToTui for Notification {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        format!(
            "[{}] {}",
            self.timestamp.with_timezone(&Local).format("%H:%M:%S"),
            self.message
        )
        .into()
    }
}

/// Format a timestamp in the local timezone
impl ToTui for DateTime<Utc> {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        self.with_timezone(&Local)
            .format("%b %e %H:%M:%S")
            .to_string()
            .into()
    }
}

impl ToTui for Duration {
    /// 'static because string is generated
    type Output<'this> = Span<'static>;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        let ms = self.num_milliseconds();
        if ms < 1000 {
            format!("{ms}ms").into()
        } else {
            format!("{:.2}s", ms as f64 / 1000.0).into()
        }
    }
}

impl ToTui for Option<Duration> {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, context: &DrawContext) -> Self::Output<'_> {
        match self {
            Some(duration) => duration.to_tui(context),
            // For incomplete requests typically
            None => "???".into(),
        }
    }
}

/// Not all header values are UTF-8; use a placeholder if not
impl ToTui for HeaderValue {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        match self.to_str() {
            Ok(s) => s.into(),
            Err(_) => "<invalid utf-8>".into(),
        }
    }
}

impl ToTui for anyhow::Error {
    /// 'static because string is generated
    type Output<'this> = Text<'static>;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        self.chain().map(|err| err.to_string()).join("\n").into()
    }
}

impl ToTui for RequestBuildError {
    type Output<'this> = Text<'static>;

    fn to_tui(&self, context: &DrawContext) -> Self::Output<'_> {
        // Defer to the underlying anyhow error
        self.error.to_tui(context)
    }
}

impl ToTui for RequestError {
    type Output<'this> = Text<'static>;

    fn to_tui(&self, _context: &DrawContext) -> Self::Output<'_> {
        self.error.to_string().into()
    }
}

/// A prompter that returns a static value; used for template previews, where
/// user interaction isn't possible
#[derive(Debug)]
pub struct PreviewPrompter;

impl Prompter for PreviewPrompter {
    fn prompt(&self, prompt: Prompt) {
        prompt.respond("<prompt>".into())
    }
}

/// Helper for building a layout with a fixed number of constraints
pub fn layout<const N: usize>(
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

/// Created a rectangle centered on the given `Rect`.
pub fn centered_rect(x: Constraint, y: Constraint, rect: Rect) -> Rect {
    fn buffer(constraint: Constraint, full_size: u16) -> Constraint {
        match constraint {
            Constraint::Percentage(percent) => {
                Constraint::Percentage((100 - percent) / 2)
            }
            Constraint::Length(length) => {
                Constraint::Length((full_size - length) / 2)
            }
            // Implement these as needed
            _ => unimplemented!("Other center constraints unsupported"),
        }
    }

    let buffer_x = buffer(x, rect.width);
    let buffer_y = buffer(y, rect.height);
    let columns = Layout::default()
        .direction(Direction::Vertical)
        .constraints([buffer_y, y, buffer_y].as_ref())
        .split(rect);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([buffer_x, x, buffer_x].as_ref())
        .split(columns[1])[1]
}
