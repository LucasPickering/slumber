use crate::tui::{
    input::Action,
    message::MessageSender,
    view::{
        draw::{Draw, DrawMetadata},
        event::{Event, EventHandler, Update},
        state::persistence::{Persistable, PersistentContainer},
    },
};
use ratatui::{
    widgets::{ListState, StatefulWidget, TableState},
    Frame,
};
use std::{cell::RefCell, fmt::Debug, marker::PhantomData};

/// State manager for a dynamic list of items.
///
/// This supports a generic type for the state "backend", which is the ratatui
/// type that stores the selection state. Typically you want `ListState` or
/// `TableState`.
#[derive(derive_more::Debug)]
pub struct SelectState<Item, State = ListState>
where
    State: SelectStateData,
{
    /// Use interior mutability because this needs to be modified during the
    /// draw phase, by [ratatui::Frame::render_stateful_widget]. This allows
    /// rendering without a mutable reference.
    state: RefCell<State>,
    #[debug(skip)]
    items: Vec<Item>,
    /// Callback when an item is highlighted
    #[debug(skip)]
    on_select: Option<Callback<Item>>,
    /// Callback when the Submit action is performed on an item
    #[debug(skip)]
    on_submit: Option<Callback<Item>>,
}

/// Builder for [SelectState]. The main reason for the builder is to allow
/// callbacks to be present during state initialization, in case we want to
/// call on_select for the default item.
pub struct SelectStateBuilder<Item, State> {
    items: Vec<Item>,
    /// Store preselected value as an index, so we don't need to care about the
    /// type of the value. Defaults to 0.
    preselect_index: usize,
    on_select: Option<Callback<Item>>,
    on_submit: Option<Callback<Item>>,
    _state: PhantomData<State>,
}

impl<Item, State> SelectStateBuilder<Item, State> {
    /// Set the value that should be initially selected
    pub fn preselect<T>(mut self, value: &T) -> Self
    where
        T: PartialEq<Item>,
    {
        // Our list of items is immutable, so we can safely store just the
        // index. This is useful so we don't need an additional generic on the
        // struct for the ID type.
        if let Some(index) = find_index(&self.items, value) {
            self.preselect_index = index;
        }
        self
    }

    /// Set the value that should be initially selected, if any. This is a
    /// convenience method for when you have an optional value, to avoid an ugly
    /// conditional in calling code.
    pub fn preselect_opt<T>(self, value: Option<&T>) -> Self
    where
        T: PartialEq<Item>,
    {
        if let Some(value) = value {
            self.preselect(value)
        } else {
            self
        }
    }

    /// Set the callback to be called when the user highlights a new item
    pub fn on_select(
        mut self,
        on_select: impl 'static + Fn(&mut Item),
    ) -> Self {
        self.on_select = Some(Box::new(on_select));
        self
    }

    /// Set the callback to be called when the user hits enter on an item
    pub fn on_submit(
        mut self,
        on_submit: impl 'static + Fn(&mut Item),
    ) -> Self {
        self.on_submit = Some(Box::new(on_submit));
        self
    }

    pub fn build(self) -> SelectState<Item, State>
    where
        State: SelectStateData,
    {
        let mut select = SelectState {
            state: RefCell::new(State::default()),
            items: self.items,
            on_select: self.on_select,
            on_submit: self.on_submit,
        };
        // Set initial value. Generally the index will be valid unless the list
        // is empty, because it's either the default of 0 or was derived from
        // a list search. Do a proper bounds check just to be safe though.
        if select.items.len() > self.preselect_index {
            select.select_index(self.preselect_index);
        }
        select
    }
}

type Callback<Item> = Box<dyn Fn(&mut Item)>;

impl<Item, State: SelectStateData> SelectState<Item, State> {
    /// Start a new builder
    pub fn builder(items: Vec<Item>) -> SelectStateBuilder<Item, State> {
        SelectStateBuilder {
            items,
            preselect_index: 0,
            on_select: None,
            on_submit: None,
            _state: PhantomData,
        }
    }

    /// Get all items in the list
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

    /// Select an item by value. Context is required for callbacks. Generally
    /// the given value will be the type `Item`, but it could be anything that
    /// compares to `Item` (e.g. an ID type).
    pub fn select<T: PartialEq<Item>>(&mut self, value: &T) {
        if let Some(index) = find_index(&self.items, value) {
            self.select_index(index);
        }
    }

    /// Select the previous item in the list
    pub fn previous(&mut self) {
        self.select_delta(-1);
    }

    /// Select the next item in the list
    pub fn next(&mut self) {
        self.select_delta(1);
    }

    /// Select an item by index
    fn select_index(&mut self, index: usize) {
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
                    on_select(selected);
                }
            }
            _ => {}
        }
    }

    /// Move some number of items up or down the list. Selection will wrap if
    /// it underflows/overflows. Context is required for callbacks.
    fn select_delta(&mut self, delta: isize) {
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
            self.select_index(index);
        }
    }

    /// Kind-agnostic helper for the selected item
    fn selected_opt(&self) -> Option<&Item> {
        self.items.get(self.state.borrow().selected()?)
    }
}

impl<Item, State> Default for SelectState<Item, State>
where
    Item: PartialEq,
    State: SelectStateData,
{
    fn default() -> Self {
        SelectState::<Item, State>::builder(Vec::new()).build()
    }
}

/// Handle input events to cycle between items
impl<Item, State> EventHandler for SelectState<Item, State>
where
    Item: Debug,
    State: Debug + SelectStateData,
{
    fn update(&mut self, _: &MessageSender, event: Event) -> Update {
        let Some(action) = event.action() else {
            return Update::Propagate(event);
        };
        // Up/down keys and scrolling. Scrolling will only work if .set_area()
        // is called on the wrapping Component by our parent
        match action {
            Action::Up | Action::ScrollUp => self.previous(),
            Action::Down | Action::ScrollDown => self.next(),
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
                        on_submit(selected);
                    }
                } else {
                    return Update::Propagate(event);
                }
            }
            _ => return Update::Propagate(event),
        }
        Update::Consumed
    }
}

/// Support rendering if the parent tells us exactly what to draw. This makes it
/// easy to track the area that a component is drawn to, so we always receive
/// the appropriate cursor events. It's impossible to draw the select component
/// in another way because of the restricted access to the inner state.
impl<Item, State, W> Draw<W> for SelectState<Item, State>
where
    State: SelectStateData,
    W: StatefulWidget<State = State>,
{
    fn draw(&self, frame: &mut Frame, props: W, metadata: DrawMetadata) {
        frame.render_stateful_widget(
            props,
            metadata.area(),
            &mut self.state.borrow_mut(),
        );
    }
}

impl<Item, State> PersistentContainer for SelectState<Item, State>
where
    Item: Persistable,
    // Whatever is persisted in the DB needs to be comparable to the items in
    // the list, so we can select by equality
    Item::Persisted: PartialEq<Item>,
    State: SelectStateData,
{
    type Value = Item;

    fn get(&self) -> Option<&Self::Value> {
        self.selected_opt()
    }

    fn set(&mut self, value: <Self::Value as Persistable>::Persisted) {
        // This will call the on_select callback if the item is in the list
        self.select(&value);
    }
}

/// Inner state for [SelectState] and [FixedSelectState]. This is an abstraction
/// to allow them to support multiple state "backends" from Ratatui, to enable
/// usage with different stateful widgets.
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

/// Find the index of a value in the list
fn find_index<Item, T>(items: &[Item], value: &T) -> Option<usize>
where
    T: PartialEq<Item>,
{
    items.iter().position(|item| value == item)
}
