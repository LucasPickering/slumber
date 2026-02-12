//! Test utilities specific to the TUI *view*

use crate::{
    http::RequestStore,
    message::{self, Message, MessageReceiver},
    view::{
        ComponentMap, UpdateContext,
        common::actions::{ActionMenu, MenuItem},
        component::{
            Canvas, Child, Component, ComponentExt, ComponentId, Draw,
            DrawMetadata, ToChild,
        },
        context::ViewContext,
        event::{BroadcastEvent, Event, EventMatch, LocalEvent, ToEmitter},
        persistent::PersistentStore,
    },
};
use itertools::Itertools;
use ratatui::{Frame, Terminal, backend::TestBackend, layout::Rect};
use rstest::fixture;
use slumber_config::{Action, Config};
use slumber_core::{collection::Collection, database::CollectionDatabase};
use slumber_util::Factory;
use std::{
    cell::RefCell,
    fmt::Debug,
    iter, mem,
    ops::{Deref, DerefMut},
    rc::Rc,
    sync::Arc,
};
use terminput::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use tracing::trace_span;

/// Get a test harness, with a clean terminal etc. See [TestHarness].
#[fixture]
pub fn harness(terminal_width: u16, terminal_height: u16) -> TestHarness {
    TestHarness::new(Collection::factory(()), terminal_width, terminal_height)
}

/// Terminal width in chars, for injection to [harness] fixture
#[fixture]
fn terminal_width() -> u16 {
    50
}

/// Terminal height in chars, for injection to [harness] fixture
#[fixture]
fn terminal_height() -> u16 {
    20
}

/// A container for all singleton types needed for tests. Most TUI tests will
/// need one of these. This should be your interface for modifying any global
/// state.
pub struct TestHarness {
    // These are public because we don't care about external mutation
    pub collection: Arc<Collection>,
    pub database: CollectionDatabase,
    /// `RefCell` needed so multiple components can hang onto this at once.
    /// Otherwise we would have to pass it to every single draw and update fn.
    request_store: Rc<RefCell<RequestStore>>,
    messages_rx: MessageReceiver,
    terminal: Terminal<TestBackend>,
}

impl TestHarness {
    /// Create a new test harness and initialize state
    pub fn new(
        collection: Collection,
        terminal_width: u16,
        terminal_height: u16,
    ) -> Self {
        let (messages_tx, messages_rx) = message::queue();
        let database = CollectionDatabase::factory(());
        let request_store =
            Rc::new(RefCell::new(RequestStore::new(database.clone())));
        let collection = Arc::new(collection);

        let backend = TestBackend::new(terminal_width, terminal_height);
        let terminal = Terminal::new(backend).unwrap();

        ViewContext::init(
            Config::default().into(),
            Arc::clone(&collection),
            database.clone(),
            messages_tx,
        );

        TestHarness {
            collection,
            database,
            request_store,
            messages_rx,
            terminal,
        }
    }

    /// Get a mutable reference to the request store
    pub fn request_store_mut(&self) -> impl DerefMut<Target = RequestStore> {
        self.request_store.borrow_mut()
    }

    /// Get a [PersistentStore] pointing at the test database
    pub fn persistent_store(&self) -> PersistentStore {
        PersistentStore::new(self.database.clone())
    }

    /// Get a mutable reference to the message queue receiver, which can be used
    /// to modify and assert on the message queue
    pub fn messages_rx(&mut self) -> &mut MessageReceiver {
        &mut self.messages_rx
    }

    /// Draw to the terminal
    pub fn draw(&mut self, f: impl FnOnce(&mut Frame)) {
        self.terminal.draw(f).unwrap();
    }

    /// Get the terminal backend, for assertions
    pub fn terminal_backend(&self) -> &TestBackend {
        self.terminal.backend()
    }

    /// Get the area of the terminal buffer
    pub fn terminal_area(&self) -> Rect {
        self.terminal.backend().buffer().area
    }
}

/// A wrapper around a component that makes it easy to test. This provides lots
/// of methods for sending events to the component. The goal is to make
/// realistic testing the easiest option, so component tests aren't contrived or
/// verbose.
///
/// This takes a a reference to the terminal so it can draw without having
/// to plumb the terminal around to every draw call.
///
/// Use the [Deref] and [DerefMut] implementations to access the component under
/// test.
#[derive(Debug)]
pub struct TestComponent<T> {
    database: CollectionDatabase,
    request_store: Rc<RefCell<RequestStore>>,
    /// Output of the most recent draw phase
    component_map: ComponentMap,
    /// The area the component will be drawn to. This defaults to the whole
    /// terminal but can be modified to test things like resizes, using
    /// [Self::set_area]
    area: Rect,
    component: TestWrapper<T>,
    /// Should the component be given focus on the next draw? Defaults to
    /// `true`
    has_focus: bool,
    /// Messages propagated by the component constructor. These will be
    /// forwarded to the first [int] call so they can be asserted on.
    initial_propagated: Vec<Message>,
}

