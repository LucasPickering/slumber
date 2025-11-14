use crate::view::{
    component::{Canvas, Component, ComponentId, Draw, DrawMetadata},
    context::UpdateContext,
    event::{Emitter, Event, EventMatch, ToEmitter},
    state::select::{
        SelectItem, SelectState, SelectStateBuilder, SelectStateData,
        SelectStateEvent, SelectStateEventType,
    },
};
use itertools::Itertools;
use persisted::PersistedContainer;
use ratatui::widgets::ListState;
use std::{
    fmt::{Debug, Display},
    ops::{Index, IndexMut},
};
use strum::{EnumCount, IntoEnumIterator};

/// State manager for a static (AKA fixed) list of items. Fixed lists must be
/// based on an iterable enum.  This supports a generic type for the state
/// "backend", which is the ratatui type that stores the selection state.
/// Typically you want `ListState` or `TableState`.
/// Invariant: The fixed-size type cannot be empty
#[derive(Debug)]
pub struct FixedSelectState<Item, State = ListState>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    id: ComponentId,
    /// Re-use SelectState for state management. The only different is we
    /// guarantee any value of the item type is in the list (because there's
    /// a fixed number of values), so in a few places we'll unwrap options.
    inner: SelectState<Item, State>,
}

impl<Item, State> FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    /// Start a builder for a new fixed-size list, with items derived from a
    /// static enum.
    ///
    /// ## Panics
    ///
    /// Panics if the enum is empty.
    pub fn builder() -> FixedSelectStateBuilder<Item, State> {
        let items = Item::iter().collect_vec();
        // Wr run on the assumption that it's not empty, to prevent
        // returning Options
        assert!(
            !items.is_empty(),
            "Empty fixed-size collection not allow. \
                Add a variant to your enum."
        );
        FixedSelectStateBuilder {
            inner: SelectState::builder(items),
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

    /// Select an item by value. Context is required for callbacks. Generally
    /// the given value will be the type `Item`, but it could be anything that
    /// compares to `Item` (e.g. an ID type).
    pub fn select<T>(&mut self, value: &T)
    where
        T: PartialEq<Item>,
    {
        self.inner.select(value);
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

impl<Item, State> Default for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Get an item by index, and panic if out of bounds. Useful with emitted
/// events, when we know the index will be valid
impl<Item, State> Index<usize> for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    type Output = Item;

    fn index(&self, index: usize) -> &Self::Output {
        &self.inner[index]
    }
}

impl<Item, State> IndexMut<usize> for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.inner[index]
    }
}

impl<Item, State> Component for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
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

/// See equivalent impl on [SelectState] for description
impl<Item, State, Props> Draw<Props> for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
    SelectState<Item, State>: Draw<Props>,
{
    fn draw(&self, canvas: &mut Canvas, props: Props, metadata: DrawMetadata) {
        // This is a transparent wrapper so we should defer directly
        self.inner.draw(canvas, props, metadata);
    }
}

impl<Item, State> PersistedContainer for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    type Value = Item;

    fn get_to_persist(&self) -> Self::Value {
        self.selected()
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        // This will emit a Select event if the item is in the list
        self.select(&value);
    }
}

impl<T: FixedSelect> ToEmitter<SelectStateEvent> for FixedSelectState<T> {
    fn to_emitter(&self) -> Emitter<SelectStateEvent> {
        self.inner.to_emitter()
    }
}

/// Wrapper around [SelectStateBuilder] to build a [FixedSelect]
pub struct FixedSelectStateBuilder<Item, State> {
    /// Defer to SelectStateBuilder for everything
    inner: SelectStateBuilder<Item, State>,
}

impl<Item, State> FixedSelectStateBuilder<Item, State> {
    /// Disable certain items in the list. Disabled items can still be selected,
    /// but do not emit events.
    pub fn disabled(mut self, disabled: impl IntoIterator<Item = Item>) -> Self
    where
        Item: FixedSelect,
    {
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

    /// Which types of events should this emit?
    pub fn subscribe(
        mut self,
        event_types: impl IntoIterator<Item = SelectStateEventType>,
    ) -> Self {
        self.inner = self.inner.subscribe(event_types);
        self
    }

    pub fn build(self) -> FixedSelectState<Item, State>
    where
        Item: FixedSelect,
        State: SelectStateData,
    {
        FixedSelectState {
            id: ComponentId::default(),
            inner: self.inner.preselect(&Item::default()).build(),
        }
    }
}

/// Trait alias for a static list of items to be cycled through
pub trait FixedSelect:
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
impl<T> FixedSelect for T where
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
