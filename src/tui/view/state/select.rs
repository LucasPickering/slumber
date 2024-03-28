use crate::tui::{
    input::Action,
    view::{
        event::{Event, EventHandler, Update, UpdateContext},
        state::persistence::{Persistable, PersistentContainer},
    },
};
use itertools::Itertools;
use ratatui::widgets::{ListState, TableState};
use std::{
    cell::RefCell,
    fmt::{Debug, Display},
    marker::PhantomData,
    ops::DerefMut,
};
use strum::{EnumCount, IntoEnumIterator};

/// State manager for a dynamic list of items. This supports a generic type for
/// the state "backend", which is the ratatui type that stores the selection
/// state. Typically you want `ListState` or `TableState`.
///
/// This uses a typestate pattern to differentiate between dynamic- and
/// fixed-size lists. Fixed-size lists must be based on an iterable enum. The
/// two share most behavior, but have some differences in API, which the `Kind`
/// parameter will switch between.
#[derive(derive_more::Debug)]
pub struct SelectState<Kind, Item, State = ListState>
where
    Kind: SelectStateKind,
    State: SelectStateData,
{
    /// Use interior mutability because this needs to be modified during the
    /// draw phase, by [ratatui::Frame::render_stateful_widget]. This allows
    /// rendering without a mutable reference.
    state: RefCell<State>,
    #[debug(skip)]
    items: Vec<Item>,
    #[debug(skip)]
    on_select: Option<Callback<Item>>,
    #[debug(skip)]
    on_submit: Option<Callback<Item>>,
    #[debug(skip)]
    _kind: PhantomData<Kind>,
}

/// Marker trait to restrict type-state options
pub trait SelectStateKind {}

/// Type-state for a dynamically sized [SelectState]
pub struct Dynamic;
impl SelectStateKind for Dynamic {}
/// Type-state for a fixed-size [SelectState]
pub struct Fixed;
impl SelectStateKind for Fixed {}

type Callback<Item> = Box<dyn Fn(&mut UpdateContext, &mut Item)>;

