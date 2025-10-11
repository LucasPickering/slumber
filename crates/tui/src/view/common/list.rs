use crate::{
    context::TuiContext,
    view::{
        common::scrollbar::Scrollbar,
        draw::Generate,
        state::{
            fixed_select::{FixedSelect, FixedSelectState},
            select::{SelectItem, SelectState},
        },
    },
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Styled,
    text::Text,
    widgets::{
        List as TuiList, ListItem as TuiListItem, ListState, StatefulWidget,
        Widget,
    },
};
use std::marker::PhantomData;

/// A sequence of items, with a scrollbar and optional surrounding pane
pub struct List<'a, Item> {
    items: Vec<ListItem<Item>>,
    /// This *shouldn't* be required, but without it we hit this ICE:
    /// <https://github.com/rust-lang/rust/issues/124189>
    phantom: PhantomData<&'a ()>,
}

impl<'a, Item> From<&'a SelectState<Item>> for List<'a, &'a Item> {
    fn from(select: &'a SelectState<Item>) -> Self {
        Self {
            items: select.items_with_metadata().map(ListItem::from).collect(),
            phantom: PhantomData,
        }
    }
}

impl<'a, Item> From<&'a FixedSelectState<Item>> for List<'a, &'a Item>
where
    Item: FixedSelect,
{
    fn from(select: &'a FixedSelectState<Item>) -> Self {
        Self {
            items: select.items_with_metadata().map(ListItem::from).collect(),
            phantom: PhantomData,
        }
    }
}

impl<'a, T, Item> StatefulWidget for List<'a, Item>
where
    T: Into<Text<'a>>,
    Item: 'a + Generate<Output<'a> = T>,
{
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut ListState) {
        let styles = &TuiContext::get().styles;

        // Draw list
        let items: Vec<TuiListItem<'_>> = self
            .items
            .into_iter()
            .map(|item| {
                let mut list_item = TuiListItem::new(item.value.generate());
                if item.disabled {
                    list_item = list_item.set_style(styles.list.disabled);
                }
                list_item
            })
            .collect();
        let num_items = items.len();
        let list = TuiList::new(items).highlight_style(styles.list.highlight);
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

struct ListItem<T> {
    value: T,
    disabled: bool,
}

impl<'a, T> From<&'a SelectItem<T>> for ListItem<&'a T> {
    fn from(item: &'a SelectItem<T>) -> Self {
        Self {
            value: &item.value,
            disabled: item.enabled(),
        }
    }
}
