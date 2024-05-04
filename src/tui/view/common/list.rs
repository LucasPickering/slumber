use crate::tui::{
    context::TuiContext,
    view::{common::Pane, draw::Generate},
};
use ratatui::{text::Span, widgets::ListItem};

/// A list with optional border and title. Each item has to be convertible to
/// text
pub struct List<'a, Item, Iter: 'a + IntoIterator<Item = Item>> {
    pub block: Option<Pane<'a>>,
    pub list: Iter,
}

impl<'a, Item, Iter> Generate for List<'a, Item, Iter>
where
    Item: 'a + Generate<Output<'a> = Span<'a>>,
    Iter: 'a + IntoIterator<Item = Item>,
{
    type Output<'this> = ratatui::widgets::List<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let block = self.block.map(Pane::generate).unwrap_or_default();

        // Convert each list item into text
        let items: Vec<ListItem<'_>> = self
            .list
            .into_iter()
            .map(|i| ListItem::new(i.generate()))
            .collect();

        ratatui::widgets::List::new(items)
            .block(block)
            .highlight_style(TuiContext::get().styles.list.highlight)
    }
}