impl<T> TestComponent<T>
where
    T: Component + Debug,
{
    /// Start building a new component
    pub fn builder<Props>(
        harness: &mut TestHarness,
        data: T,
    ) -> TestComponentBuilder<'_, T, Props>
    where
        T: Draw<Props>,
    {
        TestComponentBuilder {
            area: harness.terminal_area(),
            harness,
            component: TestWrapper::new(data),
            props: None,
        }
    }

    /// Shortcut for building and drawing a component with default props and
    /// the full terminal area
    pub fn new<Props>(harness: &mut TestHarness, data: T) -> Self
    where
        T: Draw<Props>,
        Props: Default,
    {
        Self::builder(harness, data).with_default_props().build()
    }

    /// Modify the area the component will be drawn to
    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    /// Disable focus for the next draw
    pub fn unfocus(&mut self) {
        self.has_focus = false;
    }

    /// Get a helper to chain interactions and assertions on this component.
    /// Each draw will use `Props::default()` for the props value.
    pub fn int<'a, Props>(
        &'a mut self,
        harness: &'a mut TestHarness,
    ) -> Interact<'a, T, Props>
    where
        T: Draw<Props>,
        Props: 'a + Default,
    {
        self.int_props(harness, Props::default)
    }

    /// Get a helper to chain interactions and assertions on this component.
    /// Each draw will call the given props factory function to generate the
    /// next props value.
    pub fn int_props<'a, Props>(
        &'a mut self,
        harness: &'a mut TestHarness,
        props_factory: impl 'a + Fn() -> Props,
    ) -> Interact<'a, T, Props>
    where
        T: Draw<Props>,
    {
        // If any messages were propagated during startup, forward them to
        // the first int() call
        let propagated = mem::take(&mut self.initial_propagated);
        Interact {
            component: self,
            terminal: &mut harness.terminal,
            messages_rx: &mut harness.messages_rx,
            props_factory: Box::new(props_factory),
            propagated,
        }
    }

    /// Get the owned inner component
    pub fn into_inner(self) -> T {
        self.component.inner
    }

    /// Draw this component onto the terminal, using the entire terminal frame
    /// as the draw area. If props are given, use them for the draw. If not,
    /// use the same props from the last draw.
    fn draw<Props>(&mut self, frame: &mut Frame, props: Props)
    where
        T: Draw<Props>,
    {
        // Each draw gets a new canvas, as the Lord intended
        self.component_map = Canvas::draw_all_area(
            frame.buffer_mut(),
            &self.component,
            props,
            self.area,
            self.has_focus,
        );
    }

    /// Drain events from the event queue, and handle them one-by-one. Return
    /// the messages that were propagated (i.e. not consumed by the component or
    /// its children), in the order they were queued/handled.
    fn drain_events(
        &mut self,
        messages_rx: &mut MessageReceiver,
    ) -> Vec<Message> {
        let mut persistent_store = PersistentStore::new(self.database.clone());
        let mut propagated = Vec::new();
        let mut context = UpdateContext {
            component_map: &self.component_map,
            request_store: &mut self.request_store.borrow_mut(),
        };
        while let Some(message) = messages_rx.try_pop() {
            if let Message::Event(event) = message {
                trace_span!("Handling event", ?event).in_scope(|| {
                    let event = self.component.update_all(&mut context, event);
                    if let Some(event) = event {
                        propagated.push(Message::Event(event));
                    }
                });
            } else {
                // All other message types CANNOT be handled by components, so
                // they must be propagated
                propagated.push(message);
            }
        }

        // Persist values in the store after the update. This mimics what the
        // event loop does
        self.component.persist_all(&mut persistent_store);

        propagated
    }
}

// Manual impl needed to prevent bound `TestWrapper<T>: Deref>`
impl<T> Deref for TestComponent<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.component.inner
    }
}

impl<T> DerefMut for TestComponent<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.component.inner
    }
}

/// Helper for customizing a [TestComponent] before its initial draw
pub struct TestComponentBuilder<'harness, T, Props> {
    harness: &'harness mut TestHarness,
    area: Rect,
    component: TestWrapper<T>,
    props: Option<Props>,
}

