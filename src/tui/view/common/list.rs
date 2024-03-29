use crate::tui::{
    context::TuiContext,
    view::{
        common::Pane,
        draw::Generate,
        state::select::{SelectState, SelectStateKind},
    },
};
use ratatui::{
    text::Span,
    widgets::{ListItem, ListState},
};

/// A list with optional border and title. Each item has to be convertible to
/// text
pub struct List<'a, Kind: SelectStateKind, Item> {
    pub block: Option<Pane<'a>>,
    pub list: &'a SelectState<Kind, Item, ListState>,
}

impl<'a, Kind, Item> Generate for List<'a, Kind, Item>
where
    Kind: SelectStateKind,
    &'a Item: Generate<Output<'a> = Span<'a>>,
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
            .items()
            .iter()
            .map(|i| ListItem::new(i.generate()))
            .collect();

        ratatui::widgets::List::new(items)
            .block(block)
            .highlight_style(TuiContext::get().theme.list.highlight)
    }
}
