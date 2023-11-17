use crate::tui::view::{
    common::Block, draw::Generate, state::select::SelectState, theme::Theme,
};
use ratatui::{
    text::Span,
    widgets::{ListItem, ListState},
};

/// A list with a border and title. Each item has to be convertible to text
pub struct List<'a, T> {
    pub block: Block<'a>,
    pub list: &'a SelectState<T, ListState>,
}

impl<'a, T> Generate for List<'a, T>
where
    &'a T: Generate<Output<'a> = Span<'a>>,
{
    type Output<'this> = ratatui::widgets::List<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let block = self.block.generate();

        // Convert each list item into text
        let items: Vec<ListItem<'_>> = self
            .list
            .items()
            .iter()
            .map(|i| ListItem::new(i.generate()))
            .collect();

        ratatui::widgets::List::new(items)
            .block(block)
            .highlight_style(Theme::get().list_highlight_style)
    }
}
