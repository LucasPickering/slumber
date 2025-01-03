use crate::view::{
    context::UpdateContext,
    draw::{Draw, DrawMetadata},
    event::{Emitter, EmitterId, Event, EventHandler, OptionEvent},
};
use persisted::PersistedContainer;
use ratatui::{
    widgets::{ListState, StatefulWidget, TableState},
    Frame,
};
use slumber_config::Action;
use slumber_core::collection::HasId;
use std::{
    cell::RefCell,
    fmt::Debug,
    marker::PhantomData,
    ops::{Index, IndexMut},
};
use strum::EnumDiscriminants;

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
    emitter_id: EmitterId,
    /// Which event types to emit
    subscribed_events: Vec<SelectStateEventType>,
    /// Use interior mutability because this needs to be modified during the
    /// draw phase, by [ratatui::Frame::render_stateful_widget]. This allows
    /// rendering without a mutable reference.
    state: RefCell<State>,
    items: Vec<SelectItem<Item>>,
}

/// An item in a select list, with additional metadata
#[derive(Debug)]
pub struct SelectItem<T> {
    pub value: T,
    /// If an item is disabled, we'll skip over it during selections
    disabled: bool,
}

impl<T> SelectItem<T> {
    pub fn disabled(&self) -> bool {
        self.disabled
    }
}

/// Builder for [SelectState]. The main reason for the builder is to allow
/// setting the preselect index, so the wrong index doesn't get an event emitted
/// for it
pub struct SelectStateBuilder<Item, State> {
    items: Vec<SelectItem<Item>>,
    /// Store preselected value as an index, so we don't need to care about the
    /// type of the value. Defaults to 0.
    preselect_index: usize,
    subscribed_events: Vec<SelectStateEventType>,
    _state: PhantomData<State>,
}

impl<Item, State> SelectStateBuilder<Item, State> {
    /// Disable certain items in the list by value. Disabled items can still be
    /// selected, but do not emit events.
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

    /// Which types of events should this emit?
    pub fn subscribe(
        mut self,
        event_types: impl IntoIterator<Item = SelectStateEventType>,
    ) -> Self {
        self.subscribed_events.extend(event_types);
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

    pub fn build(self) -> SelectState<Item, State>
    where
        State: SelectStateData,
    {
        let mut select = SelectState {
            emitter_id: EmitterId::new(),
            subscribed_events: self.subscribed_events,
            state: RefCell::default(),
            items: self.items,
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
            subscribed_events: Vec::new(),
            _state: PhantomData,
        }
    }

    /// Get all items in the list
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().map(|item| &item.value)
    }

    /// Get all items in the list, including each one's metadata
    pub fn items_with_metadata(
        &self,
    ) -> impl Iterator<Item = &SelectItem<Item>> {
        self.items.iter()
    }

    /// Get mutable references to all items in the list
    pub fn items_mut(&mut self) -> &mut [SelectItem<Item>] {
        &mut self.items
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
            .get(self.selected_index()?)
            .map(|item| &item.value)
    }

    /// Mutable reference to the currently selected item (if any)
    pub fn selected_mut(&mut self) -> Option<&mut Item> {
        let index = self.selected_index()?;
        self.items.get_mut(index).map(|item| &mut item.value)
    }

    /// Select an item by value. Generally the given value will be the type
    /// `Item`, but it could be anything that compares to `Item` (e.g. an ID
    /// type).
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