impl<T, Props> TestComponentBuilder<'_, T, Props>
where
    T: Component + Draw<Props> + Debug,
{
    /// Set initial props for this component
    pub fn with_props(mut self, props: Props) -> Self {
        self.props = Some(props);
        self
    }

    /// Use `Props::default()` for props
    pub fn with_default_props(mut self) -> Self
    where
        Props: Default,
    {
        self.props = Some(Props::default());
        self
    }

    /// Set area to draw the component to (defaults to the full terminal)
    pub fn with_area(mut self, area: Rect) -> Self {
        self.area = area;
        self
    }

    /// Build the component, process its initialization events, then do an
    /// initial draw
    ///
    /// Draining initial events and drawing are considered universal
    /// functionality that all components will receive as part of their
    /// normal operation.
    pub fn build(self) -> TestComponent<T> {
        let mut component = TestComponent {
            database: self.harness.database.clone(),
            request_store: self.harness.request_store.clone(),
            component_map: ComponentMap::default(),
            area: self.area,
            component: self.component,
            has_focus: true,
            initial_propagated: vec![],
        };

        // Drain any messages that may have been queued during component init,
        // then draw with the latest state
        let props = self.props.expect("Props not set for test component");
        // Propagated events just get tossed
        component.initial_propagated =
            component.drain_events(&mut self.harness.messages_rx);
        self.harness.draw(|frame| component.draw(frame, props));

        component
    }
}

/// Utility class for interacting with a test component. This allows chaining
/// various interactions. All chains should be terminated with an assertion
/// on the events propagated by the interactions. Each interaction will be
/// succeeded by a single draw, to update the view as needed.
#[must_use = "Complete interaction with assert()"]
#[derive(derive_more::Debug)]
pub struct Interact<'a, Component, Props> {
    component: &'a mut TestComponent<Component>,
    terminal: &'a mut Terminal<TestBackend>,
    /// Message queue receiver, from [TestHarness]
    messages_rx: &'a mut MessageReceiver,
    /// A repeatable function that generates a props object for each draw. In
    /// most cases this will just be `Props::default` or a function that
    /// repeatedly returns the same static value. In some cases though, the
    /// value can't be held across draws and must be recreated each time.
    props_factory: Box<dyn 'a + Fn() -> Props>,
    /// Messages queued during this interaction that were not handled by the
    /// component
    propagated: Vec<Message>,
}

