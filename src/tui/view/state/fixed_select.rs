use crate::tui::view::{
    draw::{Draw, DrawMetadata},
    event::{Event, EventHandler, Update},
    state::select::{SelectState, SelectStateBuilder, SelectStateData},
};
use itertools::Itertools;
use persisted::PersistedContainer;
use ratatui::{
    widgets::{ListState, StatefulWidget},
    Frame,
};
use std::fmt::{Debug, Display};
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
    select: SelectState<Item, State>,
}

pub struct FixedSelectStateBuilder<Item, State> {
    /// Defer to SelectStateBuilder for everything
    select: SelectStateBuilder<Item, State>,
}

impl<Item, State> FixedSelectStateBuilder<Item, State> {
    /// Set the callback to be called when the user highlights a new item
    pub fn on_select(
        mut self,
        on_select: impl 'static + Fn(&mut Item),
    ) -> Self {
        self.select = self.select.on_select(on_select);
        self
    }

    /// Set the callback to be called when the user hits enter on an item
    pub fn on_submit(
        mut self,
        on_submit: impl 'static + Fn(&mut Item),
    ) -> Self {
        self.select = self.select.on_submit(on_submit);
        self
    }

    pub fn build(self) -> FixedSelectState<Item, State>
    where
        Item: FixedSelect,
        State: SelectStateData,
    {
        FixedSelectState {
            select: self.select.preselect(&Item::default()).build(),
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
            select: SelectState::builder(items),
        }
    }

    /// Get the index of the currently selected item
    pub fn selected_index(&self) -> usize {
        self.select
            .selected_index()
            .expect("Fixed-size list cannot be empty")
    }

    /// Get the currently selected item
    pub fn selected(&self) -> Item {
        self.select
            .selected()
            .copied()
            .expect("Fixed-size list cannot be empty")
    }

    /// Get all items in the list
    pub fn items(&self) -> &[Item] {
        self.select.items()
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
        self.select.select(value);
    }

    /// Select the previous item in the list
    pub fn previous(&mut self) {
        self.select.previous();
    }

    /// Select the next item in the list
    pub fn next(&mut self) {
        self.select.next();
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

/// Handle input events to cycle between items
impl<Item, State> EventHandler for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: Debug + SelectStateData,
{
    fn update(&mut self, event: Event) -> Update {
        self.select.update(event)
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
        self.select.draw(frame, props, metadata);
    }
}

impl<Item, State> PersistedContainer for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    type Value = Item;

    fn get_persisted(&self) -> Self::Value {
        self.selected()
    }

    fn set_persisted(&mut self, value: Self::Value) {
        // This will call the on_select callback if the item is in the list
        self.select(&value);
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