        // If the selection changed, send an event
        if current != new {
            self.emit_for_selected(SelectStateEvent::Select);
        }
    }

    /// Move some number of items up or down the list. Selection will wrap if
    /// it underflows/overflows.
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

    /// Helper to generate an emit an event for the currentl selected item. The
    /// event will *not* be emitted if no item is select, or the selected item
    /// is disabled.
    fn emit_for_selected(&self, event_fn: impl Fn(usize) -> SelectStateEvent) {
        // 2024 edition: if-let chain
        match self.selected_index() {
            // Don't send event for disabled items
            Some(selected) if !self.items[selected].disabled => {
                let event = event_fn(selected);
                // Check if the parent subscribed to this event type
                if self.is_subscribed(SelectStateEventType::from(&event)) {
                    self.emit(event);
                }
            }
            _ => {}
        }
    }

    fn is_subscribed(&self, event_type: SelectStateEventType) -> bool {
        self.subscribed_events.contains(&event_type)
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

/// Get an item by index, and panic if out of bounds. Useful with emitted
/// events, when we know the index will be valid
impl<Item, State> Index<usize> for SelectState<Item, State>
where
    State: SelectStateData,
{
    type Output = Item;

    fn index(&self, index: usize) -> &Self::Output {
        &self.items[index].value
    }
}

impl<Item, State> IndexMut<usize> for SelectState<Item, State>
where
    State: SelectStateData,
{
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.items[index].value
    }
}

/// Handle input events to cycle between items
impl<Item, State> EventHandler for SelectState<Item, State>
where
    Item: Debug,
    State: Debug + SelectStateData,
{
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().action(|action, propagate| match action {
            // Up/down keys and scrolling. Scrolling will only work if
            // .set_area() is called on the wrapping Component by our parent
            Action::Up | Action::ScrollUp => self.previous(),
            Action::Down | Action::ScrollDown => self.next(),
            // Don't eat these events unless the user has subscribed
            Action::Toggle
                if self.is_subscribed(SelectStateEventType::Toggle) =>
            {
                self.emit_for_selected(SelectStateEvent::Toggle)
            }
            Action::Submit
                if self.is_subscribed(SelectStateEventType::Submit) =>
            {
                self.emit_for_selected(SelectStateEvent::Submit)
            }
            _ => propagate.set(),
        })
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

    fn get_to_persist(&self) -> Self::Value {
        self.selected().map(Item::id).cloned()
    }

    fn restore_persisted(&mut self, value: Self::Value) {
        // If we persisted `None`, we *don't* want to update state here. That
        // means the list was empty before persisting and it may now have data,
        // and we don't want to overwrite whatever was pre-selected
        if let Some(value) = &value {
            // This will emit a select event if the item is in the list
            self.select(value);
        }
    }
}

impl<Item, State: SelectStateData> Emitter for SelectState<Item, State> {
    type Emitted = SelectStateEvent;

