//! Test utilities specific to the TUI

use crate::tui::{
    context::TuiContext,
    message::{Message, MessageSender},
    view::ViewContext,
};
use ratatui::{backend::TestBackend, Terminal};
use rstest::fixture;
use slumber_core::{db::CollectionDatabase, test_util::Factory};
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// Get a test harness, with a clean terminal etc. See [TestHarness].
#[fixture]
pub fn harness(terminal_width: u16, terminal_height: u16) -> TestHarness {
    TuiContext::init_test();
    let (messages_tx, messages_rx) = mpsc::unbounded_channel();
    let messages_tx: MessageSender = messages_tx.into();
    let database = CollectionDatabase::factory(());
    ViewContext::init(database.clone(), messages_tx.clone());
    let backend = TestBackend::new(terminal_width, terminal_height);
    let terminal = Terminal::new(backend).unwrap();
    TestHarness {
        database,
        messages_tx,
        messages_rx,
        terminal,
    }
}

/// Terminal width in chars, for injection to [harness] fixture
#[fixture]
fn terminal_width() -> u16 {
    40
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
    pub database: CollectionDatabase,
    pub terminal: Terminal<TestBackend>,
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
                        "Expected event {expected} but queue is empty",
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
