use crate::{
    context::TuiContext,
    view::{
        Generate,
        common::{scrollbar::Scrollbar, table::Table},
        component::{
            Canvas, Component, ComponentExt, ComponentId, Draw, DrawMetadata,
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch, ToEmitter},
    },
};
use itertools::Itertools;
use persisted::PersistedContainer;
use ratatui::{
    style::Styled,
    text::Text,
    widgets::{
        List, ListDirection, ListItem, ListState, StatefulWidget, TableState,
    },
};
use slumber_config::Action;
use slumber_core::collection::HasId;
use std::{
    cell::RefCell,
    collections::HashSet,
    fmt::Debug,
    marker::PhantomData,
    ops::{Index, IndexMut},
};
use strum::EnumDiscriminants;
use terminput::ScrollDirection;
use tracing::error;

/// A dynamic list of items
///
/// This supports a generic type for the state "backend", which is the ratatui
/// type that stores the selection state. Typically you want `ListState` or
/// `TableState`.
///
/// ## Drawing
///
/// As this supports multiple semantic meanings with different state backends,
/// it has multiple [Draw] implementations. Each implementation has an explicit
/// prop type to make it easy to specify which implementation you want.
///
/// - List: [SelectListProps]
/// - Table: [SelectTableProps]
///
/// Drawing has to be done through these implementations, rather than adhoc
/// `render_widget` calls, because this is a [Component]. It must be drawn to
/// the canvas into order to receive events.
#[derive(derive_more::Debug)]
pub struct Select<Item, State = ListState>
where
    State: SelectData,
{
    id: ComponentId,
    emitter: Emitter<SelectEvent>,
    /// Direction of items, for both display and interaction. In top-to-bottom,
    /// index 0 is at the top, Up decrements the index, and Down increments it.
    /// In bottom-to-top, index 0 is at the bottom, Up increments the index,
    /// and Down decrements it.
    direction: ListDirection,
    /// Which event types to emit
    subscribed_events: HashSet<SelectEventType>,
    /// Use interior mutability because this needs to be modified during the
    /// draw phase, by [ratatui::Frame::render_stateful_widget]. This allows
    /// rendering without a mutable reference.
    state: RefCell<State>,
    items: Vec<SelectItem<Item>>,
}

/// An item in a select list, with additional metadata
#[derive(Debug, PartialEq)]
pub struct SelectItem<T> {
    pub value: T,
    /// If an item is disabled, we'll skip over it during selections
    enabled: bool,
}

impl<T> SelectItem<T> {
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// Builder for [Select]. The main reason for the builder is to allow
/// setting the preselect index, so the wrong index doesn't get an event emitted
/// for it
pub struct SelectBuilder<Item, State> {
    items: Vec<SelectItem<Item>>,
    /// Store preselected value as an index, so we don't need to care about the
    /// type of the value. Defaults to 0. If a filter is give, this will be
    /// the index *after* the filter is applied.
    preselect_index: usize,
    direction: ListDirection,
    subscribed_events: HashSet<SelectEventType>,
    _state: PhantomData<State>,
}

impl<Item, State> SelectBuilder<Item, State> {
    /// Disable certain items in the list by index. Disabled items can still be
    /// selected, but do not emit events.
    pub fn disabled_indexes(
        mut self,
        disabled_indexes: impl IntoIterator<Item = usize>,
    ) -> Self {
        for index in disabled_indexes {
            // A slight UI bug is better than a crash, so if the index is
            // unknown just log it
            if let Some(item) = self.items.get_mut(index) {
                item.enabled = false;
            } else {
                error!("Disabled index {index} out of bounds");
            }
        }
        self
    }