    fn id(&self) -> EmitterId {
        self.emitter_id
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

/// Emitted event for select list
#[derive(Debug, EnumDiscriminants)]
#[strum_discriminants(name(SelectStateEventType))]
#[cfg_attr(test, derive(PartialEq))]
pub enum SelectStateEvent {
    /// User highlight a new item in the list
    Select(usize),
    /// User hit submit button (Enter by default) on an item
    Submit(usize),
    /// User hit toggle button (Space by default) on an item
    Toggle(usize),
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
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::{
            test_util::TestComponent,
            util::persistence::{DatabasePersistedStore, PersistedLazy},
        },
    };
    use crossterm::event::KeyCode;
    use persisted::{PersistedKey, PersistedStore};
    use ratatui::widgets::List;
    use rstest::{fixture, rstest};
    use serde::Serialize;
    use slumber_core::{assert_matches, collection::ProfileId};

    /// Test going up and down in the list
    #[rstest]
    fn test_navigation(
        harness: TestHarness,
        terminal: TestTerminal,
        items: (Vec<&'static str>, List<'static>),
    ) {
        let select = SelectState::builder(items.0).build();
        let mut component =
            TestComponent::with_props(&harness, &terminal, select, items.1);
        component.drain_draw().assert_empty();
        assert_eq!(component.data().selected(), Some(&"a"));
        component.send_key(KeyCode::Down).assert_empty();
        assert_eq!(component.data().selected(), Some(&"b"));

        component.send_key(KeyCode::Up).assert_empty();
        assert_eq!(component.data().selected(), Some(&"a"));
    }

    /// Test select emitted event
    #[rstest]
    fn test_select(
        harness: TestHarness,
        terminal: TestTerminal,
        items: (Vec<&'static str>, List<'static>),
    ) {
        let select = SelectState::builder(items.0)
            .disabled_items(&["c"])
            .subscribe([SelectStateEventType::Select])
            .build();
        let mut component =
            TestComponent::with_props(&harness, &terminal, select, items.1);

        // Initial selection
        assert_eq!(component.data().selected(), Some(&"a"));
        component
            .drain_draw()
            .assert_emitted([SelectStateEvent::Select(0)]);

        component
            .send_key(KeyCode::Down)
            .assert_emitted([SelectStateEvent::Select(1)]);

        // "c" is disabled, should not trigger events
        component.send_key(KeyCode::Down).assert_empty();
    }

    /// Test submit emitted event
    #[rstest]
    fn test_submit(
        harness: TestHarness,
        terminal: TestTerminal,
        items: (Vec<&'static str>, List<'static>),
    ) {
        let select = SelectState::builder(items.0)
            .disabled_items(&["c"])
            .subscribe([SelectStateEventType::Submit])
            .build();
        let mut component =
            TestComponent::with_props(&harness, &terminal, select, items.1);
        component.drain_draw().assert_empty();

        component
            .send_keys([KeyCode::Down, KeyCode::Enter])
            .assert_emitted([SelectStateEvent::Submit(1)]);

        // "c" is disabled, should not trigger events
        component
            .send_keys([KeyCode::Down, KeyCode::Enter])
            .assert_empty();
    }

    /// Test that submit and toggle input events are propagated if we're not
    /// subscribed to them
    #[rstest]
    fn test_propagate(
        harness: TestHarness,
        terminal: TestTerminal,
        items: (Vec<&'static str>, List<'static>),
    ) {
        let select = SelectState::builder(items.0).build();
        let mut component =
            TestComponent::with_props(&harness, &terminal, select, items.1);

        assert_matches!(
            component.send_key(KeyCode::Enter).events(),
            &[Event::Input {
                action: Some(Action::Submit),
                ..
            }]
        );
        assert_matches!(
            component.send_key(KeyCode::Char(' ')).events(),
            &[Event::Input {
                action: Some(Action::Toggle),
                ..
            }]
        );
    }

    /// Test persisting selected item
    #[rstest]
    fn test_persistence(harness: TestHarness, terminal: TestTerminal) {
        #[derive(Debug, PersistedKey, Serialize)]
        #[persisted(Option<ProfileId>)]
        struct Key;

        #[derive(Debug)]
        struct ProfileItem(ProfileId);

        impl HasId for ProfileItem {
            type Id = ProfileId;

            fn id(&self) -> &Self::Id {
                &self.0
            }

            fn set_id(&mut self, id: Self::Id) {
                self.0 = id;
            }
        }

        impl PartialEq<ProfileItem> for ProfileId {
            fn eq(&self, item: &ProfileItem) -> bool {
                self == &item.0
            }
        }

        let profile_id: ProfileId = "profile2".into();
        let profile = ProfileItem(profile_id.clone());

        DatabasePersistedStore::store_persisted(
            &Key,
            &Some(profile_id.clone()),
        );

        // Second profile should be pre-selected because of persistence
        let select =
            SelectState::builder(vec![ProfileItem("profile1".into()), profile])
                .subscribe([SelectStateEventType::Select])
                .build();
        let list = select
            .items()
            .map(|item| item.0.to_string())
            .collect::<List>();
        let mut component = TestComponent::with_props(
            &harness,
            &terminal,
            PersistedLazy::new(Key, select),
            list,
        );
        assert_eq!(
            component.data().selected().map(ProfileItem::id),
            Some(&profile_id)
        );
        component.drain_draw().assert_emitted([
            // First item gets selected by preselection, second by persistence
            SelectStateEvent::Select(0),
            SelectStateEvent::Select(1),
        ])
    }

    #[fixture]
    fn items() -> (Vec<&'static str>, List<'static>) {
        let items = vec!["a", "b", "c"];
        let list = items.iter().copied().collect();
        (items, list)
    }
}
