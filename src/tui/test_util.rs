//! Test utilities specific to the TUI

use crate::{
    db::CollectionDatabase,
    test_util::Factory,
    tui::{
        context::TuiContext,
        message::{Message, MessageSender},
        view::ViewContext,
    },
};
use ratatui::{backend::TestBackend, Terminal};
use rstest::fixture;
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

    /// Assert the message queue is empty. Requires `&mut self` because it will
    /// actually pop a message off the queue
    pub fn assert_messages_empty(&mut self) {
        let message = self.messages_rx.try_recv().ok();
        assert!(
            message.is_none(),
            "Expected empty queue, but had message {message:?}"
        );
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

/// Assert that the event queue matches the given list of patterns
macro_rules! assert_events {
    ($($pattern:pat),* $(,)?) => {
        ViewContext::inspect_event_queue(|events| {
            crate::test_util::assert_matches!(events, &[$($pattern,)*]);
        });
    }
}
pub(crate) use assert_events;