    /// Set list display direction
    pub fn direction(mut self, direction: ListDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Which types of events should this emit?
    pub fn subscribe(
        mut self,
        event_types: impl IntoIterator<Item = SelectEventType>,
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

    /// Apply a case-insensitive text filter to the list. Any item whose label
    /// does not contain the text will be excluded from the list. During the
    /// build, this should be called *before* any `preselect` functions to
    /// ensure you don't select a value that will then be filtered out.
    pub fn filter(mut self, filter: &str) -> Self
    where
        Item: ToString,
    {
        let filter = filter.trim();
        if !filter.is_empty() {
            let filter = filter.to_lowercase();
            self.items.retain(|item| {
                item.value.to_string().to_lowercase().contains(&filter)
            });
        }
        self
    }

    pub fn build(self) -> Select<Item, State>
    where
        State: SelectData,
    {
        let mut select = Select {
            id: ComponentId::default(),
            emitter: Default::default(),
            direction: self.direction,
            subscribed_events: self.subscribed_events,
            state: RefCell::default(),
            items: self.items,
        };

        // Set initial value. The given index can be invalid in three ways:
        // - That item is disabled. Select the first enabled item
        // - Index is out of bounds. Select the first enabled item
        // - List is empty. Select nothing
        if select
            .items
            .get(self.preselect_index)
            .is_some_and(SelectItem::enabled)
        {
            select.select_index(self.preselect_index);
        } else if let Some(index) = select.first_enabled_index() {
            select.select_index(index);
        }

        select
    }
}

impl<Item, State: SelectData> Select<Item, State> {
    /// Start a new builder
    pub fn builder(items: Vec<Item>) -> SelectBuilder<Item, State> {
        SelectBuilder {
            items: items
                .into_iter()
                .map(|item| SelectItem {
                    value: item,
                    enabled: true,
                })
                .collect(),
            direction: ListDirection::TopToBottom,
            preselect_index: 0,
            subscribed_events: HashSet::new(),
            _state: PhantomData,
        }
    }

    /// Get all items in the list
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().map(|item| &item.value)
    }

    /// Get mutable references to all items in the list
    pub fn items_mut(&mut self) -> &mut [SelectItem<Item>] {
        &mut self.items
    }

    /// Get all items in the list, including each one's metadata
    pub fn items_with_metadata(
        &self,
    ) -> impl Iterator<Item = &SelectItem<Item>> {
        self.items.iter()
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

    /// Move the selected item out of the list, if there is any
    pub fn into_selected(mut self) -> Option<Item> {
        let index = self.selected_index()?;
        Some(self.items.swap_remove(index).value)
    }

    /// Select an item by value. Generally the given value will be the type
    /// `Item`, but it could be anything that compares to `Item` (e.g. an ID
    /// type).
    pub fn select<T: PartialEq<Item>>(&mut self, value: &T) {
        if let Some(index) = find_index(&self.items, value) {
            self.select_index(index);
        }
    }

    /// Select an item by index
    pub fn select_index(&mut self, index: usize) {
        // If the chosen item is out of bounds or disabled, do nothing
        if self.items.get(index).is_none_or(|item| !item.enabled) {
            return;
        }

        let state = self.state.get_mut();
        let current = state.selected();
        state.select(Some(index));
        let new = state.selected();

        // If the selection changed, send an event
        if current != new {
            self.emit_for_selected(SelectEvent::Select);
        }
    }

    /// Select the item above the selected item. For top-to-bottom lists, this
    /// is the previous item. For bottom-to-top, it's the next.
    pub fn up(&mut self) {
        let delta = match self.direction {
            ListDirection::TopToBottom => -1,
            ListDirection::BottomToTop => 1,
        };
        self.select_delta(delta);
    }

    /// Select the item below the selected item. For top-to-bottom lists, this
    /// is the next item. For bottom-to-top, it's the previous.
    pub fn down(&mut self) {
        let delta = match self.direction {
            ListDirection::TopToBottom => 1,
            ListDirection::BottomToTop => -1,
        };
        self.select_delta(delta);
    }

    /// Remove the selected item from the list. This will slide everything after
    /// that item up one slot, so that the item after it is selected. If the
    /// selected item was at the end of the list, the item before it will be
    /// selected. If the list is now empty, the selection is cleared.
    pub fn delete_selected(&mut self) {
        let state = self.state.get_mut();
        let selected_index = state.selected();
        if let Some(index) = selected_index {
            self.items.remove(index);

            // There are two ways the selection could now be invalid:
            // - Deleted the only item in the list. Clear the selection
            // - Deleted the item at the end of the list. Select the item before
            //   it
            // Otherwise, we implicitly selected the item after the deleted one,
            // and can proceed as usual
            if self.items.is_empty() {
                state.select(None);
            } else if index == self.items.len() {
                state.select(Some(index - 1));
            }

            // This will do nothing if the selection is now empty
            self.emit_for_selected(SelectEvent::Select);
        }
    }

    /// Move some number of items up or down the list. Selection will wrap if
    /// it underflows/overflows.
    fn select_delta(&mut self, delta: isize) {
        // Get a list of which items are enabled, where the first item is the
        // currently selected one. We'll use this to figure out what item we
        // select after n steps forward/back.
        let mut enabled_indexes = (0..self.items.len()).collect_vec();
        enabled_indexes.rotate_left(self.selected_index().unwrap_or(0));
        // Filter out disabled items so they get skipped over
        enabled_indexes.retain(|index| self.items[*index].enabled);

        // If there are no enabled items, there's nothing we can do
        if enabled_indexes.is_empty() {
            return;
        }

        // Banking on the list not being longer than 2.4B items...
        let index_index =
            delta.rem_euclid(enabled_indexes.len() as isize) as usize;
        self.select_index(enabled_indexes[index_index]);
    }

    /// Get the index of the first item in the list that isn't disabled. `None`
    /// if the list is empty or all items are disabled
    fn first_enabled_index(&self) -> Option<usize> {
        self.items.iter().position(SelectItem::enabled)
    }

    /// Helper to generate an emit an event for the currentl selected item. The
    /// event will *not* be emitted if no item is select, or the selected item
    /// is disabled.
    fn emit_for_selected(&self, event_fn: impl Fn(usize) -> SelectEvent) {
        if let Some(selected) = self.selected_index() {
            let event = event_fn(selected);
            // Check if the parent subscribed to this event type
            if self.is_subscribed(SelectEventType::from(&event)) {
                self.emitter.emit(event);
            }
        }
    }

    fn is_subscribed(&self, event_type: SelectEventType) -> bool {
        self.subscribed_events.contains(&event_type)
    }
}

impl<Item, State> Default for Select<Item, State>
where
    State: SelectData,
{
    fn default() -> Self {
        Select::<Item, State>::builder(Vec::new()).build()
    }
}

/// Get an item by index, and panic if out of bounds. Useful with emitted
/// events, when we know the index will be valid
impl<Item, State> Index<usize> for Select<Item, State>
where
    State: SelectData,
{
    type Output = Item;

    fn index(&self, index: usize) -> &Self::Output {
        &self.items[index].value
    }
}

impl<Item, State> IndexMut<usize> for Select<Item, State>
where
    State: SelectData,
{
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.items[index].value
    }
}

impl<Item, State> Component for Select<Item, State>
where
    State: SelectData,
{
    fn id(&self) -> ComponentId {
        self.id
    }

    // Handle input events to cycle between items
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .click(|position, propagate| {
                // Map the position to the relative to our top-left. Each item
                // is one row, so the index is just the y position.
                //
                // Area should always be Some because this can't receive events
                // if it's not visible, but check to be safe
                if let Some(area) = self.area() {
                    let clicked_index = (position.y - area.y) as usize;
                    if clicked_index < self.items.len() {
                        self.select_index(clicked_index);
                    }
                }

                // Always propagate this click, because the parent may be a
                // pane that uses it to select itself
                propagate.set();
            })
            .scroll(|direction| match direction {
                ScrollDirection::Up => self.up(),
                ScrollDirection::Down => self.down(),
                ScrollDirection::Left | ScrollDirection::Right => {}
            })
            .action(|action, propagate| match action {
                // Up/down keys and scrolling. Scrolling will only work if
                // .set_area() is called on the wrapping Component by our parent
                Action::Up | Action::ScrollUp => self.up(),
                Action::Down | Action::ScrollDown => self.down(),
                // Don't eat these events unless the user has subscribed
                Action::Toggle
                    if self.is_subscribed(SelectEventType::Toggle) =>
                {
                    self.emit_for_selected(SelectEvent::Toggle);
                }
                Action::Submit
                    if self.is_subscribed(SelectEventType::Submit) =>
                {
                    self.emit_for_selected(SelectEvent::Submit);
                }
                _ => propagate.set(),
            })
    }
}

impl<Item, State> PersistedContainer for Select<Item, State>
where
    Item: HasId,
    // PartialEq needed so we can select items by ID
    Item::Id: Clone + PartialEq<Item>,
    State: SelectData,
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

impl<Item, State> ToEmitter<SelectEvent> for Select<Item, State>
where
    State: SelectData,
{
    fn to_emitter(&self) -> Emitter<SelectEvent> {
        self.emitter
    }
}

/// Props for rendering [Select] as a list
///
/// Currently this is empty, but it serves as a type parameter so callers can
/// easily specify which impl of [Draw] they want.
#[derive(Default)]
pub struct SelectListProps;

/// Render as a list
///
/// Item has to generate() to something that converts to Text
impl<Item> Draw<SelectListProps> for Select<Item, ListState>
where
    for<'a> &'a Item: Generate,
    for<'a> <&'a Item as Generate>::Output<'a>: Into<Text<'a>>,
{
    fn draw(
        &self,
        canvas: &mut Canvas,
        _: SelectListProps,
        metadata: DrawMetadata,
    ) {
        let styles = &TuiContext::get().styles.list;

        // Draw list
        let items: Vec<ListItem<'_>> = self
            .items
            .iter()
            .map(|item| {
                let mut list_item = ListItem::new(item.value.generate());
                if !item.enabled {
                    list_item = list_item.set_style(styles.disabled);
                }
                list_item
            })
            .collect();
        let num_items = items.len();
        // Highlight styling is based on focus. Useful for layered action menus
        let highlight_style = if metadata.has_focus() {
            styles.highlight
        } else {
            styles.highlight_inactive
        };
        let list = List::new(items)
            .highlight_style(highlight_style)
            .direction(self.direction);
        let area = metadata.area();
        let state = &mut self.state.borrow_mut();
        canvas.render_stateful_widget(list, area, state);

        // Draw scrollbar
        canvas.render_widget(
            Scrollbar {
                content_length: num_items,
                offset: state.offset(),
                ..Default::default()
            },
            metadata.area(),
        );
    }
}

/// Props for rendering [Select] as a table
pub struct SelectTableProps<'a, const COLS: usize, R> {
    /// Format to render as. This allows the parent to determine the columns
    /// and rows of the table; the [Select] just provides the selected row
    /// state.
    pub table: Table<'a, COLS, R>,
}

