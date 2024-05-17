use crate::tui::{
    context::TuiContext,
    view::{common::scrollbar::Scrollbar, draw::Generate},
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Text,
    widgets::{ListItem, ListState, StatefulWidget, Widget},
};
use std::marker::PhantomData;

/// A sequence of items, with a scrollbar and optional surrounding pane
pub struct List<'a, Item, Iter: 'a + IntoIterator<Item = Item>> {
    items: Iter,
    _phantom: PhantomData<&'a ()>,
}

impl<'a, Item, Iter: 'a + IntoIterator<Item = Item>> List<'a, Item, Iter> {
    pub fn new(items: Iter) -> Self {
        Self {
            items,
            _phantom: PhantomData,
        }
    }
}

impl<'a, T, Item, Iter> StatefulWidget for List<'a, Item, Iter>
where
    T: Into<Text<'a>>,
    Item: 'a + Generate<Output<'a> = T>,
    Iter: 'a + IntoIterator<Item = Item>,
{
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut ListState) {
        // Draw list
        let items: Vec<ListItem<'_>> = self
            .items
            .into_iter()
            .map(|i| ListItem::new(i.generate()))
            .collect();
        let num_items = items.len();
        let list = ratatui::widgets::List::new(items)
            .highlight_style(TuiContext::get().styles.list.highlight);
        StatefulWidget::render(list, area, buf, state);

        // Draw scrollbar
        Scrollbar {
            content_length: num_items,
            offset: state.offset(),
            ..Default::default()
        }
        .render(area, buf);
    }
}