impl<Kind, Item, State: SelectStateData> SelectState<Kind, Item, State>
where
    Kind: SelectStateKind,
{
    /// Set the callback to be called when the user highlights a new item
    pub fn on_select(
        mut self,
        on_select: impl 'static + Fn(&mut UpdateContext, &mut Item),
    ) -> Self {
        self.on_select = Some(Box::new(on_select));
        self
    }

    /// Set the callback to be called when the user hits enter on an item
    pub fn on_submit(
        mut self,
        on_submit: impl 'static + Fn(&mut UpdateContext, &mut Item),
    ) -> Self {
        self.on_submit = Some(Box::new(on_submit));
        self
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Is the given item selected?
    pub fn is_selected(&self, item: &Item) -> bool
    where
        Item: PartialEq,
    {
        self.selected_opt() == Some(item)
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

    /// Select an item by value. Context is required for callbacks. Generally
    /// the given value will be the type `Item`, but it could be anything that
    /// compares to `Item` (e.g. an ID type).
    pub fn select<T>(&mut self, context: &mut UpdateContext, value: &T)
    where
        T: PartialEq<Item>,
    {
        if let Some((index, _)) =
            self.items.iter().find_position(|item| value == *item)
        {
            self.select_index(context, index);
        }
    }

    /// Select the previous item in the list. Context is required for callbacks.
    pub fn previous(&mut self, context: &mut UpdateContext) {
        self.select_delta(context, -1);
    }

    /// Select the next item in the list. Context is required for callbacks.
    pub fn next(&mut self, context: &mut UpdateContext) {
        self.select_delta(context, 1);
    }

    /// Select an item by index
    fn select_index(&mut self, context: &mut UpdateContext, index: usize) {
        let state = self.state.get_mut();
        let current = state.selected();
        state.select(index);
        let new = state.selected();

        // If the selection changed, call the callback
        match &self.on_select {
            Some(on_select) if current != new => {
                let selected = self
                    .state
                    .get_mut()
                    .selected()
                    .and_then(|index| self.items.get_mut(index));
                if let Some(selected) = selected {
                    on_select(context, selected);
                }
            }
            _ => {}
        }
    }

    /// Move some number of items up or down the list. Selection will wrap if
    /// it underflows/overflows. Context is required for callbacks.
    fn select_delta(&mut self, context: &mut UpdateContext, delta: isize) {
        // If there's nothing in the list, we can't do anything
        if !self.items.is_empty() {
            let index = match self.state.get_mut().selected() {
                Some(i) => {
                    // Banking on the list not being longer than 2.4B items...
                    (i as isize + delta).rem_euclid(self.items.len() as isize)
                        as usize
                }
                // Nothing selected yet, pick the first item
                None => 0,
            };
            self.select_index(context, index);
        }
    }

    /// Kind-agnostic helper for the selected item
    fn selected_opt(&self) -> Option<&Item> {
        self.items.get(self.state.borrow().selected()?)
    }
}

/// Functions available only on dynamic selects, which may have an empty list
impl<Item, State> SelectState<Dynamic, Item, State>
where
    State: SelectStateData,
{
    pub fn new(items: Vec<Item>) -> Self {
        let mut state = State::default();
        // Pre-select the first item if possible
        if !items.is_empty() {
            state.select(0);
        }
        Self {
            state: RefCell::new(state),
            items,
            on_select: None,
            on_submit: None,
            _kind: PhantomData,
        }
    }

    /// Get the currently selected item (if any)
    pub fn selected(&self) -> Option<&Item> {
        self.items.get(self.state.borrow().selected()?)
    }
}

/// Functions available only on fixed selects, which *cannot* have an empty list
impl<Item, State> SelectState<Fixed, Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    /// Create a new fixed-size list, with options derived from a static enum.
    ///
    /// ## Panics
    ///
    /// Panics if the enum is empty.
    pub fn fixed() -> Self {
        let items = Item::iter().collect_vec();

        if items.is_empty() {
            // Wr run on the assumption that it's not empty, to prevent
            // returning Options
            panic!(
                "Empty fixed-size collection not allow. \
                Add a variant to your enum."
            );
        }

        // Pre-select first item
        let mut state = State::default();
        state.select(0);

        Self {
            state: RefCell::new(state),
            items,
            on_select: None,
            on_submit: None,
            _kind: PhantomData,
        }
    }

    /// Get the index of the currently selected item (if any)
    pub fn selected_index(&self) -> usize {
        // We know the select list is not empty
        self.state.borrow().selected().unwrap()
    }

    /// Get the currently selected item (if any)
    pub fn selected(&self) -> &Item {
        // We know the select list is not empty
        self.selected_opt().unwrap()
    }
}

impl<Item, State> Default for SelectState<Fixed, Item, State>
where
    Item: FixedSelect,
    State: SelectStateData,
{
    fn default() -> Self {
        Self::fixed()
    }
}

/// Handle input events to cycle between items
impl<Kind, Item, State> EventHandler for SelectState<Kind, Item, State>
where
    Kind: SelectStateKind,
    Item: Debug,
    State: Debug + SelectStateData,
{
    fn update(&mut self, context: &mut UpdateContext, event: Event) -> Update {
        match event {
            // Up/down keys/scrolling. Scrolling will only work if .set_area()
            // is called on the wrapping Component by our parent
            Event::Input {
                action: Some(action),
                ..
            } => match action {
                Action::Up | Action::ScrollUp => {
                    self.previous(context);
                    Update::Consumed
                }
                Action::Down | Action::ScrollDown => {
                    self.next(context);
                    Update::Consumed
                }
                Action::Submit => {
                    // If we have an on_submit, our parent wants us to handle
                    // submit events so consume it even if nothing is selected
                    if let Some(on_submit) = &self.on_submit {
                        let selected = self
                            .state
                            .get_mut()
                            .selected()
                            .and_then(|index| self.items.get_mut(index));
                        if let Some(selected) = selected {
                            on_submit(context, selected);
                        }

                        Update::Consumed
                    } else {
                        Update::Propagate(event)
                    }
                }
                _ => Update::Propagate(event),
            },

            _ => Update::Propagate(event),
        }
    }
}

impl<Kind, Item, State> PersistentContainer for SelectState<Kind, Item, State>
where
    Kind: SelectStateKind,
    Item: Persistable,
    Item::Persisted: PartialEq<Item>,
    State: SelectStateData,
{
    type Value = Item;

    fn get(&self) -> Option<&Self::Value> {
        self.selected_opt()
    }

    fn set(&mut self, value: <Self::Value as Persistable>::Persisted) {
        // Do *not* use the helper methods for selecting by value. That requires
        // UpdateContext because it wants to call the callbacks. It'd be nice if
        // we could trigger callbacks here, but with no access to the update
        // context it's not possible.
        if let Some((index, _)) =
            self.items.iter().find_position(|item| &value == *item)
        {
            self.state.get_mut().select(index);
        }
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
    'static
    + Copy
    + Clone
    + Debug
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
        + Display
        + EnumCount
        + IntoEnumIterator
        + PartialEq
{
}
