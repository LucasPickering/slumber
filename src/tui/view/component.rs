//! Helper components for building panes

use crate::{
    config::{Environment, RequestRecipe},
    template::TemplateString,
    tui::{
        state::{FixedSelect, StatefulList, StatefulSelect},
        view::Renderer,
    },
};
use ratatui::{
    text::{Line, Text},
    widgets::{Block, Borders, List, ListItem, Tabs},
};
use reqwest::header::HeaderMap;
use std::collections::HashMap;

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

pub struct TabComponent<'a, T: FixedSelect> {
    pub tabs: &'a StatefulSelect<T>,
}

impl<'a, T: FixedSelect> Component for TabComponent<'a, T> {
    type Output = Tabs<'static>;

    fn render(self, renderer: &Renderer) -> Self::Output {
        Tabs::new(T::iter().map(|e| e.to_string()).collect())
            .select(self.tabs.selected_index())
            .highlight_style(renderer.theme.tab_highlight_style)
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
            // .style(Style::default().fg(Color::Black).bg(Color::White))
            .highlight_style(renderer.theme.list_highlight_style)
            .highlight_symbol(renderer.theme.list_highlight_symbol)
    }
}

/// Convert a value into a renderable text. This *could* just be a
/// Component impl, but there may be a different way to render the same type,
/// with more detail.
///
/// This uses 'static to get around some borrow checking issues. It's lazy but
/// it works.
pub trait ToText {
    fn to_text(&self) -> Text<'static>;
}

// Getting lazy with the lifetimes here...
impl ToText for Environment {
    fn to_text(&self) -> Text<'static> {
        vec![Line::from(self.name.clone())].into()
    }
}

impl ToText for RequestRecipe {
    fn to_text(&self) -> Text<'static> {
        vec![Line::from(format!("[{}] {}", self.method, self.name))].into()
    }
}

impl ToText for HashMap<String, TemplateString> {
    fn to_text(&self) -> Text<'static> {
        self.iter()
            .map(|(key, value)| format!("{key} = {value}").into())
            .collect::<Vec<Line>>()
            .into()
    }
}

impl ToText for HeaderMap {
    fn to_text(&self) -> Text<'static> {
        self.into_iter()
            .map(|(key, value)| {
                format!("{key} = {}", value.to_str().unwrap_or("<unknown>"))
                    .into()
            })
            .collect::<Vec<Line>>()
            .into()
    }
}