impl<'a, Comp, Props> Interact<'a, Comp, Props>
where
    Comp: Component + Draw<Props> + Debug,
{
    /// Drain all events in the queue, then draw the component to the terminal.
    ///
    /// This similar to [update_draw](Self::update_draw), but doesn't require
    /// you to queue a new event first. This is helpful in the rare occasions
    /// where the UI needs to respond to some asynchronous event, such as a
    /// callback that would normally be called by the main loop.
    pub fn drain_draw(mut self) -> Self {
        let propagated = self.component.drain_events(self.messages_rx);
        self.terminal
            .draw(|frame| {
                let props = (self.props_factory)();
                self.component.draw(frame, props);
            })
            .unwrap();
        self.propagated.extend(propagated);
        self
    }

    /// Put an event on the event queue, handle **all** events in the queue,
    /// then redraw to the screen (using whatever props were used for the last
    /// draw). This is the generic "do something in a test" method. Generally
    /// any user interaction that you want to simulate in a test should use this
    /// method (or one of its callers, like [Self::send_key]). This most closely
    /// simulates behavior in the wild, because the TUI will typically re-draw
    /// after every user input (unless the user hits two keys *really* quickly).
    ///
    /// Return whatever events were propagated, so you can test for events that
    /// you expect to be generated, but consumed by a parent component that
    /// doesn't exist in the test case. This return value should be used, even
    /// if you're just checking that it's empty. This is important because
    /// propagated events *may* be intentional, but could also indicate a bug
    /// where you component isn't handling events it should (or vice versa).
    pub fn update_draw(self, event: Event) -> Self {
        // This is a safety check, so we don't end up handling events we didn't
        // expect to
        assert!(
            self.messages_rx.is_empty(),
            "Message queue is not empty. To prevent unintended side effects, \
                the queue must be empty before an update. Maybe you want to call
                drain_draw() before the first interaction?\n{:?}",
            self.messages_rx
        );
        ViewContext::push_message(event);
        self.drain_draw()
    }

    /// Run a function with access to the component. Useful for debugging and
    /// assertions in the middle of an interaction chain.
    pub fn inspect(self, f: impl FnOnce(&Comp)) -> Self {
        f(self.component_data());
        self
    }

    /// Push a terminal input event onto the event queue, then drain events and
    /// draw. This will include the bound action for the event, based on the key
    /// code or mouse button. See [Self::update_draw] about return value.
    pub fn send_input(self, terminal_event: terminput::Event) -> Self {
        let input_event = ViewContext::with_input(|input| {
            input.convert_event(terminal_event)
        })
        .expect("Event does not map to an input event");
        self.update_draw(Event::Input(input_event))
    }

    /// Simulate a left click at the given location, then drain events and draw.
    /// See [Self::update_draw] about return value.
    pub fn click(self, x: u16, y: u16) -> Self {
        let term_event = terminput::Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        self.send_input(term_event)
    }

    /// Simulate a key press on this component. This will generate the
    /// corresponding event (including bound action, if any), send it to the
    /// component, then drain events and draw.  See
    /// [Self::update_draw] about return value.
    pub fn send_key(self, code: KeyCode) -> Self {
        self.send_key_modifiers(KeyModifiers::NONE, code)
    }

    /// [Self::send_key], but with modifier keys applied
    pub fn send_key_modifiers(
        self,
        modifiers: KeyModifiers,
        code: KeyCode,
    ) -> Self {
        let term_event = terminput::Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        });
        self.send_input(term_event)
    }

    /// Send multiple key events in sequence
    pub fn send_keys(
        mut self,
        codes: impl IntoIterator<Item = KeyCode>,
    ) -> Self {
        for code in codes {
            self = self.send_key(code);
        }
        self
    }

    /// Send some text as a series of key events, handling each event and
    /// re-drawing after each character. This may seem wasteful, but it most
    /// closely simulates what happens in the real world. Return propagated
    /// events from *all* updates, e.g. the concatenation of propagated events
    /// from each individual call to [Self::update_draw].
    pub fn send_text(self, text: &str) -> Self {
        self.send_keys(text.chars().map(KeyCode::Char))
    }

    /// Open the action menu and execute the action matching the given path.
    /// Each step in the path corresponds to a single layer in the action menu.
    /// For actions in the top level of the menu, the path will be just a single
    /// element.
    ///
    /// Panic if no matching action exists
    pub fn action(mut self, path: &[&str]) -> Self {
        /// Inner helper to select+Enter an item within a single menu layer
        fn find_item<'a>(
            items: &'a [MenuItem],
            name: &str,
        ) -> (usize, &'a MenuItem) {
            // Find the index of the action in the list so we know how far to
            // scroll
            let (index, item) = items
                .iter()
                .enumerate()
                .find(|(_, action)| action.to_string() == name)
                .unwrap_or_else(|| {
                    panic!(
                        "No action `{name}`. Available actions: {}",
                        items.iter().format(", "),
                    )
                });
            // Disabled actions can't be selected or triggered
            assert!(
                item.enabled(),
                "Action `{item}` cannot be selected because it is disabled"
            );

            // Disabled actions are auto-skipped, so don't include them in the
            // number of steps to make
            let steps = index
                - items[0..index]
                    .iter()
                    .filter(|action| !action.enabled())
                    .count();

            (steps, item)
        }

        let items = {
            let context = UpdateContext {
                component_map: &self.component.component_map,
                request_store: &mut self.component.request_store.borrow_mut(),
            };
            self.component.component.collect_actions(&context)
        };
        // Open the menu
        self = self.send_key(KeyCode::Char('x'));

        // For each layer in the path, find+select the matching item
        let mut next = &items;
        for name in path {
            let (steps, item) = find_item(next, name);
            // If this is a group, drop down a layer
            if let MenuItem::Group { children, .. } = item {
                next = children;
            }

            self = self
                // Move down to select the matching action
                .send_keys(iter::repeat_n(KeyCode::Down, steps))
                // Open group or execute action
                .send_key(KeyCode::Enter);
        }

        self
    }

    /// Pop a [Message::Spawn] off the **propagated** list and run it
    ///
    /// Panic if the propagated is empty or the most recent propagated message
    /// isn't `Spawn`. This uses the propagated list instead of the message
    /// queue because this is typically called right after another interaction
    /// that would drain the message queue. Any `Spawn` messages in the queue
    /// would have been propagated. This is a bit shitty but it works (for now).
    pub async fn run_task(mut self) -> Self {
        let future = match self.propagated.pop() {
            Some(Message::Spawn(future)) => future,
            other => panic!(
                "run_task expected Spawn message, but received: {other:?}"
            ),
        };
        future.await;
        // Handle any messages from the task
        self.drain_draw()
    }

    /// Get the underlying component value
    pub fn component_data(&self) -> &Comp {
        &self.component.component.inner
    }

    /// Get the messages propagated during this interaction
    ///
    /// This will include any view events not handled by the component, as well
    /// as all other message types. The returned value is an array of fixed size
    /// `N`. The output if this is generally passed directly to
    /// `assert_matches!()`, where you now exactly how many messages will be
    /// returned. Returning an array makes it possible to match on owned values.
    #[must_use]
    #[track_caller]
    pub fn into_propagated<const N: usize>(self) -> [Message; N] {
        #[expect(clippy::expect_fun_call)] // Expect gives better stack trace
        self.propagated
            .try_into()
            .expect(&format!("Expected {N} messages"))
    }

    /// Get an [AssertMessages] to assert properties about the list of messages
    /// propagated by this interaction
    pub fn assert(self) -> AssertMessages<'a, Comp> {
        AssertMessages {
            component: self.component,
            propagated: self.propagated,
        }
    }
}

