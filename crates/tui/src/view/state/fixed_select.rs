use crate::view::{
    context::UpdateContext,
    draw::{Draw, DrawMetadata},
    event::{Emitter, EmitterId, Event, EventHandler, Update},
    state::select::{
        SelectItem, SelectState, SelectStateBuilder, SelectStateData,
        SelectStateEvent, SelectStateEventType,
    },
};
use itertools::Itertools;
use persisted::PersistedContainer;
use ratatui::{
    widgets::{ListState, StatefulWidget},
    Frame,
};
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
    /// Re-use SelectState for state management. The only different is we
    /// guarantee any value of the item type is in the list (because there's
    /// a fixed number of values), so in a few places we'll unwrap options.
    inner: SelectState<Item, State>,
}

pub struct FixedSelectStateBuilder<Item, State> {
    /// Defer to SelectStateBuilder for everything
    inner: SelectStateBuilder<Item, State>,
}

impl<Item, State> FixedSelectStateBuilder<Item, State> {
    /// Disable certain items in the list by value. Disabled items can still be
    /// selected, but do not trigger callbacks.
    pub fn disabled_items<'a, T>(
        mut self,
        disabled_items: impl IntoIterator<Item = &'a T>,
    ) -> Self
    where
        T: 'a + PartialEq<Item>,
    {
        self.inner = self.inner.disabled_items(disabled_items);
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
            inner: self.inner.preselect(&Item::default()).build(),
        }
    }
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
        if items.is_empty() {
            // Wr run on the assumption that it's not empty, to prevent
            // returning Options
            panic!(
                "Empty fixed-size collection not allow. \
                Add a variant to your enum."
            );
        }
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
        self.inner.previous();
    }

    /// Select the next item in the list
    pub fn next(&mut self) {
        self.inner.next();
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

/// Handle input events to cycle between items
impl<Item, State> EventHandler for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: Debug + SelectStateData,
{
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        self.inner.update(context, event)
    }
}

/// See equivalent impl on [SelectState] for description
impl<Item, State, W> Draw<W> for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
    W: StatefulWidget<State = State>,
{
    fn draw(&self, frame: &mut Frame, props: W, metadata: DrawMetadata) {
        self.inner.draw(frame, props, metadata);
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

impl<T: FixedSelect> Emitter for FixedSelectState<T> {
    type Emitted = SelectStateEvent;

    fn id(&self) -> EmitterId {
        self.inner.id()
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
