use crate::tui::{
    input::Action,
    view::event::{Event, EventHandler, Update, UpdateContext},
};
use itertools::Itertools;
use ratatui::widgets::{ListState, TableState};
use std::{
    cell::RefCell,
    fmt::{Debug, Display},
    ops::DerefMut,
};
use strum::IntoEnumIterator;

/// State manager for a dynamic list of items. This supports a generic type for
/// the state "backend", which is the ratatui type that stores the selection
/// state. Typically you want `ListState` or `TableState`.
#[derive(Debug)]
pub struct SelectState<Item, State = ListState> {
    /// Use interior mutability because this needs to be modified during the
    /// draw phase, by [Frame::render_stateful_widget]. This allows rendering
    /// without a mutable reference.
    state: RefCell<State>,
    items: Vec<Item>,
}

impl<Item, State: SelectStateData> SelectState<Item, State> {
    pub fn new(items: Vec<Item>) -> Self {
        let mut state = State::default();
        // Pre-select the first item if possible
        if !items.is_empty() {
            state.select(0);
        }
        SelectState {
            state: RefCell::new(state),
            items,
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Get the index of the currently selected item (if any)
    pub fn selected_index(&self) -> Option<usize> {
        self.state.borrow().selected()
    }

    /// Get the currently selected item (if any)
    pub fn selected(&self) -> Option<&Item> {
        self.items.get(self.state.borrow().selected()?)
    }

    /// Get the number of items in the list
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Get a mutable reference to state. This uses `RefCell` underneath so it
    /// will panic if aliased. Only call this during the draw phase!
    pub fn state_mut(&self) -> impl DerefMut<Target = State> + '_ {
        self.state.borrow_mut()
    }

    /// Select the previous item in the list. This should only be called during
    /// the message phase, so we can take `&mut self`.
    pub fn previous(&mut self) {
        let state = self.state.get_mut();
        let i = match state.selected() {
            Some(i) => {
                // Avoid underflow here
                if i == 0 {
                    self.items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        state.select(i);
    }

    /// Select the next item in the list. This should only be called during the
    /// message phase, so we can take `&mut self`.
    pub fn next(&mut self) {
        let state = self.state.get_mut();
        let i = match state.selected() {
            Some(i) => (i + 1) % self.items.len(),
            None => 0,
        };
        state.select(i);
    }
}

/// Handle input events to cycle between items
impl<Item: Debug, State: Debug + SelectStateData> EventHandler
    for SelectState<Item, State>
{
    fn update(&mut self, _context: &mut UpdateContext, event: Event) -> Update {
        match event {
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Up => {
                    self.previous();
                    Update::Consumed
                }
                Action::Down => {
                    self.next();
                    Update::Consumed
                }
                _ => Update::Propagate(event),
            },
            _ => Update::Propagate(event),
        }
    }
}

/// State manager for a fixed-size collection of statically known selectable
/// items, e.g. panes or tabs. User can cycle between them. This is mostly a
/// wrapper around [SelectState], with some extra convenience based around the
/// fact that we statically know the available options.
#[derive(Debug)]
pub struct FixedSelectState<
    Item: FixedSelect,
    State: SelectStateData = ListState,
> {
    /// Internally use a dynamic list. We know it's not empty though, so we can
    /// assume that an item is always selected.
    state: SelectState<Item, State>,
}

impl<Item, State> FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    pub fn new() -> Self {
        let items = Item::iter().collect_vec();
        let mut state = State::default();
        // Pre-select the default item
        let selected = items
            .iter()
            .find_position(|value| *value == &Item::default())
            .expect("Empty fixed select")
            .0;
        state.select(selected);

        Self {
            state: SelectState {
                state: RefCell::new(state),
                items,
            },
        }
    }

    /// Get the index of the selected element
    pub fn selected_index(&self) -> usize {
        self.state.selected_index().unwrap()
    }

    /// Get the selected element
    pub fn selected(&self) -> &Item {
        self.state.selected().unwrap()
    }

    /// Is the given item selected?
    pub fn is_selected(&self, item: &Item) -> bool {
        self.selected() == item
    }

    /// Select previous item
    pub fn previous(&mut self) {
        self.state.previous()
    }

    /// Select next item
    pub fn next(&mut self) {
        self.state.next()
    }

    /// Get a mutable reference to state. This uses `RefCell` underneath so it
    /// will panic if aliased. Only call this during the draw phase!
    pub fn state_mut(&self) -> impl DerefMut<Target = State> + '_ {
        self.state.state_mut()
    }
}

impl<Item, State> Default for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Handle input events to cycle between items
impl<Item, State> EventHandler for FixedSelectState<Item, State>
where
    Item: FixedSelect,
    State: Debug + SelectStateData,
{
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        self.state.update(context, event)
    }
}

/// Inner state for [SelectState]. This is an abstraction to allow [SelectState]
/// to support multiple state "backends" from Ratatui. This enables usage with
/// different stateful widgets.
pub trait SelectStateData: Default {
    fn selected(&self) -> Option<usize>;

    fn select(&mut self, option: usize);
}

impl SelectStateData for ListState {
    fn selected(&self) -> Option<usize> {
        self.selected()
    }

    fn select(&mut self, option: usize) {
        self.select(Some(option))
    }
}

impl SelectStateData for TableState {
    fn selected(&self) -> Option<usize> {
        self.selected()
    }

    fn select(&mut self, option: usize) {
        self.select(Some(option))
    }
}

impl SelectStateData for usize {
    fn selected(&self) -> Option<usize> {
        Some(*self)
    }

    fn select(&mut self, option: usize) {
        *self = option;
    }
}

/// Trait alias for a static list of items to be cycled through
pub trait FixedSelect:
    Debug + Default + Display + IntoEnumIterator + PartialEq
{
}

impl<T: Debug + Default + Display + IntoEnumIterator + PartialEq> FixedSelect
    for T
{
}
