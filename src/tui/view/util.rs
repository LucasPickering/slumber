//! Helper structs and functions for building components

use crate::{
    config::{Profile, RequestRecipe},
    http::{RequestBuildError, RequestError},
    tui::view::{
        component::Draw,
        state::{FixedSelect, Notification, StatefulList, StatefulSelect},
        Frame, RenderContext,
    },
};
use chrono::{DateTime, Duration, Local, Utc};
use indexmap::IndexMap;
use ratatui::{
    prelude::*,
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Tabs},
};
use reqwest::header::HeaderMap;
use std::{
    fmt::{Debug, Display},
    mem,
};
use tracing::warn;

/// A helper for building a UI. It can be converted into some UI element to be
/// drawn.
pub trait ToTui {
    type Output<'this>
    where
        Self: 'this;

    /// Build a UI element
    fn to_tui(&self, context: &RenderContext) -> Self::Output<'_>;
}

/// A container with a title and border
pub struct BlockBrick {
    pub title: String,
    pub is_focused: bool,
}

impl ToTui for BlockBrick {
    type Output<'this> = Block<'this> where Self: 'this;

    fn to_tui(&self, context: &RenderContext) -> Self::Output<'_> {
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

    fn to_tui(&self, context: &RenderContext) -> Self::Output<'_> {
        Text::styled(self.text, context.theme.text_highlight_style)
    }
}

pub struct TabBrick<'a, T: FixedSelect> {
    pub tabs: &'a StatefulSelect<T>,
}

impl<'a, T: FixedSelect> ToTui for TabBrick<'a, T> {
    type Output<'this> = Tabs<'this> where Self: 'this;

    fn to_tui(&self, context: &RenderContext) -> Self::Output<'_> {
        Tabs::new(T::iter().map(|e| e.to_string()).collect())
            .select(self.tabs.selected_index())
            .highlight_style(context.theme.text_highlight_style)
    }
}

/// A list with a border and title. Each item has to be convertible to text
pub struct ListBrick<'a, T: ToTui<Output<'a> = Span<'a>>> {
    pub block: BlockBrick,
    pub list: &'a StatefulList<T>,
}

impl<'a, T: ToTui<Output<'a> = Span<'a>>> ToTui for ListBrick<'a, T> {
    type Output<'this> = List<'this> where Self: 'this;

    fn to_tui(&self, context: &RenderContext) -> Self::Output<'_> {
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
            .highlight_style(context.theme.text_highlight_style)
            .highlight_symbol(context.theme.list_highlight_symbol)
    }
}

impl ToTui for Profile {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
        self.name().to_owned().into()
    }
}

impl ToTui for RequestRecipe {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
        format!("[{}] {}", self.method, self.name()).into()
    }
}

impl ToTui for Notification {
    type Output<'this> = Span<'this> where Self: 'this;

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
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

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
        self.with_timezone(&Local)
            .format("%b %e %H:%M:%S")
            .to_string()
            .into()
    }
}

impl ToTui for Duration {
    /// 'static because string is generated
    type Output<'this> = Span<'static>;

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
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

    fn to_tui(&self, context: &RenderContext) -> Self::Output<'_> {
        match self {
            Some(duration) => duration.to_tui(context),
            // For incomplete requests typically
            None => "???".into(),
        }
    }
}

impl<K: Display, V: Display> ToTui for IndexMap<K, V> {
    type Output<'this> = Text<'this> where Self: 'this;

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
        self.iter()
            .map(|(key, value)| format!("{key} = {value}").into())
            .collect::<Vec<Line>>()
            .into()
    }
}

impl ToTui for HeaderMap {
    /// 'static because string is generated
    type Output<'this> = Text<'static>;

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
        self.iter()
            .map(|(key, value)| {
                format!(
                    "{key} = {}",
                    value.to_str().unwrap_or("<unrepresentable>")
                )
                .into()
            })
            .collect::<Vec<Line>>()
            .into()
    }
}

impl ToTui for anyhow::Error {
    /// 'static because string is generated
    type Output<'this> = Text<'static>;

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
        self.chain()
            .enumerate()
            .map(|(i, err)| {
                // Add indentation to parent errors
                format!("{}{err}", if i > 0 { "  " } else { "" }).into()
            })
            .collect::<Vec<Line>>()
            .into()
    }
}

impl ToTui for RequestBuildError {
    type Output<'this> = Text<'static>;

    fn to_tui(&self, context: &RenderContext) -> Self::Output<'_> {
        // Defer to the underlying anyhow error
        self.error.to_tui(context)
    }
}

impl ToTui for RequestError {
    type Output<'this> = Text<'static>;

    fn to_tui(&self, _context: &RenderContext) -> Self::Output<'_> {
        self.error.to_string().into()
    }
}

/// A generic modal, which is a temporary dialog that appears for the user. The
/// contents of the modal should be determined by the concrete implementation.
///
/// The modal is generally responsible for listening for its own open event,
/// and also closing itself. This leads to update logic being a bit grungy
/// because you have to check `self.is_open()`.
/// TODO move open/close logic into generic struct
#[derive(Debug, Default)]
pub enum Modal<T> {
    #[default]
    Closed,
    Open(T),
}

/// Something that can be rendered into a modal
pub trait ModalContent {
    /// Text at the top of the modal
    fn title(&self) -> &str;

    /// Dimensions of the modal, relative to the whole screen
    fn dimensions(&self) -> (Constraint, Constraint);
}

impl<T: Debug + Draw> Modal<T> {
    pub fn new() -> Self {
        Self::Closed
    }

    pub fn is_open(&self) -> bool {
        matches!(self, Self::Open { .. })
    }

    /// Open a new modal with the given initial state
    pub fn open(&mut self, state: T) {
        if let Self::Open(existing) = self {
            // Just a safety check
            warn!("Modal {existing:?} already open, overwriting it...");
        }
        *self = Self::Open(state);
    }

    /// Close the modal, and if it was open, return the inner value that was
    /// there
    pub fn close(&mut self) -> Option<T> {
        match mem::take(self) {
            Modal::Closed => None,
            Modal::Open(state) => Some(state),
        }
    }
}

impl<T: Draw + ModalContent> Draw for Modal<T> {
    type Props<'a> = T::Props<'a> where Self: 'a;

    fn draw<'a>(
        &'a self,
        context: &RenderContext,
        props: Self::Props<'a>,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // If open, draw the contents
        if let Self::Open(inner) = self {
            let (x, y) = inner.dimensions();
            let chunk = centered_rect(x, y, chunk);
            let block = Block::default()
                .title(inner.title())
                .borders(Borders::ALL)
                .border_type(BorderType::Thick);
            let inner_chunk = block.inner(chunk);

            // Draw the outline of the modal
            frame.render_widget(Clear, chunk);
            frame.render_widget(block, chunk);

            // Render the actual content
            inner.draw(context, props, frame, inner_chunk);
        }
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
