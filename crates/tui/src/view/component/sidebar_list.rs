use crate::view::{
    Generate, UpdateContext,
    common::{
        Pane,
        select::{Select, SelectEventKind, SelectListProps},
        text_box::{TextBox, TextBoxEvent, TextBoxProps},
    },
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
    },
    context::ViewContext,
    event::{Emitter, Event, EventMatch, ToEmitter},
    persistent::{PersistentKey, PersistentStore},
};
use ratatui::{
    layout::{Constraint, Layout},
    text::{Span, Text},
};
use slumber_config::Action;
use std::borrow::Cow;

/// A list that can be displayed in the sidebar or collapsed into a header
///
/// This is generic and powers all the collapsible headers, including the recipe
/// and profile lists. This does *not* retain the collapsed state itself; it's
/// passed as a prop at draw time.
#[derive(Debug)]
pub struct SidebarList<State: SidebarListState> {
    id: ComponentId,
    emitter: Emitter<SidebarListEvent>,
    title: String,
    select: Select<ItemWrapper<State::Item>>,
    /// Implementation-specific list state
    state: State,

    /// Text box for filtering down items in the list
    filter: TextBox,
    /// Is the user typing in the filter box? User has to explicitly grab focus
    /// on the box to start typing
    filter_focused: bool,
}

impl<State: SidebarListState> SidebarList<State> {
    /// Initialize a new sidebar list
    ///
    /// [SidebarListState::items] will be used to populate the list.
    pub fn new(state: State) -> Self {
        let title = ViewContext::add_binding_hint(State::TITLE, State::ACTION);
        let select = Self::build_select(&state, "");
        let filter = TextBox::default()
            .placeholder(format!(
                "{binding} to filter",
                binding = ViewContext::binding_display(Action::Search)
            ))
            .subscribe([
                TextBoxEvent::Cancel,
                TextBoxEvent::Change,
                TextBoxEvent::Submit,
            ]);

        Self {
            id: ComponentId::default(),
            emitter: Emitter::default(),
            title,
            select,
            state,
            filter,
            filter_focused: false,
        }
    }

    /// Get the selected item, or `None` if the list is empty
    pub fn selected(&self) -> Option<&State::Item> {
        self.select.selected().map(|item| &item.0)
    }

    /// Get the ID of the selected item, or `None` if the list is empty
    pub fn selected_id(&self) -> Option<&<State::Item as SidebarListItem>::Id> {
        self.selected().map(State::Item::id)
    }

    /// Get the inner state value
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Get a mutable reference to the inner state value
    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    /// Rebuild the select. Call this whenever the list of items may change
    pub fn rebuild_select(&mut self) {
        self.select = Self::build_select(&self.state, self.filter.text());
    }

    /// Build/rebuild a select based on the item list
    fn build_select(
        state: &State,
        filter: &str,
    ) -> Select<ItemWrapper<State::Item>> {
        let filter = filter.trim().to_lowercase();
        let items = state
            .items()
            .into_iter()
            .filter(|item| {
                // Apply text filtering
                if filter.is_empty() {
                    return true;
                }

                // If any search term matches caselessly, it's a match
                let terms = item.filter_terms();
                terms
                    .iter()
                    .any(|term| term.to_lowercase().contains(&filter))
            })
            .map(ItemWrapper)
            .collect();

        Select::builder(items)
            .subscribe([SelectEventKind::Select])
            .persisted(&state.persistent_key())
            .build()
    }
}

impl<State: Default + SidebarListState> Default for SidebarList<State> {
    fn default() -> Self {
        Self::new(State::default())
    }
}

