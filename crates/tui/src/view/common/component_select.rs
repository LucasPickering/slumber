use crate::{
    context::TuiContext,
    view::{
        UpdateContext,
        common::select::{Select, SelectData},
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{Event, EventMatch},
    },
};
use derive_more::derive::{Deref, DerefMut};
use ratatui::{
    layout::{Constraint, Layout},
    style::Style,
    widgets::{Block, ListState, TableState},
};

/// A wrapper around [Select] for a list of items that implement [Component].
/// This provides some additional functionality:
/// - Items are treated as children in the component tree, allowing them to
///   receive events
/// - The [Draw] implementation uses each item's own [Draw] impl, allowing for
///   complex rendering beyond just generating `Text`
#[derive(Debug, Deref, DerefMut)]
pub struct ComponentSelect<Item, State = ListState> {
    #[deref]
    select: Select<Item, State>,
}

impl<Item, State> ComponentSelect<Item, State> {
    pub fn new(select: Select<Item, State>) -> Self {
        Self { select }
    }
}

impl<Item, State> Component for ComponentSelect<Item, State>
where
    Item: Component,
    State: SelectData,
{
    fn id(&self) -> ComponentId {
        self.select.id()
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        // Since this is a wrapper for Select, we pass events directly to it
        // instead of treating it as a child. This allows us to include all its
        // items as children while avoiding multiple uses of the mutable ref
        self.select.update(context, event)
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        self.select.items_mut().map(ToChild::to_child_mut).collect()
    }
}

/// Props for rendering a [ComponentSelect] as a table
pub struct ComponentSelectTableProps<T>(pub T);

impl<Item, Props> Draw<ComponentSelectTableProps<Props>>
    for ComponentSelect<Item, TableState>
where
    Props: Clone,
    Item: Component + Draw<Props>,
{
    fn draw(
        &self,
        canvas: &mut Canvas,
        ComponentSelectTableProps(props): ComponentSelectTableProps<Props>,
        metadata: DrawMetadata,
    ) {
        let styles = &TuiContext::get().styles.table;

        // Grab the subset of items in the viewport. Each item gets only 1
        // cell of height. No newlines!
        let height = metadata.area().height as usize;
        self.select.scroll_to_selected(height);
        let selected_index = self.selected_index();
        let iter = self
            .items_with_metadata()
            .enumerate()
            .map(|(i, item)| (item, Some(i) == selected_index))
            .skip(self.select.offset())
            .take(height);

        let item_areas =
            Layout::vertical((0..iter.len()).map(|_| Constraint::Length(1)))
                .split(metadata.area());
        for ((item, is_selected), area) in iter.zip(item_areas.iter()) {
            // Apply styling before the render
            let mut style = Style::default();
            if !item.enabled() {
                style = style.patch(styles.disabled);
            }
            if is_selected {
                style = style.patch(styles.highlight);
            }
            canvas.render_widget(Block::new().style(style), *area);

            canvas.draw(&item.value, props.clone(), *area, is_selected);
        }
    }
}
