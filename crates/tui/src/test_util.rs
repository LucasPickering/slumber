//! Test utilities specific to the TUI

use crate::{
    context::TuiContext,
    http::{RequestStore, ResponseParser},
    message::{Message, MessageSender},
    view::ViewContext,
};
use ratatui::{
    backend::TestBackend,
    layout::{Position, Rect},
    text::Line,
    Frame, Terminal,
};
use rstest::fixture;
use slumber_core::{
    collection::Collection,
    db::CollectionDatabase,
    http::{RequestId, ResponseRecord},
    test_util::Factory,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// Get a test harness, with a clean terminal etc. See [TestHarness].
#[fixture]
pub fn harness() -> TestHarness {
    TuiContext::init_test();
    let (messages_tx, messages_rx) = mpsc::unbounded_channel();
    let messages_tx: MessageSender = messages_tx.into();
    let collection = Collection::factory(()).into();
    let database = CollectionDatabase::factory(());
    let request_store = Rc::new(RefCell::new(RequestStore::new(
        database.clone(),
        TestResponseParser,
    )));
    ViewContext::init(
        Arc::clone(&collection),
        database.clone(),
        messages_tx.clone(),
    );
    TestHarness {
        collection,
        database,
        request_store,
        messages_tx,
        messages_rx,
    }
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
    pub request_store: Rc<RefCell<RequestStore>>,
    messages_tx: MessageSender,
    messages_rx: UnboundedReceiver<Message>,
}

impl TestHarness {
    /// Get the message sender
    pub fn messages_tx(&self) -> &MessageSender {
        &self.messages_tx
    }

    /// Pop the next message off the queue. Panic if the queue is empty
    pub fn pop_message_now(&mut self) -> Message {
        self.messages_rx.try_recv().expect("Message queue empty")
    }

    /// Pop the next message off the queue, waiting if empty
    pub async fn pop_message_wait(&mut self) -> Message {
        self.messages_rx.recv().await.expect("Message queue closed")
    }

    /// Clear all messages in the queue
    pub fn clear_messages(&mut self) {
        while self.messages_rx.try_recv().is_ok() {}
    }
}

/// Create a mock terminal, which can be written to for tests
#[fixture]
pub fn terminal(width: u16, height: u16) -> TestTerminal {
    TestTerminal::new(width, height)
}

/// Terminal width in chars, for injection to [terminal] fixture
#[fixture]
fn width() -> u16 {
    40
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
#[derive(Clone)]
pub struct TestTerminal(RefCell<Terminal<TestBackend>>);

impl TestTerminal {
    fn new(width: u16, height: u16) -> Self {
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
    pub fn assert_buffer_lines<'a>(
        &self,
        expected: impl IntoIterator<Item = impl Into<Line<'a>>>,
    ) {
        self.0.borrow().backend().assert_buffer_lines(expected)
    }

    pub fn draw(&self, f: impl FnOnce(&mut Frame)) {
        self.0.borrow_mut().draw(f).unwrap();
    }
}

/// Parse response bodies inline, for simplicity. Maybe not using the main
/// code path for tests is bad practice, but IMO it's not worth the effort
/// to make the background parser work in tests.
#[derive(Debug)]
pub struct TestResponseParser;

impl TestResponseParser {
    /// A helper for manually parsing a response body in tests
    pub fn parse_body(response: &mut ResponseRecord) {
        // Request ID is never used, so we can just pass a random one in
        Self.parse(RequestId::new(), response);
    }
}

impl ResponseParser for TestResponseParser {
    fn parse(&self, _: RequestId, response: &mut ResponseRecord) {
        let Some(content_type) = response.content_type() else {
            return;
        };
        let Ok(parsed) = content_type.parse_content(response.body.bytes())
        else {
            return;
        };
        response.set_parsed_body(parsed);
    }
}

/// Assert that the event queue matches the given list of patterns. Each event
/// can optionally include a conditional expression to apply additional
/// assertions.
macro_rules! assert_events {
    ($($pattern:pat $(if $condition:expr)?),* $(,)?) => {
        ViewContext::inspect_event_queue(|events| {
            // In order to support conditions on each individual event, we have
            // to unpack them here
            #[allow(unused_mut)]
            let mut len = 0;
            $(
                let Some(event) = events.get(len) else {
                    panic!(
                        "Expected event {expected} but reached end of queue",
                        expected = stringify!($pattern),
                    );
                };
                slumber_core::assert_matches!(event, $pattern $(if $condition)?);
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