impl<State: SidebarListState> Component for SidebarList<State> {
    fn id(&self) -> super::ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .click(|_, _| self.emitter.emit(SidebarListEvent::Open))
            .action(|action, propagate| match action {
                Action::Cancel => self.emitter.emit(SidebarListEvent::Close),
                Action::Search => self.filter_focused = true,

                // We can't check for our own action to open here because we
                // won't have focus while closed
                _ => propagate.set(),
            })
            // Emitted events from select
            .emitted(self.select.to_emitter(), |event| match event.kind {
                SelectEventKind::Select => {
                    self.emitter.emit(SidebarListEvent::Select);
                }
                SelectEventKind::Submit | SelectEventKind::Toggle => {}
            })
            // Emitted events from filter
            .emitted(self.filter.to_emitter(), |event| match event {
                TextBoxEvent::Change => self.rebuild_select(),
                TextBoxEvent::Cancel | TextBoxEvent::Submit => {
                    self.filter_focused = false;
                }
            })
    }

    fn persist(&self, store: &mut PersistentStore) {
        // Persist selected item
        store.set_opt(
            &self.state.persistent_key(),
            self.select.selected().map(|item| item.0.id()),
        );
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            // Filter gets highest priority because when it's focused, it
            // should eat all events
            self.filter.to_child_mut(),
            self.select.to_child_mut(),
        ]
    }
}

impl<State: SidebarListState> Draw<SidebarListProps> for SidebarList<State> {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: SidebarListProps,
        metadata: DrawMetadata,
    ) {
        // Both formats use a pane outline
        let block = Pane {
            title: &self.title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        match props.format {
            Format::Header => {
                let value: Text = self
                    .select
                    .selected()
                    .map(|item| item.0.display_header().into())
                    .unwrap_or_else(|| "None".into());
                canvas.render_widget(value, area);
            }
            Format::List => {
                // Expanded sidebar
                let [filter_area, list_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(area);
                canvas.draw(
                    &self.filter,
                    TextBoxProps::default(),
                    filter_area,
                    self.filter_focused,
                );
                canvas.draw(
                    &self.select,
                    SelectListProps::pane(),
                    list_area,
                    true,
                );
            }
        }
    }
}

impl<State> ToEmitter<SidebarListEvent> for SidebarList<State>
where
    State: SidebarListState,
{
    fn to_emitter(&self) -> Emitter<SidebarListEvent> {
        self.emitter
    }
}

/// Draw props for [SidebarList]
#[derive(Default)]
pub struct SidebarListProps {
    format: Format,
}

impl SidebarListProps {
    /// Draw the sidebar in collapsed/header mode, where just the selected
    /// value is visit
    pub fn header() -> Self {
        Self {
            format: Format::Header,
        }
    }

    /// Draw the sidebar in list mode, where the entire list is visible and
    /// interactive
    pub fn list() -> Self {
        Self {
            format: Format::List,
        }
    }
}

/// Visual format of the list
#[derive(Debug, Default)]
enum Format {
    /// List is collapsed and just visible as a header. Only the selected value
    /// is visible
    Header,
    /// List is open in the sidebar and the entire list is visible
    #[default]
    List,
}

/// Emitted event from [SidebarList]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SidebarListEvent {
    /// Sidebar should be expanded
    Open,
    /// List selection changed
    Select,
    /// Sidebar should be collapsed
    Close,
}

/// Generic state for [SidebarList]. This defines how the list is populated
pub trait SidebarListState {
    /// Display title of the list; will be shown in the pane header
    const TITLE: &str;
    /// Action that opens this list; a hint will be shown in the pane header
    const ACTION: Action;

    /// Type of item in the list
    type Item: 'static + SidebarListItem;
    /// Type of the key under which the selected item will be persisted
    type PersistentKey: PersistentKey<
        Value = <Self::Item as SidebarListItem>::Id,
    >;

    /// The key under which the selected item will be persisted
    fn persistent_key(&self) -> Self::PersistentKey;

    /// Get the list of items that should be shown
    ///
    /// This will be called each time the select list is rebuilt
    fn items(&self) -> Vec<Self::Item>;
}

/// Abstraction for an item in a [SidebarListState] list. This provides some
/// common functionality needed on a per-item basis for all lists.
pub trait SidebarListItem {
    /// Unique identifier type for each item in the list, e.g. recipe ID. Must
    /// implement `PartialEq` to enable persistence restoration.
    type Id: PartialEq;

    /// Get this item's unique ID
    fn id(&self) -> &Self::Id;

    /// Get the string to be displayed in the header when this item is selected
    fn display_header(&self) -> Cow<'_, str>;

