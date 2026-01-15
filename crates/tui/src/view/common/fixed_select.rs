use crate::view::{
    common::select::{
        Select, SelectBuilder, SelectEvent, SelectItem, SelectState,
    },
    component::{Canvas, Component, ComponentId, Draw, DrawMetadata},
    context::UpdateContext,
    event::{Emitter, Event, EventMatch, ToEmitter},
    persistent::PersistentKey,
};
use itertools::Itertools;
use ratatui::widgets::ListState;
use std::fmt::{Debug, Display};
use strum::{EnumCount, IntoEnumIterator};

/// A static (AKA fixed) list of items
///
/// Fixed lists must be based on an iterable enum.  This supports a generic type
/// for the state "backend", which is the ratatui type that stores the selection
/// state. Typically you want `ListState` or `TableState`.
/// Invariant: The fixed-size type cannot be empty
#[derive(Debug)]
pub struct FixedSelect<Item, State = ListState>
where
    Item: FixedSelectItem,
    State: SelectState,
{
    id: ComponentId,
    /// Re-use Select for state management. The only different is we
    /// guarantee any value of the item type is in the list (because there's
    /// a fixed number of values), so in a few places we'll unwrap options.
    inner: Select<Item, State>,
}

impl<Item, State> FixedSelect<Item, State>
where
    Item: FixedSelectItem,
    State: SelectState,
{
    /// Start a builder for a new fixed-size list, with items derived from a
    /// static enum.
    ///
    /// ## Panics
    ///
    /// Panics if the enum is empty.
    pub fn builder() -> FixedSelectBuilder<Item, State> {
        let items = Item::iter().collect_vec();
        // Wr run on the assumption that it's not empty, to prevent
        // returning Options
        assert!(
            !items.is_empty(),
            "Empty fixed-size collection not allow. \
                Add a variant to your enum."
        );
        FixedSelectBuilder {
            inner: Select::builder(items).preselect(&Item::default()),
        }
    }

    /// Get the index of the currently selected item
    pub fn selected_index(&self) -> usize {
        self.inner
            .selected_index()
            .expect("Fixed-size list cannot be empty")
    }

    /// Get the currently selected item
    pub fn selected(&self) -> Item {
        self.inner
            .selected()
            .copied()
            .expect("Fixed-size list cannot be empty")
    }

    /// Get all items in the list
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.inner.items()
    }

    /// Get all items in the list, including each one's metadata
    pub fn items_with_metadata(
        &self,
    ) -> impl Iterator<Item = &SelectItem<Item>> {
        self.inner.items_with_metadata()
    }

    /// Is the given item selected?
    pub fn is_selected(&self, item: &Item) -> bool
    where
        Item: PartialEq,
    {
        &self.selected() == item
    }

    /// Select the previous item in the list
    pub fn previous(&mut self) {
        // We only support top-to-bottom, so up is previous
        self.inner.up();
    }

    /// Select the next item in the list
    pub fn next(&mut self) {
        // We only support top-to-bottom, so down is next
        self.inner.down();
    }
}

impl<Item, State> Default for FixedSelect<Item, State>
where
    Item: FixedSelectItem,
    State: SelectState,
{
    fn default() -> Self {
        Self::builder().build()
    }
}

impl<Item, State> Component for FixedSelect<Item, State>
where
    Item: FixedSelectItem,
    State: SelectState,
{
    fn id(&self) -> ComponentId {
        self.id
    }

    // Handle input events to cycle between items
    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        self.inner.update(context, event)
    }
}

/// See equivalent impl on [Select] for description
impl<Item, State, Props> Draw<Props> for FixedSelect<Item, State>
where
    Item: FixedSelectItem,
    State: SelectState,
    Select<Item, State>: Draw<Props>,
{
    fn draw(&self, canvas: &mut Canvas, props: Props, metadata: DrawMetadata) {
        // This is a transparent wrapper so we should defer directly
        self.inner.draw(canvas, props, metadata);
    }
}

impl<Item: FixedSelectItem> ToEmitter<SelectEvent<Item>> for FixedSelect<Item> {
    fn to_emitter(&self) -> Emitter<SelectEvent<Item>> {
        self.inner.to_emitter()
    }
}

/// Wrapper around [SelectBuilder] to build a [FixedSelect]
pub struct FixedSelectBuilder<Item, State> {
    /// Defer to SelectBuilder for everything
    inner: SelectBuilder<Item, State>,
}

impl<Item: FixedSelectItem, State> FixedSelectBuilder<Item, State> {
    /// Disable certain items in the list. Disabled items can still be selected,
    /// but do not emit events.
    pub fn disabled(
        mut self,
        disabled: impl IntoIterator<Item = Item>,
    ) -> Self {
        // The inner builder disables by index, so we need to find the index for
        // each value. This is O(n^2) but the lists are so small it doesn't
        // matter.
        let disabled_indexes = disabled
            .into_iter()
            // unwrap() is safet because Item::iter() contains all possible
            // values of the enum
            .map(|v1| Item::iter().position(|v2| v1 == v2).unwrap());
        self.inner = self.inner.disabled_indexes(disabled_indexes);
        self
    }

    /// Get the persisted item from the store and select it
    pub fn persisted<K>(mut self, key: &K) -> Self
    where
        K: PersistentKey<Value = Item>,
    {
        self.inner = self.inner.persisted(key);
        self
    }

    pub fn build(self) -> FixedSelect<Item, State>
    where
        Item: FixedSelectItem,
        State: SelectState,
    {
        FixedSelect {
            id: ComponentId::default(),
            inner: self.inner.build(),
        }
    }
}

/// Trait alias for a static list of items to be cycled through
pub trait FixedSelectItem:
    'static
    + Copy
    + Clone
    + Debug
    + Default
    + Display
    + EnumCount
    + IntoEnumIterator
    + PartialEq
{
}

/// Auto-impl for anything we can
impl<T> FixedSelectItem for T where
    T: 'static
        + Copy
        + Clone
        + Debug
        + Default
        + Display
        + EnumCount
        + IntoEnumIterator
        + PartialEq
{
}
