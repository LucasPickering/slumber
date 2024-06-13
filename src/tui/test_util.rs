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
use std::{
    env,
    sync::{Mutex, MutexGuard},
};
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

/// A guard used to indicate that the current process environment is locked.
/// This should be used in all tests that access environment variables, to
/// prevent interference from external variable settings or tests conflicting
/// with each other.
pub struct EnvGuard {
    previous_values: Vec<(String, Option<String>)>,
    #[allow(unused)]
    guard: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Lock the environment and set each given variable to its corresponding
    /// value. The returned guard will keep the environment locked so the
    /// calling test has exclusive access to it. Upon being dropped, the old
    /// environment values will be restored and then the environment will be
    /// unlocked.
    pub fn lock(
        variables: impl IntoIterator<
            Item = (impl Into<String>, Option<impl Into<String>>),
        >,
    ) -> Self {
        /// Global mutex for accessing environment variables. Technically we
        /// could break this out into a map with one mutex per variable, but
        /// that adds a ton of complexity for very little value.
        static MUTEX: Mutex<()> = Mutex::new(());

        let guard = MUTEX.lock().expect("Environment lock is poisoned");
        let previous_values = variables
            .into_iter()
            .map(|(variable, new_value)| {
                let variable: String = variable.into();
                let previous_value = env::var(&variable).ok();

                if let Some(value) = new_value {
                    env::set_var(&variable, value.into());
                } else {
                    env::remove_var(&variable);
                }

                (variable, previous_value)
            })
            .collect();

        Self {
            previous_values,
            guard,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Restore each env var
        for (variable, value) in &self.previous_values {
            if let Some(value) = value {
                env::set_var(variable, value);
            } else {
                env::remove_var(variable);
            }
        }
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