    /// Get the string to be displayed in the list for this item. This is
    /// typically the same as the header string, but may vary. E.g. the recipe
    /// list uses indentation and arrows to indicating its tree structure.
    fn display_list(&self) -> Cow<'_, str> {
        self.display_header()
    }

    /// Get the terms to search against when apply a test filter to the list.
    /// If any term in the returned list contains the filter query, the item
    /// will be included. By default this just is the header display text.
    /// Recipe folders use additional search terms to match all their children.
    fn filter_terms(&self) -> Vec<Cow<'_, str>> {
        vec![self.display_header()]
    }
}

/// Wrapper for each item in the list that provides some trait implementations.
/// These impls are needed to integrate with [Select]'s trait bounds.
#[derive(Debug)]
struct ItemWrapper<T>(T);

impl<T: SidebarListItem> PartialEq<T::Id> for ItemWrapper<T> {
    fn eq(&self, id: &T::Id) -> bool {
        self.0.id() == id
    }
}

impl<T: SidebarListItem> Generate for &ItemWrapper<T> {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        self.0.display_list().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
    };
    use itertools::Itertools;
    use rstest::rstest;
    use serde::Serialize;
    use terminput::KeyCode;

    /// Test the filter box
    #[rstest]
    #[case::single_match("two", &["item2"])]
    #[case::caseless("ITEM T", &["item2", "item3"])]
    #[case::hidden_term("hidden", &["item1", "item2", "item3"])]
    // Whitespace gets trimmed away
    #[case::whitespace("   two   ", &["item2"])]
    fn test_filter(
        harness: TestHarness,
        terminal: TestTerminal,
        #[case] filter: &str,
        #[case] expected: &[&str],
    ) {
        use std::iter;

        let mut component: TestComponent<SidebarList<TestState>> =
            TestComponent::builder(&harness, &terminal, SidebarList::default())
                .with_default_props()
                // Event is emitted for the initial selection
                .with_assert_events(|assert| {
                    assert.emitted([SidebarListEvent::Select]);
                })
                .build();

        // Enter filter mode
        component
            .int()
            .send_key(KeyCode::Char('/'))
            .assert()
            .empty();
        assert!(component.filter_focused);

        // Type the input
        component
            .int()
            .send_text(filter)
            // A select event is emitted each time the select is rebuilt, which
            // is after each entered character
            .assert()
            .emitted(iter::repeat_n(SidebarListEvent::Select, filter.len()));
        let select = &component.select;
        assert_eq!(
            select
                .items()
                .map(|item| item.0.id.as_str())
                .collect_vec()
                .as_slice(),
            expected
        );

        // Exit filter
        component.int().send_key(KeyCode::Esc).assert().empty();
        assert!(!component.filter_focused);
    }

    #[derive(Debug, Default)]
    struct TestState {
        id: ComponentId,
    }

    impl Component for TestState {
        fn id(&self) -> ComponentId {
            self.id
        }
    }

    impl SidebarListState for TestState {
        const TITLE: &str = "Test";
        const ACTION: Action = Action::SelectRecipeList;

        type Item = TestItem;
        type PersistentKey = TestKey;

        fn persistent_key(&self) -> Self::PersistentKey {
            TestKey
        }

        fn items(&self) -> Vec<Self::Item> {
            vec![
                TestItem::new("item1", "Item One"),
                TestItem::new("item2", "Item Two"),
                TestItem::new("item3", "Item Three"),
            ]
        }
    }

    #[derive(Debug)]
    struct TestItem {
        id: String,
        name: String,
    }

    impl TestItem {
        fn new(id: &str, name: &str) -> Self {
            Self {
                id: id.into(),
                name: name.into(),
            }
        }
    }

    impl SidebarListItem for TestItem {
        type Id = String;

        fn id(&self) -> &Self::Id {
            &self.id
        }

        fn display_header(&self) -> Cow<'_, str> {
            self.name.as_str().into()
        }

        fn filter_terms(&self) -> Vec<Cow<'_, str>> {
            vec![self.display_header(), "hidden term".into()]
        }
    }

    #[derive(Debug, Serialize)]
    struct TestKey;

    impl PersistentKey for TestKey {
        type Value = String;
    }
}
