//! Test utilities specific to the TUI *view*

use crate::tui::{
    context::TuiContext,
    test_util::TestHarness,
    view::{
        component::Component,
        context::ViewContext,
        draw::Draw,
        event::{Event, EventHandler, Update},
    },
};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::text::Line;

/// A wrapper around a component that makes it easy to test. This provides lots
/// of methods for sending events to the component. The goal is to make
/// realistic testing the easiest option, so component tests aren't contrived or
/// verbose.
pub struct TestComponent<T, Props> {
    harness: TestHarness,
    component: Component<T>,
    /// Whatever props were used for the most recent draw. We store these for
    /// convenience, because in most test cases we use the same props over and
    /// over, and just care about changes in response to events. This requires
    /// that `Props` implements `Clone`, but that's not a problem for most
    /// components since props typically just contain identifiers, references,
    /// and primitives.
    last_props: Props,
}

impl<Props, T> TestComponent<T, Props>
where
    Props: Clone,
    T: Draw<Props> + EventHandler,
{
    /// Create a new component, then draw it to the screen and drain the event
    /// queue. Components aren't useful until they've been drawn once, because
    /// they won't receive events until they're marked as visible. For this
    /// reason, this constructor takes care of all the things you would
    /// immediately have to do anyway.
    ///
    /// This takes a test harness so it can access the terminal. Most tests only
    /// need to interact with a single component, so it's fine to pass ownership
    /// of the harness.
    pub fn new(harness: TestHarness, data: T, initial_props: Props) -> Self {
        let component: Component<T> = data.into();
        let mut slf = Self {
            harness,
            component,
            last_props: initial_props,
        };
        // Do an initial draw to set up state, then handle any triggered events
        slf.draw(None);
        // Ignore any propagated events from initialization. Maybe we *should*
        // be checking these, but the mechanics of that aren't smooth. Punting
        // for now
        let _ = slf.drain_events();
        slf
    }

    /// Get a mutable reference to the test harness
    pub fn harness_mut(&mut self) -> &mut TestHarness {
        &mut self.harness
    }

    /// Drop this component, returning the contained harness to be re-used
    pub fn into_harness(self) -> TestHarness {
        self.harness
    }

    /// Get a reference to the wrapped component's inner data
    pub fn data(&self) -> &T {
        self.component.data()
    }

    /// Alias for
    /// [TestBackend::assert_buffer_lines](ratatui::backend::TestBackend::assert_buffer_lines)
    pub fn assert_buffer_lines<'a>(
        &self,
        expected: impl IntoIterator<Item = impl Into<Line<'a>>>,
    ) {
        self.harness
            .terminal
            .backend()
            .assert_buffer_lines(expected)
    }

    /// Draw this component onto the terminal, using the entire terminal frame
    /// as the draw area. If props are given, use them for the draw. If not,
    /// use the same props from the last draw.
    fn draw(&mut self, props: Option<Props>) {
        if let Some(props) = props {
            self.last_props = props;
        }
        self.harness
            .terminal
            .draw(|frame| {
                self.component.draw(
                    frame,
                    self.last_props.clone(),
                    frame.size(),
                    true,
                )
            })
            .unwrap();
    }

    /// Drain events from the event queue, and handle them one-by-one. Return
    /// the events that were propagated (i.e. not consumed by the component or
    /// its children), in the order they were queued/handled.
    fn drain_events(&mut self) -> PropagatedEvents {
        // Safety check, prevent annoying bugs
        assert!(
            self.component.is_visible(),
            "Component {} is not visible, it can't handle events",
            self.component.name()
        );

        let mut propagated = Vec::new();
        while let Some(event) = ViewContext::pop_event() {
            if let Update::Propagate(event) = self.component.update_all(event) {
                propagated.push(event);
            }
        }
        PropagatedEvents(propagated)
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
    pub fn update_draw(&mut self, event: Event) -> PropagatedEvents {
        // This is a safety check, so we don't end up handling events we didn't
        // expect to
        ViewContext::inspect_event_queue(|queue| {
            assert!(
                queue.is_empty(),
                "Event queue is not empty. To prevent unintended side-effects, \
                the queue must be empty before an update."
            )
        });
        ViewContext::push_event(event);
        let propagated = self.drain_events();
        self.draw(None);
        propagated
    }

    /// Push a terminal input event onto the event queue, then drain events and
    /// draw. This will include the bound action for the event, based on the key
    /// code or mouse button. See [Self::update_draw] about return value.
    pub fn send_input(
        &mut self,
        crossterm_event: crossterm::event::Event,
    ) -> PropagatedEvents {
        let action = TuiContext::get().input_engine.action(&crossterm_event);
        let event = Event::Input {
            event: crossterm_event,
            action,
        };
        self.update_draw(event)
    }

    /// Simulate a left click at the given location, then drain events and draw.
    /// See [Self::update_draw] about return value.
    pub fn click(&mut self, x: u16, y: u16) -> PropagatedEvents {
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
    pub fn send_key(&mut self, code: KeyCode) -> PropagatedEvents {
        let crossterm_event = crossterm::event::Event::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        });
        self.send_input(crossterm_event)
    }

    /// Send some text as a series of key events, handling each event and
    /// re-drawing after each character. This may seem wasteful, but it most
    /// closely simulates what happens in the real world. Return propagated
    /// events from *all* updates, e.g. the concatenation of propagated events
    /// from each individual call to [Self::update_draw].
    pub fn send_text(&mut self, text: &str) -> PropagatedEvents {
        PropagatedEvents(
            text.chars()
                .flat_map(|c| self.send_key(KeyCode::Char(c)).0)
                .collect(),
        )
    }
}

/// A collection of events that were propagated out from a particular
/// [TestComponent::update_draw] call. This wrapper makes it easy to check
/// which, if any, events were propagated.
#[must_use = "Propagated events must be checked"]
pub struct PropagatedEvents(Vec<Event>);

impl PropagatedEvents {
    /// Assert that no events were propagated, i.e. the component handled all
    /// given and generated events.
    pub fn assert_empty(self) {
        assert!(
            self.0.is_empty(),
            "Expected no propagated events, but got {:?}",
            self.0
        )
    }
}
