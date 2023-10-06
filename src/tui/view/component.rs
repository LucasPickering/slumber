//! Helper components for building panes

use crate::{
    config::{Profile, RequestRecipe},
    tui::{
        state::{FixedSelect, Notification, StatefulList, StatefulSelect},
        view::Renderer,
    },
};
use chrono::{DateTime, Duration, Local, Utc};
use indexmap::IndexMap;
use ratatui::{
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Tabs},
};
use reqwest::header::HeaderMap;
use std::fmt::Display;

/// A component is a helper for building a UI. It can be rendered into some UI
/// element to be drawn.
///
/// These components generally clone the state data while rendering, in order
/// to detach the rendered content from app state. Some drawn panes require
/// a mutable reference to the state, which means we can't retain that ref here.
pub trait Component {
    type Output;

    /// Build a UI element
    fn render(self, renderer: &Renderer) -> Self::Output;
}

pub struct BlockComponent {
    pub title: String,
    pub is_focused: bool,
}

impl Component for BlockComponent {
    type Output = Block<'static>;

    fn render(self, renderer: &Renderer) -> Self::Output {
        Block::default()
            .borders(Borders::ALL)
            .border_style(renderer.theme.pane_border_style(self.is_focused))
            .title(self.title)
    }
}

/// A piece of text that looks interactable
pub struct ButtonComponent<'a> {
    pub text: &'a str,
    pub is_highlighted: bool,
}

impl<'a> Component for ButtonComponent<'a> {
    type Output = Text<'a>;

    fn render(self, renderer: &Renderer) -> Self::Output {
        Text::styled(self.text, renderer.theme.text_highlight_style)
    }
}

pub struct TabComponent<'a, T: FixedSelect> {
    pub tabs: &'a StatefulSelect<T>,
}

impl<'a, T: FixedSelect> Component for TabComponent<'a, T> {
    type Output = Tabs<'static>;

    fn render(self, renderer: &Renderer) -> Self::Output {
        Tabs::new(T::iter().map(|e| e.to_string()).collect())
            .select(self.tabs.selected_index())
            .highlight_style(renderer.theme.text_highlight_style)
    }
}

pub struct ListComponent<'a, T: ToText> {
    pub block: BlockComponent,
    pub list: &'a StatefulList<T>,
}

impl<'a, T: ToText> Component for ListComponent<'a, T> {
    type Output = List<'static>;

    fn render(self, renderer: &Renderer) -> Self::Output {
        let block = self.block.render(renderer);

        // Convert each list item into text
        let items: Vec<ListItem<'static>> = self
            .list
            .items
            .iter()
            .map(|i| ListItem::new(i.to_text()))
            .collect();

        List::new(items)
            .block(block)
            .highlight_style(renderer.theme.text_highlight_style)
            .highlight_symbol(renderer.theme.list_highlight_symbol)
    }
}

/// Convert a value into a renderable text. This *could* just be a Component
/// impl, but there may be a different way to render the same type, with more
/// detail.
///
/// This uses 'static to get around some borrow checking issues. It's lazy but
/// it works.
pub trait ToText {
    fn to_text(&self) -> Text<'static>;
}

/// Convert a value into a single span of renderable text. Like [ToText], but
/// for text that doesn't take up multiple lines.
pub trait ToSpan {
    fn to_span(&self) -> Span<'static>;
}

// Getting lazy with the lifetimes here...
impl ToSpan for Profile {
    fn to_span(&self) -> Span<'static> {
        self.name().to_owned().into()
    }
}

impl ToSpan for RequestRecipe {
    fn to_span(&self) -> Span<'static> {
        format!("[{}] {}", self.method, self.name()).into()
    }
}

impl ToSpan for Notification {
    fn to_span(&self) -> Span<'static> {
        format!(
            "[{}] {}",
            self.timestamp.with_timezone(&Local).format("%H:%M:%S"),
            self.message
        )
        .into()
    }
}

/// Format a timestamp in the local timezone
impl ToSpan for DateTime<Utc> {
    fn to_span(&self) -> Span<'static> {
        self.with_timezone(&Local)
            .format("%b %e %H:%M:%S")
            .to_string()
            .into()
    }
}

impl ToSpan for Duration {
    fn to_span(&self) -> Span<'static> {
        let ms = self.num_milliseconds();
        if ms < 1000 {
            format!("{ms}ms").into()
        } else {
            format!("{:.2}s", ms as f64 / 1000.0).into()
        }
    }
}

impl ToSpan for Option<Duration> {
    fn to_span(&self) -> Span<'static> {
        match self {
            Some(duration) => duration.to_span(),
            // For incomplete requests typically
            None => "???".into(),
        }
    }
}

/// If we can make a little text, we can make a lotta text
impl<T: ToSpan> ToText for T {
    fn to_text(&self) -> Text<'static> {
        Line::from(self.to_span()).into()
    }
}

impl<K: Display, V: Display> ToText for IndexMap<K, V> {
    fn to_text(&self) -> Text<'static> {
        self.iter()
            .map(|(key, value)| format!("{key} = {value}").into())
            .collect::<Vec<Line>>()
            .into()
    }
}

impl ToText for HeaderMap {
    fn to_text(&self) -> Text<'static> {
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