/// Assert on the list of propagated events
#[must_use = "Propagated events must be checked"]
pub struct AssertMessages<'a, Comp> {
    component: &'a mut TestComponent<Comp>,
    propagated: Vec<Message>,
}

impl<Comp> AssertMessages<'_, Comp> {
    /// Get the underlying component value
    pub fn component_data(&self) -> &Comp {
        &*self.component
    }

    /// Assert that no messages were propagated, i.e. the component handled all
    /// given and generated messages.
    #[track_caller]
    pub fn empty(self) {
        assert!(
            self.propagated.is_empty(),
            "Expected no propagated events, but got {:?}",
            self.propagated
        );
    }

    /// Assert that one or more [BroadcastEvent]s were emitted. No other
    /// messages should have bene propagated.
    #[track_caller]
    pub fn broadcast(self, expected: impl IntoIterator<Item = BroadcastEvent>) {
        let mut actual = Vec::new();
        for message in self.propagated {
            // Do this map in a for loop instead of map() so the panic gets
            // attributed to our caller
            if let Message::Event(Event::Broadcast(event)) = message {
                actual.push(event);
            } else {
                panic!(
                    "Expected only broadcasts to have been propagated,\
                        but received: {message:#?}"
                )
            }
        }
        let expected = expected.into_iter().collect_vec();
        assert_eq!(actual, expected);
    }

    /// Assert that only emitted events were propagated, and those events match
    /// a specific sequence. Requires `PartialEq` to be implemented for the
    /// emitted event type.
    #[track_caller]
    pub fn emitted<E>(self, expected: impl IntoIterator<Item = E>)
    where
        Comp: ToEmitter<E>,
        E: LocalEvent + PartialEq,
    {
        let emitter = self.component.to_emitter();
        let mut emitted = Vec::new();
        for message in self.propagated {
            // Do this map in a for loop instead of map() so the panic gets
            // attributed to our caller
            if let Message::Event(event) = message {
                match emitter.emitted(event) {
                    Ok(event) => emitted.push(event),
                    Err(event) => panic!(
                        "Expected only events emitted by {emitter} to have \
                        been propagated, but received: {event:#?}",
                    ),
                }
            } else {
                panic!(
                    "Expected only emitted events to have been propagated,\
                        but received: {message:#?}"
                )
            }
        }
        let expected = expected.into_iter().collect_vec();
        assert_eq!(emitted, expected);
    }
}

/// A wrapper component to provide global functionality to a component in unit
/// tests. This provides a modal queue for the action menu, which is normally
/// provided by the root component during app operation. This is included
/// automatically in all tests.
#[derive(Debug)]
struct TestWrapper<T> {
    inner: T,
    actions: ActionMenu,
}

impl<T> TestWrapper<T> {
    pub fn new(component: T) -> Self {
        Self {
            inner: component,
            actions: ActionMenu::default(),
        }
    }
}

impl<T: Component> Component for TestWrapper<T> {
    fn id(&self) -> ComponentId {
        self.inner.id()
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event.m().action(|action, propagate| match action {
            // Unfortunately we have to duplicate this with Root because the
            // child component is different
            Action::OpenActions => {
                // Walk down the component tree and collect actions from
                // all visible+focused components
                let actions = self.inner.collect_actions(context);
                self.actions.open(actions);
            }
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.actions.to_child_mut(), self.inner.to_child_mut()]
    }
}

impl<T, Props> Draw<Props> for TestWrapper<T>
where
    T: Component + Draw<Props>,
{
    fn draw(&self, canvas: &mut Canvas, props: Props, metadata: DrawMetadata) {
        canvas.draw(&self.inner, props, metadata.area(), metadata.has_focus());
        canvas.draw(&self.actions, (), metadata.area(), true);
    }
}
