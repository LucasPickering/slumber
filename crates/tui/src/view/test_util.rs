//! Test utilities specific to the TUI *view*

use crate::{
    context::TuiContext,
    http::RequestStore,
    test_util::{TestHarness, TestTerminal},
    view::{
        common::modal::ModalQueue,
        component::Component,
        context::ViewContext,
        draw::{Draw, DrawMetadata},
        event::{Child, Emitter, Event, EventHandler, ToChild, Update},
        UpdateContext,
    },
};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use itertools::Itertools;
use ratatui::{layout::Rect, Frame};
use std::{cell::RefCell, rc::Rc};

/// A wrapper around a component that makes it easy to test. This provides lots
/// of methods for sending events to the component. The goal is to make
/// realistic testing the easiest option, so component tests aren't contrived or
/// verbose.
pub struct TestComponent<'term, T, Props> {
    /// Terminal to draw to
    terminal: &'term TestTerminal,
    request_store: Rc<RefCell<RequestStore>>,
    /// The area the component will be drawn to. This defaults to the whole
    /// terminal but can be modified to test things like resizes, using
    /// [Self::set_area]
    area: Rect,
    component: Component<WithModalQueue<T>>,
    /// Whatever props were used for the most recent draw. We store these for
    /// convenience, because in most test cases we use the same props over and
    /// over, and just care about changes in response to events. This requires
    /// that `Props` implements `Clone`, but that's not a problem for most
    /// components since props typically just contain identifiers, references,
    /// and primitives. Modify using [Self::set_props].
    props: Props,
    /// Should the component be given focus on the next draw? Defaults to
    /// `true`
    has_focus: bool,
}

impl<'term, T, Props> TestComponent<'term, T, Props>
where
    Props: Clone,
    T: Draw<Props> + ToChild,
{
    /// Create a new component, then draw it to the screen and drain the event
    /// queue. Components aren't useful until they've been drawn once, because
    /// they won't receive events until they're marked as visible. For this
    /// reason, this constructor takes care of all the things you would
    /// immediately have to do anyway.
    ///
    /// This takes a a reference to the terminal so it can draw without having
    /// to plumb the terminal around to every draw call.
    pub fn new(
        harness: &TestHarness,
        terminal: &'term TestTerminal,
        data: T,
        initial_props: Props,
    ) -> Self {
        let component: Component<WithModalQueue<T>> =
            WithModalQueue::new(data).into();
        let mut slf = Self {
            terminal,
            request_store: Rc::clone(&harness.request_store),
            area: terminal.area(),
            component,
            props: initial_props,
            has_focus: true,
        };
        // Do an initial draw to set up state, then handle any triggered events
        slf.draw();
        slf
    }

    /// Get a reference to the wrapped component's inner data
    pub fn data(&self) -> &T {
        self.component.data().inner()
    }

    /// Get a mutable  reference to the wrapped component's inner data
    pub fn data_mut(&mut self) -> &mut T {
        self.component.data_mut().inner_mut()
    }

    /// Modify the area the component will be drawn to
    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    /// Set props to be used for future draws
    pub fn set_props(&mut self, props: Props) {
        self.props = props;
    }

    /// Enable focus for the next draw
    pub fn focus(&mut self) {
        self.has_focus = true;
    }

    /// Disable focus for the next draw
    pub fn unfocus(&mut self) {
        self.has_focus = false;
    }

    /// Draw this component onto the terminal, using the entire terminal frame
    /// as the draw area. If props are given, use them for the draw. If not,
    /// use the same props from the last draw.
    fn draw(&mut self) {
        self.terminal.draw(|frame| {
            self.component.draw(
                frame,
                self.props.clone(),
                self.area,
                self.has_focus,
            )
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
            if let Update::Propagate(event) =
                self.component.update_all(&mut update_context, event)
            {
                propagated.push(event);
            }
        }
        propagated
    }

    /// Drain all events in the queue, then draw the component to the terminal.
    ///
    /// This similar to [update_draw](Self::update_draw), but doesn't require
    /// you to queue a new event first. This is helpful in the rare occasions
    /// where the UI needs to respond to some asynchronous event, such as a
    /// callback that would normally be called by the main loop.
    pub fn drain_draw(&mut self) -> PropagatedEvents<'_, T> {
        let propagated = self.drain_events();
        self.draw();
        PropagatedEvents {
            component: self.data(),
            events: propagated,
        }
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
    pub fn update_draw(&mut self, event: Event) -> PropagatedEvents<'_, T> {
        // This is a safety check, so we don't end up handling events we didn't
        // expect to
        ViewContext::inspect_event_queue(|queue| {
            assert!(
                queue.is_empty(),
                "Event queue is not empty. To prevent unintended side-effects, \
                the queue must be empty before an update. {queue:?}"
            )
        });
        ViewContext::push_event(event);
        self.drain_draw()
    }

    /// Push a terminal input event onto the event queue, then drain events and
    /// draw. This will include the bound action for the event, based on the key
    /// code or mouse button. See [Self::update_draw] about return value.
    pub fn send_input(
        &mut self,
        crossterm_event: crossterm::event::Event,
    ) -> PropagatedEvents<'_, T> {
        let action = TuiContext::get().input_engine.action(&crossterm_event);
        let event = Event::Input {
            event: crossterm_event,
            action,
        };
        self.update_draw(event)
    }

    /// Simulate a left click at the given location, then drain events and draw.
    /// See [Self::update_draw] about return value.
    pub fn click(&mut self, x: u16, y: u16) -> PropagatedEvents<'_, T> {
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
    pub fn send_key(&mut self, code: KeyCode) -> PropagatedEvents<'_, T> {
        self.send_key_modifiers(code, KeyModifiers::NONE)
    }

    /// [Self::send_key], but with modifier keys applied
    pub fn send_key_modifiers(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> PropagatedEvents<'_, T> {
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
        &mut self,
        codes: impl IntoIterator<Item = KeyCode>,
    ) -> PropagatedEvents<'_, T> {
        let events = codes
            .into_iter()
            .flat_map(|code| self.send_key(code).events)
            .collect();
        PropagatedEvents {
            component: self.data(),
            events,
        }
    }

    /// Send some text as a series of key events, handling each event and
    /// re-drawing after each character. This may seem wasteful, but it most
    /// closely simulates what happens in the real world. Return propagated
    /// events from *all* updates, e.g. the concatenation of propagated events
    /// from each individual call to [Self::update_draw].
    pub fn send_text(&mut self, text: &str) -> PropagatedEvents<'_, T> {
        let events = text
            .chars()
            .flat_map(|c| self.send_key(KeyCode::Char(c)).events)
            .collect();
        PropagatedEvents {
            component: self.data(),
            events,
        }
    }
}

