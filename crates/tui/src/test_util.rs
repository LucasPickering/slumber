//! Test utilities specific to the TUI

use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Position, Rect},
    text::Line,
};
use rstest::fixture;
use std::cell::RefCell;

/// Create a mock terminal, which can be written to for tests
#[fixture]
pub fn terminal(width: u16, height: u16) -> TestTerminal {
    TestTerminal::new(width, height)
}

/// Terminal width in chars, for injection to [terminal] fixture
#[fixture]
fn width() -> u16 {
    50
}

/// Terminal height in chars, for injection to [terminal] fixture
#[fixture]
fn height() -> u16 {
    20
}

/// Wrapper around ratatui's terminal, to allow interior mutability. This is
/// needed so we can test multiple components in parallel, with each component
/// holding an immutable reference to the terminal. Mutable access is
/// encapulated within [Self::draw], so overlapping mutations is impossible.
#[derive(Clone, Debug)]
pub struct TestTerminal(RefCell<Terminal<TestBackend>>);

impl TestTerminal {
    pub fn new(width: u16, height: u16) -> Self {
        let backend = TestBackend::new(width, height);
        let terminal = Terminal::new(backend).unwrap();
        Self(terminal.into())
    }

    pub fn area(&self) -> Rect {
        self.0
            .borrow()
            .size()
            .map(|size| (Position::default(), size).into())
            .unwrap_or_default()
    }

    /// Alias for
    /// [TestBackend::assert_buffer_lines](ratatui::backend::TestBackend::assert_buffer_lines)
    #[track_caller]
    pub fn assert_buffer_lines<'a>(
        &self,
        expected: impl IntoIterator<Item = impl Into<Line<'a>>>,
    ) {
        self.0.borrow().backend().assert_buffer_lines(expected);
    }

    /// Draw to the frame
    pub fn draw(&self, f: impl FnOnce(&mut Frame)) {
        self.0.borrow_mut().draw(f).unwrap();
    }
}

/// Assert that the event queue matches the given list of patterns. Each event
/// can optionally include a conditional expression to apply additional
/// assertions.
macro_rules! assert_events {
    ($($pattern:pat $(if $condition:expr)?),* $(,)?) => {
        ViewContext::inspect_event_queue(|events| {
            // Can't use expect(unused_mut) because not all invocations trigger
            // the warning
            #[allow(clippy::allow_attributes)]
            #[allow(unused_mut)]
            let mut len = 0;

            // In order to support conditions on each individual event, we have
            // to unpack them here
            $(
                let Some(event) = events.get(len) else {
                    panic!(
                        "Expected event {expected} but reached end of queue",
                        expected = stringify!($pattern),
                    );
                };
                slumber_util::assert_matches!(event, $pattern $(if $condition)?);
                len += 1;
            )*
            // Make sure there aren't any trailing events
            let actual_len = events.len();
            assert_eq!(actual_len, len, "Too many events. Expected {len} but \
                got {actual_len}: {events:?}");
        });
    }
}
pub(crate) use assert_events;
