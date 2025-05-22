//! Test utilities specific to the TUI *view*

use crate::{
    context::TuiContext,
    http::RequestStore,
    test_util::{TestHarness, TestTerminal},
    view::{
        UpdateContext,
        common::{
            actions::ActionsModal,
            modal::{Modal, ModalQueue},
        },
        component::Component,
        context::ViewContext,
        draw::{Draw, DrawMetadata},
        event::{
            Child, Event, EventHandler, LocalEvent, OptionEvent, ToChild,
            ToEmitter,
        },
    },
};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use itertools::Itertools;
use ratatui::{Frame, layout::Rect};
use slumber_config::Action;
use std::{cell::RefCell, rc::Rc};

/// A wrapper around a component that makes it easy to test. This provides lots
/// of methods for sending events to the component. The goal is to make
/// realistic testing the easiest option, so component tests aren't contrived or
/// verbose.
///
/// This takes a a reference to the terminal so it can draw without having
/// to plumb the terminal around to every draw call.
#[derive(Debug)]
pub struct TestComponent<'term, T> {
    /// Terminal to draw to
    terminal: &'term TestTerminal,
    request_store: Rc<RefCell<RequestStore>>,
    /// The area the component will be drawn to. This defaults to the whole
    /// terminal but can be modified to test things like resizes, using
    /// [Self::set_area]
    area: Rect,
    component: Component<TestWrapper<T>>,
    /// Should the component be given focus on the next draw? Defaults to
    /// `true`
    has_focus: bool,
}

impl<'term, T> TestComponent<'term, T>
where
    T: ToChild,
{
    /// Start building a new component
    pub fn builder<Props>(
        harness: &TestHarness,
        terminal: &'term TestTerminal,
        data: T,
    ) -> TestComponentBuilder<'term, T, Props>
    where
        T: Draw<Props>,
    {
        let component: Component<TestWrapper<T>> =
            TestWrapper::new(data).into();
        TestComponentBuilder {
            terminal,
            request_store: Rc::clone(&harness.request_store),
            area: terminal.area(),
            component,
            props: None,
        }
    }

    /// Shortcut for building and drawing a component with default props and
    /// the full terminal area
    pub fn new<Props>(
        harness: &TestHarness,
        terminal: &'term TestTerminal,
        data: T,
    ) -> Self
    where
        T: Draw<Props>,
        Props: Default,
    {
        Self::builder(harness, terminal, data)
            .with_default_props()
            .build()
    }

    /// Get a reference to the wrapped component's inner data
    pub fn data(&self) -> &T {
        self.component.data().inner()
    }

    /// Get a mutable  reference to the wrapped component's inner data
    pub fn data_mut(&mut self) -> &mut T {
        self.component.data_mut().inner_mut()
    }

    /// Get the current visible modal, if any
    pub fn modal(&self) -> Option<&dyn Modal> {
        self.component.data().modal_queue.data().get()
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
    pub fn int<'a, Props>(&'a mut self) -> Interact<'term, 'a, T, Props>
    where
        T: Draw<Props>,
        Props: 'a + Default,
    {
        self.int_props(Props::default)
    }

    /// Get a helper to chain interactions and assertions on this component.
    /// Each draw will call the given props factory function to generate the
    /// next props value.
    pub fn int_props<'a, Props>(
        &'a mut self,
        props_factory: impl 'a + Fn() -> Props,
    ) -> Interact<'term, 'a, T, Props>
    where
        T: Draw<Props>,
    {
        Interact {
            component: self,
            props_factory: Box::new(props_factory),
            propagated: Vec::new(),
        }
    }

    /// Draw this component onto the terminal, using the entire terminal frame
    /// as the draw area. If props are given, use them for the draw. If not,
    /// use the same props from the last draw.
    fn draw<Props>(&mut self, props: Props)
    where
        T: Draw<Props>,
    {
        self.terminal.draw(|frame| {
            self.component.draw(frame, props, self.area, self.has_focus);
        });
    }

    /// Drain events from the event queue, and handle them one-by-one. Return
    /// the events that were propagated (i.e. not consumed by the component or
    /// its children), in the order they were queued/handled.
    fn drain_events(&mut self) -> Vec<Event> {
        // Safety check, prevent annoying bugs
        assert!(
            self.component.is_visible(),
            "Component {} is not visible, it can't handle events",
            self.component.name()
        );

        let mut propagated = Vec::new();
        let mut update_context = UpdateContext {
            request_store: &mut self.request_store.borrow_mut(),
        };
        while let Some(event) = ViewContext::pop_event() {
            if let Some(event) =
                self.component.update_all(&mut update_context, event)
            {
                propagated.push(event);
            }
        }
        propagated
    }
}

/// Helper for customizing a [TestComponent] before its initial draw
pub struct TestComponentBuilder<'term, T, Props> {
    terminal: &'term TestTerminal,
    request_store: Rc<RefCell<RequestStore>>,
    area: Rect,
    component: Component<TestWrapper<T>>,
    props: Option<Props>,
}

impl<'term, T, Props> TestComponentBuilder<'term, T, Props>
where
    T: Draw<Props> + ToChild,
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

    /// Build the component and do its initial draw. Components aren't useful
    /// until they've been drawn once, because they won't receive events
    /// until they're marked as visible. For this reason, this constructor
    /// takes care of all the things you would immediately have to do anyway.
    pub fn build(self) -> TestComponent<'term, T> {
        let mut component = TestComponent {
            terminal: self.terminal,
            request_store: self.request_store,
            area: self.area,
            component: self.component,

            has_focus: true,
        };
        // Do an initial draw to set up state, then handle any triggered events
        component.draw(self.props.expect("Props not set for test component"));
        component
    }
}