/// A collection of events that were propagated out from a particular
/// [TestComponent::update_draw] call. This wrapper makes it easy to check
/// which, if any, events were propagated.
#[must_use = "Propagated events must be checked"]
#[derive(derive_more::Debug)]
pub struct PropagatedEvents<'a, Component> {
    component: &'a Component,
    events: Vec<Event>,
}

impl<'a, Component> PropagatedEvents<'a, Component> {
    /// Assert that no events were propagated, i.e. the component handled all
    /// given and generated events.
    pub fn assert_empty(self) {
        assert!(
            self.events.is_empty(),
            "Expected no propagated events, but got {:?}",
            self.events
        )
    }

    /// Assert that only emitted events were propagated, and those events match
    /// a specific sequence. Requires `PartialEq` to be implemented for the
    /// emitted event type.
    pub fn assert_emitted(
        &self,
        expected: impl IntoIterator<Item = Component::Emitted>,
    ) where
        Component: Emitter,
        Component::Emitted: PartialEq,
    {
        let emitted = self
            .events
            .iter()
            .map(|event| {
                self.component.emitted(event).unwrap_or_else(|| {
                    panic!(
                        "Expected only emitted events to have been propagated, \
                        but received: {event:#?}\nAll: {:#?}",
                        self.events()
                    )
                })
            })
            .collect::<Vec<_>>();
        let expected = expected.into_iter().collect_vec();
        let expected = expected.iter().collect_vec();
        assert_eq!(emitted.as_slice(), expected.as_slice());
    }

    /// Get propagated events as a slice
    pub fn events(&self) -> &[Event] {
        &self.events
    }
}

/// A wrapper component to pair a component with a modal queue. Useful when the
/// component opens modals. This is included automatically in all tests, because
/// the modal queue is always present in the real app.
struct WithModalQueue<T> {
    inner: Component<T>,
    modal_queue: Component<ModalQueue>,
}

impl<T> WithModalQueue<T> {
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

impl<T: ToChild> EventHandler for WithModalQueue<T> {
    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.modal_queue.to_child_mut(), self.inner.to_child_mut()]
    }
}

impl<Props, T: Draw<Props>> Draw<Props> for WithModalQueue<T> {
    fn draw(&self, frame: &mut Frame, props: Props, metadata: DrawMetadata) {
        self.inner
            .draw(frame, props, metadata.area(), metadata.has_focus());
        self.modal_queue.draw(frame, (), metadata.area(), true);
    }
}