/// Render as a table
impl<'a, const COLS: usize, R, Item> Draw<SelectTableProps<'a, COLS, R>>
    for Select<Item, TableState>
where
    Table<'a, COLS, R>: StatefulWidget<State = TableState>,
{
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: SelectTableProps<'a, COLS, R>,
        metadata: DrawMetadata,
    ) {
        canvas.render_stateful_widget(
            props.table,
            metadata.area(),
            &mut self.state.borrow_mut(),
        );
    }
}

/// Inner state for [Select]. This is an abstraction to allow it to support
/// multiple state "backends" from Ratatui, to enable usage with different
/// stateful widgets.
pub trait SelectData: Default {
    /// Index of the selected element
    fn selected(&self) -> Option<usize>;

    /// Select an element by index. Clear the selection if `None` is given
    fn select(&mut self, index: Option<usize>);
}

impl SelectData for ListState {
    fn selected(&self) -> Option<usize> {
        self.selected()
    }

    fn select(&mut self, index: Option<usize>) {
        self.select(index);
    }
}

impl SelectData for TableState {
    fn selected(&self) -> Option<usize> {
        self.selected()
    }

    fn select(&mut self, index: Option<usize>) {
        self.select(index);
    }
}

/// Selection state is just a number. This is useful for tabs and other
/// fixed-size elements that can never be empty.
impl SelectData for usize {
    fn selected(&self) -> Option<usize> {
        Some(*self)
    }

