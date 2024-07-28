use crate::view::{
    draw::{Draw, DrawMetadata},
    event::{Event, EventHandler, Update},
};
use persisted::PersistedContainer;
use ratatui::{
    widgets::{ListState, StatefulWidget, TableState},
    Frame,
};
use slumber_config::Action;
use slumber_core::collection::HasId;
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
    items: Vec<SelectItem<Item>>,
    /// Callback when an item is highlighted
    #[debug(skip)]
    on_select: Option<Callback<Item>>,
    /// Callback when the Toggle action is performed on an item
    #[debug(skip)]
    on_toggle: Option<Callback<Item>>,
    /// Callback when the Submit action is performed on an item
    #[debug(skip)]
    on_submit: Option<Callback<Item>>,
}

/// An item in a select list, with additional metadata
#[derive(Debug)]
pub struct SelectItem<T> {
    pub value: T,
    /// If an item is disabled, we'll skip over it during selections
    pub disabled: bool,
}

/// Builder for [SelectState]. The main reason for the builder is to allow
/// callbacks to be present during state initialization, in case we want to
/// call on_select for the default item.
pub struct SelectStateBuilder<Item, State> {
    items: Vec<SelectItem<Item>>,
    /// Store preselected value as an index, so we don't need to care about the
    /// type of the value. Defaults to 0.
    preselect_index: usize,
    on_select: Option<Callback<Item>>,
    on_toggle: Option<Callback<Item>>,
    on_submit: Option<Callback<Item>>,
    _state: PhantomData<State>,
}

