//! Helper components for building panes

use crate::{
    config::{Environment, RequestRecipe},
    state::StatefulList,
    view::Renderer,
};
use ratatui::{
    text::Line,
    widgets::{Block, Borders, List, ListItem},
};

/// A component is a helper for building a UI. It can be rendered into some UI
/// element to be drawn.
///
/// These components generally clone the state data while rendering, in order
/// to detach the rendered content from app state. Some drawn panes require
/// a mutable reference to the state, which means we can't retain that ref here.
pub trait Component {
    type Output;

    /// Build a UI element
    fn render(&self, renderer: &Renderer) -> Self::Output;
}

pub struct ListComponent<'a, T: ToListItem> {
    pub block: BlockComponent,
    pub list: &'a StatefulList<T>,
}

pub struct BlockComponent {
    pub title: &'static str,
    pub is_focused: bool,
}

impl Component for BlockComponent {
    type Output = Block<'static>;

    fn render(&self, renderer: &Renderer) -> Self::Output {
        Block::default()
            .borders(Borders::ALL)
            .border_style(renderer.theme.pane_border_style(self.is_focused))
            .title(self.title)
    }
}

impl<'a, T: ToListItem> Component for ListComponent<'a, T> {
    type Output = List<'static>;

    fn render(&self, renderer: &Renderer) -> Self::Output {
        let block = self.block.render(renderer);

        // Convert each list item into text
        let items: Vec<ListItem<'static>> =
            self.list.items.iter().map(T::to_list_item).collect();

        List::new(items)
            .block(block)
            // .style(Style::default().fg(Color::Black).bg(Color::White))
            .highlight_style(renderer.theme.list_highlight_style)
            .highlight_symbol(renderer.theme.list_highlight_symbol)
    }
}

/// Convert a value into a renderable list item. This *could* just be a
/// Component impl, but there may be a different way to render the same type,
/// with more detail.
pub trait ToListItem {
    fn to_list_item(&self) -> ListItem<'static>;
}

// Getting lazy with the lifetimes here...
impl ToListItem for Environment {
    fn to_list_item(&self) -> ListItem<'static> {
        ListItem::new(vec![Line::from(self.name.clone())])
    }
}

impl ToListItem for RequestRecipe {
    fn to_list_item(&self) -> ListItem<'static> {
        ListItem::new(vec![Line::from(format!(
            "[{}] {}",
            self.method, self.name
        ))])
    }
}