/// Utility class for interacting with a test component. This allows chaining
/// various interactions. All chains should be terminated with an assertion
/// on the events propagated by the interactions. Each interaction will be
/// succeeded by a single draw, to update the view as needed.
#[must_use = "Propagated events must be checked"]
#[derive(derive_more::Debug)]
pub struct Interact<'term, 'a, Component, Props> {
    component: &'a mut TestComponent<'term, Component>,
    /// A repeatable function that generates a props object for each draw. In
    /// most cases this will just be `Props::default` or a function that
    /// repeatedly returns the same static value. In some cases though, the
    /// value can't be held across draws and must be recreated each time.
    props_factory: Box<dyn 'a + Fn() -> Props>,
    propagated: Vec<Event>,
}

impl<Component, Props> Interact<'_, '_, Component, Props>
where
    Component: Draw<Props> + ToChild,
{
    /// Drain all events in the queue, then draw the component to the terminal.
    ///
    /// This similar to [update_draw](Self::update_draw), but doesn't require
    /// you to queue a new event first. This is helpful in the rare occasions
    /// where the UI needs to respond to some asynchronous event, such as a
    /// callback that would normally be called by the main loop.
    pub fn drain_draw(mut self) -> Self {
        let propagated = self.component.drain_events();
        self.component.draw((self.props_factory)());
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
        ViewContext::inspect_event_queue(|queue| {
            assert!(
                queue.is_empty(),
                "Event queue is not empty. To prevent unintended side effects, \
                the queue must be empty before an update. {queue:?}"
            );
        });
        ViewContext::push_event(event);
        self.drain_draw()
    }

    /// Push a terminal input event onto the event queue, then drain events and
    /// draw. This will include the bound action for the event, based on the key
    /// code or mouse button. See [Self::update_draw] about return value.
    pub fn send_input(self, crossterm_event: crossterm::event::Event) -> Self {
        let action = TuiContext::get().input_engine.action(&crossterm_event);
        let event = Event::Input {
            event: crossterm_event,
            action,
        };
        self.update_draw(event)
    }

    /// Simulate a left click at the given location, then drain events and draw.
    /// See [Self::update_draw] about return value.
    pub fn click(self, x: u16, y: u16) -> Self {
        let crossterm_event = crossterm::event::Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        self.send_input(crossterm_event)
    }

    /// Simulate a key press on this component. This will generate the
    /// corresponding event (including bound action, if any), send it to the
    /// component, then drain events and draw.  See
    /// [Self::update_draw] about return value.
    pub fn send_key(self, code: KeyCode) -> Self {
        self.send_key_modifiers(code, KeyModifiers::NONE)
    }

    /// [Self::send_key], but with modifier keys applied
    pub fn send_key_modifiers(
        self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Self {
        let crossterm_event = crossterm::event::Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        });
        self.send_input(crossterm_event)
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

    /// Open the actions menu
    pub fn open_actions(self) -> Self {
        self.send_key(KeyCode::Char('x'))
    }

    /// Assert that no events were propagated, i.e. the component handled all
    /// given and generated events.
    #[track_caller]
    pub fn assert_empty(self) {
        assert!(
            self.propagated.is_empty(),
            "Expected no propagated events, but got {:?}",
            self.propagated
        );
    }

    /// Assert that only emitted events were propagated, and those events match
    /// a specific sequence. Requires `PartialEq` to be implemented for the
    /// emitted event type.
    #[track_caller]
    pub fn assert_emitted<E>(self, expected: impl IntoIterator<Item = E>)
    where
        Component: ToEmitter<E>,
        E: LocalEvent + PartialEq,
    {
        let emitter = self.component.data().to_emitter();
        let emitted = self
            .propagated
            .into_iter()
            .map(|event| {
                emitter.emitted(event).unwrap_or_else(|event| {
                    panic!(
                        "Expected only emitted events to have been propagated, \
                        but received: {event:#?}",
                    )
                })
            })
            .collect::<Vec<_>>();
        let expected = expected.into_iter().collect_vec();
        assert_eq!(emitted, expected);
    }

    /// Get propagated events as a slice
    pub fn events(&self) -> &[Event] {
        &self.propagated
    }
}

/// A wrapper component to provide global functionality to a component in unit
/// tests. This provides a modal queue and action menu, which are provided by
/// the root component during app operation. This is included automatically in
/// all tests.
///
/// In a sense this is a duplicate of the root component. Maybe someday we could
/// make that component generic and get rid of this?
#[derive(Debug)]
struct TestWrapper<T> {
    inner: Component<T>,
    modal_queue: Component<ModalQueue>,
}

impl<T> TestWrapper<T> {
    pub fn new(component: T) -> Self {
        Self {
            inner: component.into(),
            modal_queue: ModalQueue::default().into(),
        }
    }

    pub fn inner(&self) -> &T {
        self.inner.data()
    }

    pub fn inner_mut(&mut self) -> &mut T {
        self.inner.data_mut()
    }
}

impl<T: ToChild> EventHandler for TestWrapper<T> {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().action(|action, propagate| match action {
            // Unfortunately we have to duplicate this with Root because the
            // child component is different
            Action::OpenActions => {
                // Walk down the component tree and collect actions from
                // all visible+focused components
                let actions = self.inner.collect_actions();
                ActionsModal::new(actions).open();
            }
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.modal_queue.to_child_mut(), self.inner.to_child_mut()]
    }
}

impl<Props, T: Draw<Props>> Draw<Props> for TestWrapper<T> {
    fn draw(&self, frame: &mut Frame, props: Props, metadata: DrawMetadata) {
        self.inner
            .draw(frame, props, metadata.area(), metadata.has_focus());
        self.modal_queue.draw(frame, (), metadata.area(), true);
    }
}