impl<Item, State> SelectStateBuilder<Item, State> {
    /// Disable certain items in the list by value. Disabled items can still be
    /// selected, but do not trigger callbacks.
    pub fn disabled_items<'a, T>(
        mut self,
        disabled_items: impl IntoIterator<Item = &'a T>,
    ) -> Self
    where
        T: 'a + PartialEq<Item>,
    {
        // O(n^2)! We expect both lists to be very small so it's not an issue
        for disabled in disabled_items {
            for item in &mut self.items {
                if disabled == &item.value {
                    item.disabled = true;
                }
            }
        }
        self
    }

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

    /// Set the callback to be called when the user hits space on an item
    pub fn on_toggle(
        mut self,
        on_toggle: impl 'static + Fn(&mut Item),
    ) -> Self {
        self.on_toggle = Some(Box::new(on_toggle));
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
            state: RefCell::default(),
            items: self.items,
            on_select: self.on_select,
            on_toggle: self.on_toggle,
            on_submit: self.on_submit,
        };
        // Set initial value. Generally the index will be valid unless the list
        // is empty, because it's either the default of 0 or was derived from
        // a list search. Do a proper bounds check just to be safe though.
        if self.preselect_index < select.items.len() {
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
            items: items
                .into_iter()
                .map(|item| SelectItem {
                    value: item,
                    disabled: false,
                })
                .collect(),
            preselect_index: 0,
            on_select: None,
            on_toggle: None,
            on_submit: None,
            _state: PhantomData,
        }
    }

    /// Get all items in the list
    pub fn items(&self) -> &[SelectItem<Item>] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Get the index of the currently selected item (if any)
    pub fn selected_index(&self) -> Option<usize> {
        self.state.borrow().selected()
    }

    /// Get the currently selected item (if any)
    pub fn selected(&self) -> Option<&Item> {
        self.items
            .get(self.state.borrow().selected()?)
            .map(|item| &item.value)
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
                let selected = new.and_then(|index| self.items.get_mut(index));
                // Don't call callbacks for disabled items
                match selected {
                    Some(selected) if !selected.disabled => {
                        on_select(&mut selected.value);
                    }
                    _ => {}
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
    fn update(&mut self, event: Event) -> Update {
        let Some(action) = event.action() else {
            return Update::Propagate(event);
        };
        // Up/down keys and scrolling. Scrolling will only work if .set_area()
        // is called on the wrapping Component by our parent
        match action {
            Action::Up | Action::ScrollUp => self.previous(),
            Action::Down | Action::ScrollDown => self.next(),
            Action::Toggle => {
                // If we have an on_toggle, our parent wants us to handle
                // toggle events so consume it even if nothing is selected
                if let Some(on_toggle) = &self.on_toggle {
                    let selected = self
                        .state
                        .get_mut()
                        .selected()
                        .and_then(|index| self.items.get_mut(index));
                    if let Some(selected) = selected {
                        on_toggle(&mut selected.value);
                    }
                } else {
                    return Update::Propagate(event);
                }
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
                    // Don't call callbacks for disabled items
                    match selected {
                        Some(selected) if !selected.disabled => {
                            on_submit(&mut selected.value);
                        }
                        _ => {}
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
/// easy to track the area that a select is drawn to, so we always receive
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

impl<Item, State> PersistedContainer for SelectState<Item, State>
where
    Item: HasId,
    Item::Id: PartialEq<Item>, // Bound needed so we can select items by ID
    State: SelectStateData,
{
    type Value = Option<Item::Id>;

    fn get_persisted(&self) -> Self::Value {
        self.selected().map(Item::id).cloned()
    }

    fn set_persisted(&mut self, value: Self::Value) {
        // If we persisted `None`, we *don't* want to update state here. That
        // means the list was empty before persisting and it may now have data,
        // and we don't want to overwrite whatever was pre-selected
        if let Some(value) = &value {
            // This will call the on_select callback if the item is in the list
            self.select(value);
        }
    }
}

/// Inner state for [SelectState]. This is an abstraction to allow it to support
/// multiple state "backends" from Ratatui, to enable usage with different
/// stateful widgets.
pub trait SelectStateData: Default {
    /// Index of the selected element
    fn selected(&self) -> Option<usize>;

    /// Select an element by index
    fn select(&mut self, index: usize);
}

impl SelectStateData for ListState {
    fn selected(&self) -> Option<usize> {
        self.selected()
    }

    fn select(&mut self, index: usize) {
        self.select(Some(index))
    }
}

impl SelectStateData for TableState {
    fn selected(&self) -> Option<usize> {
        self.selected()
    }

    fn select(&mut self, index: usize) {
        self.select(Some(index))
    }
}

impl SelectStateData for usize {
    fn selected(&self) -> Option<usize> {
        Some(*self)
    }

    fn select(&mut self, index: usize) {
        *self = index;
    }
}

/// Find the index of a value in the list
fn find_index<Item, T>(items: &[SelectItem<Item>], value: &T) -> Option<usize>
where
    T: PartialEq<Item>,
{
    items.iter().position(|item| value == &item.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{harness, TestHarness},
        view::{context::PersistedLazy, test_util::TestComponent, ViewContext},
    };
    use crossterm::event::KeyCode;
    use persisted::{PersistedKey, PersistedStore};
    use ratatui::widgets::List;
    use rstest::{fixture, rstest};
    use serde::Serialize;
    use slumber_core::{
        collection::{Profile, ProfileId},
        test_util::Factory,
    };
    use std::sync::mpsc;

    /// Test going up and down in the list
    #[rstest]
    fn test_navigation(
        harness: TestHarness,
        items: (Vec<&'static str>, List<'static>),
    ) {
        let select = SelectState::builder(items.0).build();
        let mut component = TestComponent::new(harness, select, items.1);
        assert_eq!(component.data().selected(), Some(&"a"));
        component.send_key(KeyCode::Down).assert_empty();
        assert_eq!(component.data().selected(), Some(&"b"));

        component.send_key(KeyCode::Up).assert_empty();
        assert_eq!(component.data().selected(), Some(&"a"));
    }

    /// Test on_select callback
    #[rstest]
    fn test_on_select(
        harness: TestHarness,
        items: (Vec<&'static str>, List<'static>),
    ) {
        // Track calls to the callback
        let (tx, rx) = mpsc::channel();

        let select = SelectState::builder(items.0)
            .disabled_items(&["c"])
            .on_select(move |item| tx.send(*item).unwrap())
            .build();
        let mut component = TestComponent::new(harness, select, items.1);

        assert_eq!(component.data().selected(), Some(&"a"));
        assert_eq!(rx.recv().unwrap(), "a");
        component.send_key(KeyCode::Down).assert_empty();
        assert_eq!(rx.recv().unwrap(), "b");

        // "c" is disabled, should not trigger callback
        component.send_key(KeyCode::Down).assert_empty();
        assert!(rx.try_recv().is_err());
    }

    /// Test on_submit callback
    #[rstest]
    fn test_on_submit(
        harness: TestHarness,
        items: (Vec<&'static str>, List<'static>),
    ) {
        // Track calls to the callback
        let (tx, rx) = mpsc::channel();

        let select = SelectState::builder(items.0)
            .disabled_items(&["c"])
            .on_submit(move |item| tx.send(*item).unwrap())
            .build();
        let mut component = TestComponent::new(harness, select, items.1);

        component.send_key(KeyCode::Down).assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(rx.recv().unwrap(), "b");

        // "c" is disabled, should not trigger callback
        component.send_key(KeyCode::Down).assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert!(rx.try_recv().is_err());
    }

    /// Test persisting selected item
    #[rstest]
    fn test_persistence(_harness: TestHarness) {
        #[derive(Debug, PersistedKey, Serialize)]
        #[persisted(Option<ProfileId>)]
        struct Key;

        let profile = Profile::factory(());
        let profile_id = profile.id.clone();

        ViewContext::store_persisted(&Key, Some(profile_id.clone()));

        let pid = profile_id.clone();
        let select = PersistedLazy::new(
            Key,
            SelectState::<_, usize>::builder(vec![profile])
                .on_select(move |item| assert_eq!(item.id, pid))
                .build(),
        );
        assert_eq!(select.selected().map(Profile::id), Some(&profile_id));
    }

    #[fixture]
    fn items() -> (Vec<&'static str>, List<'static>) {
        let items = vec!["a", "b", "c"];
        let list = items.iter().copied().collect();
        (items, list)
    }
}