    fn select(&mut self, index: Option<usize>) {
        *self = index.expect("Cannot clear selection");
    }
}

/// Emitted event for [Select]
#[derive(Debug, EnumDiscriminants)]
#[strum_discriminants(name(SelectEventType), derive(Hash))]
#[cfg_attr(test, derive(PartialEq))]
pub enum SelectEvent {
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
        input::InputEvent,
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::{
            Generate,
            test_util::{PersistedComponent, TestComponent},
            util::persistence::DatabasePersistedStore,
        },
    };
    use persisted::{PersistedKey, PersistedStore};
    use proptest::{collection, sample, test_runner::TestRunner};
    use ratatui::text::Span;
    use rstest::rstest;
    use serde::Serialize;
    use slumber_core::collection::ProfileId;
    use slumber_util::assert_matches;
    use std::{collections::HashSet, ops::Deref};
    use terminput::KeyCode;

    /// Test preselection, where the initial selected state is modified at
    /// build time. There are various cases to test related to the list being
    /// empty, all items disabled, etc.
    #[rstest]
    // No explicit preselection
    #[case::default(&[0, 1], &[], None, Some(0))]
    #[case::empty(&[], &[], Some(0), None)]
    // Invalid selection, and the first is disabled - default to the second
    #[case::out_of_bounds(&[0, 1, 2], &[0], Some(3), Some(1))]
    // First item is disabled - default to the second
    #[case::default_disabled(&[0, 1], &[0], None, Some(1))]
    // Chosen item is disabled - select the first enabled
    #[case::selected_disabled(&[0, 1, 2, 3], &[0, 3], Some(3), Some(1))]
    // All are disabled - nothing to default to
    #[case::all_disabled_default(&[0, 1, 2], &[0, 1, 2], None, None)]
    // All are disabled - selection doesn't work at all
    #[case::all_disabled_default(&[0, 1, 2], &[0, 1, 2], Some(1), None)]
    fn test_preselect(
        #[case] items: &[usize],
        #[case] disabled: &[usize],
        #[case] preselect: Option<usize>,
        #[case] expected_selected: Option<usize>,
    ) {
        let select: Select<usize, ListState> =
            Select::builder(items.to_owned())
                .disabled_indexes(disabled.to_owned())
                .preselect_opt(preselect.as_ref())
                .build();
        assert_eq!(select.selected_index(), expected_selected);
    }

    /// Bottom-to-top inverts both the display direction and controls/selection
    #[rstest]
    fn test_bottom_to_top(
        harness: TestHarness,
        #[with(5, 3)] terminal: TestTerminal,
    ) {
        let styles = &TuiContext::get().styles.list;
        let items = vec!["one", "two", "three"];
        let select: Select<&str, ListState> = Select::builder(items)
            .direction(ListDirection::BottomToTop)
            .build();
        let mut component = TestComponent::new(&harness, &terminal, select);

        // Initial state - first item is at the bottom
        component.int().drain_draw().assert_empty();
        assert_eq!(component.selected(), Some(&"one"));
        terminal.assert_buffer_lines([
            "three".into(),
            "two  ".into(),
            "one  ".set_style(styles.highlight),
        ]);

        // Up -> next
        component.int().send_key(KeyCode::Up).assert_empty();
        assert_eq!(component.selected(), Some(&"two"));
        terminal.assert_buffer_lines([
            "three".into(),
            "two  ".set_style(styles.highlight),
            "one  ".into(),
        ]);

        // Down -> previous
        component.int().send_key(KeyCode::Down).assert_empty();
        assert_eq!(component.selected(), Some(&"one"));
        terminal.assert_buffer_lines([
            "three".into(),
            "two  ".into(),
            "one  ".set_style(styles.highlight),
        ]);
    }

    /// Apply a filter during build
    #[rstest]
    #[case::no_match("mango", None, &[], None)]
    #[case::no_preselect("pl", None, &["apple", "APPLE"], Some(0))]
    #[case::preselect_visible("pl", Some("APPLE"), &["apple", "APPLE"], Some(1))]
    #[case::preselect_hidden("pl", Some("banana"), &["apple", "APPLE"], Some(0))]
    fn test_filter(
        #[case] filter: &str,
        #[case] preselect: Option<&str>,
        #[case] expected_visible: &[&str],
        #[case] expected_selected: Option<usize>,
    ) {
        let items = ["apple", "APPLE", "banana"];
        let select: Select<&str, ListState> =
            Select::builder(items.into_iter().collect())
                .filter(filter)
                .preselect_opt(preselect.as_ref())
                .build();
        assert_eq!(
            select.items().copied().collect::<Vec<_>>(),
            expected_visible
        );
        assert_eq!(select.selected_index(), expected_selected);
    }

    /// Test going up and down in the list
    #[rstest]
    fn test_navigation(harness: TestHarness, terminal: TestTerminal) {
        let items = vec!["a", "b", "c"];
        let select: Select<&str, ListState> = Select::builder(items).build();
        let mut component = TestComponent::new(&harness, &terminal, select);
        component.int().drain_draw().assert_empty();
        assert_eq!(component.selected(), Some(&"a"));
        component.int().send_key(KeyCode::Down).assert_empty();
        assert_eq!(component.selected(), Some(&"b"));

        component.int().send_key(KeyCode::Up).assert_empty();
        assert_eq!(component.selected(), Some(&"a"));
    }

    /// Test deleting the selected item
    #[rstest]
    fn test_delete(harness: TestHarness, terminal: TestTerminal) {
        let items = vec!["a", "b", "c"];
        let select: Select<&str, ListState> = Select::builder(items)
            .subscribe([SelectEventType::Select])
            .build();
        let mut component = TestComponent::new(&harness, &terminal, select);

        // Start by selecting the second item, so we can assert that we select
        // the one after it when possible
        component
            .int()
            .drain_draw() // Handle initial state
            .send_key(KeyCode::Down)
            .assert_emitted([SelectEvent::Select(0), SelectEvent::Select(1)]);
        assert_eq!(component.selected(), Some(&"b"));

        // Delete `b`, `c` should get selected because it's below
        component.delete_selected();
        assert_eq!(component.selected(), Some(&"c"));
        component
            .int()
            .drain_draw()
            .assert_emitted([SelectEvent::Select(1)]);

        // Delete `c`; there's nothing left below so select above
        component.delete_selected();
        assert_eq!(component.selected(), Some(&"a"));
        component
            .int()
            .drain_draw()
            .assert_emitted([SelectEvent::Select(0)]);

        // Delete `a`, nothing left to select
        component.delete_selected();
        assert_eq!(component.selected(), None);
        component.int().drain_draw().assert_emitted([]);

        // Delete nothing; nothing should happen
        component.delete_selected();
        component.int().drain_draw().assert_emitted([]);
    }

    /// Test select emitted event
    #[rstest]
    fn test_select(harness: TestHarness, terminal: TestTerminal) {
        let items = vec!["a", "b", "c"];
        let select: Select<&str, ListState> = Select::builder(items)
            .subscribe([SelectEventType::Select])
            .build();
        let mut component = TestComponent::new(&harness, &terminal, select);

        // Initial selection
        assert_eq!(component.selected(), Some(&"a"));
        component
            .int()
            .drain_draw()
            .assert_emitted([SelectEvent::Select(0)]);

        // Select another one
        component
            .int()
            .send_key(KeyCode::Down)
            .assert_emitted([SelectEvent::Select(1)]);
    }

    /// Test submit emitted event
    #[rstest]
    fn test_submit(harness: TestHarness, terminal: TestTerminal) {
        let items = vec!["a", "b", "c"];
        let select: Select<&str, ListState> = Select::builder(items)
            .subscribe([SelectEventType::Submit])
            .build();
        let mut component = TestComponent::new(&harness, &terminal, select);
        component.int().drain_draw().assert_empty();

        component
            .int()
            .send_keys([KeyCode::Down, KeyCode::Enter])
            .assert_emitted([SelectEvent::Submit(1)]);
    }

    /// Test that the clicked item is selected
    #[rstest]
    fn test_click(harness: TestHarness, terminal: TestTerminal) {
        let items = vec!["a", "b", "c"];
        let select: Select<&str, ListState> = Select::builder(items).build();
        let mut component = TestComponent::new(&harness, &terminal, select);

        // Select item by click. Click is always propagated
        assert_matches!(
            component.int().click(0, 1).propagated(),
            &[Event::Input(InputEvent::Click { .. })]
        );
        assert_eq!(component.selected_index(), Some(1));

        // Click outside the select - does nothing
        assert_matches!(
            component.int().click(0, 3).propagated(),
            &[Event::Input(InputEvent::Click { .. })]
        );
        assert_eq!(component.selected_index(), Some(1));
    }

    /// Test that submit and toggle input events are propagated if we're not
    /// subscribed to them
    #[rstest]
    fn test_propagate(harness: TestHarness, terminal: TestTerminal) {
        let items = vec!["a", "b", "c"];
        let select: Select<&str, ListState> = Select::builder(items).build();
        let mut component = TestComponent::new(&harness, &terminal, select);

        assert_matches!(
            component.int().send_key(KeyCode::Enter).propagated(),
            &[Event::Input(InputEvent::Key {
                action: Some(Action::Submit),
                ..
            })]
        );
        assert_matches!(
            component.int().send_key(KeyCode::Char(' ')).propagated(),
            &[Event::Input(InputEvent::Key {
                action: Some(Action::Toggle),
                ..
            })]
        );
    }

    /// Test that disabled items can never be selected. When navigating up/down,
    /// they are skipped over. If a disabled item is specifically selected by
    /// index/value, nothing happens.
    #[rstest]
    fn test_disabled(harness: TestHarness, terminal: TestTerminal) {
        let items = vec!["a", "b", "c", "d", "e"];
        let select = Select::builder(items)
            .disabled_indexes([1, 3])
            // For simplicity, we're only looking for select events. Seems like
            // a safe assumption that if it doesn't emit Select, it won't emit
            // anything else.
            .subscribe([SelectEventType::Select])
            .build();
        let mut component: TestComponent<'_, Select<&'static str>> =
            TestComponent::new(&harness, &terminal, select);

        assert_eq!(component.selected(), Some(&"a"));
        component
            .int()
            .drain_draw()
            .assert_emitted([SelectEvent::Select(0)]);

        // Move down - skips over the disabled item
        component
            .int()
            .send_key(KeyCode::Down)
            .assert_emitted([SelectEvent::Select(2)]);
        assert_eq!(component.selected(), Some(&"c"));

        // Move down again - skips over the disabled item
        component
            .int()
            .send_key(KeyCode::Down)
            .assert_emitted([SelectEvent::Select(4)]);
        assert_eq!(component.selected(), Some(&"e"));

        // Move up - skips over the disabled item
        component
            .int()
            .send_key(KeyCode::Up)
            .assert_emitted([SelectEvent::Select(2)]);
        assert_eq!(component.selected(), Some(&"c"));

        // Select by value/index should do nothing if it's disabled
        let select = &mut component;
        // Make sure that *nothing* happens, and that it's not skipping to the
        // next/previous enabled value
        select.select(&"b");
        assert_eq!(select.selected(), Some(&"c"));
        select.select(&"d");
        assert_eq!(select.selected(), Some(&"c"));
        select.select_index(1);
        assert_eq!(select.selected(), Some(&"c"));
    }

    /// Test some properties related to disabled items:
    /// - Disabled items cannot be selected
    /// - Disabled items never emit events
    /// - If all items are disabled, nothing can be selected
    /// - If only one item is enabled, selection events are never emitted after
    ///   init (because the selection never changes)
    ///
    /// We'll disable some items and perform some actions, and after each
    /// action, all those properties should be true.
    #[rstest]
    fn test_disabled_prop(harness: TestHarness, terminal: TestTerminal) {
        let items = vec!["a", "b", "c", "d"];

        // I think the proptest! macro is annoying so I prefer manual
        let test = |(disabled_indexes, inputs): (
            HashSet<usize>,
            Vec<KeyCode>,
        )| {
            let num_enabled = items.len() - disabled_indexes.len();
            // For simplicity, we're only looking for select events.
            // Seems like a safe assumption that if it doesn't emit
            // Select, it won't emit anything else.
            let select: Select<&str, ListState> =
                Select::builder(items.clone())
                    .disabled_indexes(disabled_indexes)
                    .subscribe([SelectEventType::Select])
                    .build();
            let mut component = TestComponent::new(&harness, &terminal, select);

            // Drain inital events and check state
            let first_enabled = component.first_enabled_index();
            component
                .int()
                .drain_draw()
                // Event should be emitted iff there is 1+ enabled items
                .assert_emitted(first_enabled.map(SelectEvent::Select));
            assert_eq!(component.selected_index(), first_enabled);

            for input in inputs {
                let interact = component.int().send_key(input);
                let select = interact.component_data();
                let selected_index = select.selected_index();
                match num_enabled {
                    // All items are disabled
                    0 => {
                        assert_eq!(
                            selected_index, None,
                            "Selection should be none when all items are disabled"
                        );
                        // No items should emit events
                        interact.assert_emitted([]);
                    }
                    // Exactly one item is enabled - the selection can never
                    // change
                    1 => {
                        assert!(
                            selected_index
                                .map(|index| &select.items[index])
                                .is_some_and(SelectItem::enabled),
                            "Selection should not be disabled"
                        );

                        // Should not emit an event because the selection never
                        // changes
                        interact.assert_emitted([]);
                    }
                    // Multiple items are enabled
                    2.. => {
                        assert!(
                            selected_index
                                .map(|index| &select.items[index])
                                .is_some_and(SelectItem::enabled),
                            "Selection should not be disabled"
                        );

                        // Event should've been emitted for the selected item,
                        // and *not* for any disabled items that may have been
                        // skipped over
                        let event =
                            SelectEvent::Select(selected_index.unwrap());
                        interact.assert_emitted([event]);
                    }
                }
            }

            Ok(())
        };

        let mut runner = TestRunner::default();
        let range = || 0..items.len();
        runner
            .run(
                &(
                    collection::hash_set(range(), range()),
                    collection::vec(
                        sample::select(&[KeyCode::Up, KeyCode::Down]),
                        0..8usize,
                    ),
                ),
                test,
            )
            .unwrap();
    }

    /// Test persisting selected item
    #[rstest]
    // First item gets selected by preselection, second by persistence
    #[case::persisted("persisted", "persisted", &[0, 1])]
    // If the persisted item is disabled, we'll select the first item instead
    #[case::disabled("disabled", "default", &[0])]
    fn test_persistence(
        harness: TestHarness,
        terminal: TestTerminal,
        #[case] persisted_id: &str,
        #[case] expected_selected: &str,
        // Which items emit Select events?
        #[case] expected_event_indexes: &[usize],
    ) {
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

        impl From<&str> for ProfileItem {
            fn from(id: &str) -> Self {
                Self(id.into())
            }
        }

        impl Generate for &ProfileItem {
            type Output<'this>
                = Span<'this>
            where
                Self: 'this;

            fn generate<'this>(self) -> Self::Output<'this>
            where
                Self: 'this,
            {
                self.0.deref().into()
            }
        }

        DatabasePersistedStore::store_persisted(
            &Key,
            &Some(persisted_id.into()),
        );

        // Second profile should be pre-selected because of persistence
        let select: Select<ProfileItem> = Select::builder(vec![
            "default".into(),
            "persisted".into(),
            "disabled".into(),
        ])
        .disabled_indexes([2])
        .subscribe([SelectEventType::Select])
        .build();
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            PersistedComponent::new(Key, select),
        );
        assert_eq!(
            component.selected().map(|item| item.0.deref()),
            Some(expected_selected)
        );
        component.int().drain_draw().assert_emitted(
            expected_event_indexes
                .iter()
                .copied()
                .map(SelectEvent::Select),
        );
    }
}
