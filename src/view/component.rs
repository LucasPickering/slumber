//! Helper components for building panes

use crate::{
    config::{Environment, RequestRecipe},
    state::{FixedSelect, StatefulList, StatefulSelect},
    view::Renderer,
};
use ratatui::{
    text::Line,
    widgets::{Block, Borders, List, ListItem, Tabs},
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
    fn render(self, renderer: &Renderer) -> Self::Output;
}

pub struct BlockComponent {
    pub title: &'static str,
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
        Tabs::new(T::all().into_iter().map(|e| e.title()).collect())
            .select(self.tabs.selected_index())
            .highlight_style(renderer.theme.tab_highlight_style)
    }
}

pub struct ListComponent<'a, T: ToListItem> {
    pub block: BlockComponent,
    pub list: &'a StatefulList<T>,
}

impl<'a, T: ToListItem> Component for ListComponent<'a, T> {
    type Output = List<'static>;

    fn render(self, renderer: &Renderer) -> Self::Output {
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
