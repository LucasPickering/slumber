//! TUI testing utilities

use futures::Stream;
use ratatui::backend::TestBackend;
use rstest::fixture;
use slumber_tui::Tui;
use std::{
    fmt::Debug,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use terminput::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::{JoinHandle, LocalSet},
    time::{self, MissedTickBehavior},
};

/// Maximum duration to run any async test operations for. Any async operation
/// should resolve in this amount of time. Anything that takes longer will
/// panic.
pub const TIMEOUT: Duration = Duration::from_millis(1000);

pub type TestTui = Tui<TestBackend>;

/// Harness for running the TUI in integration tests
///
/// Spawn the TUI loop with [Self::run]. Use the combinator methods to send
/// input, awaiting conditions, etc. Call [Self::done] to shut the TUI down and
/// get it back.
#[must_use = "Call done() to run the TUI loop to completion"]
pub struct Runner {
    /// Local set to run all spawned tasks. This will run the TUI loop, all its
    /// spawned tasks, and whatever tasks we spawn in our own methods
    local: LocalSet,
    input_tx: UnboundedSender<Event>,
    /// Join handle for the TUI loop task. We'll await this in [Self::done]
    join_handle: JoinHandle<anyhow::Result<TestTui>>,
}

impl Runner {
    /// Create a new runner with the TUI loop running the background
    ///
    /// Because the TUI is run on a local set, **this method alone** will not
    /// cause it to run. Call [Self::done] to run the loop to completion.
    pub fn run(tui: TestTui) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let local = LocalSet::new();
        let join_handle = local.spawn_local(tui.run(InputStream(rx)));
        Self {
            input_tx: tx,
            local,
            join_handle,
        }
    }

    /// Simulate a key press with modifiers
    pub fn send_key_modifiers(
        self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Self {
        self.send_input(terminput::Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }))
    }

    /// Simulate user input
    pub fn send_input(self, event: terminput::Event) -> Self {
        self.input_tx
            .send(event)
            .expect("Cannot send input; TUI has dropped the receiver");
        self
    }

    /// Run a fallible future the local task set
    ///
    /// The given future will run to completion. Panic if it returns `Err` or
    /// takes over [TIMEOUT]. The TUI loop will be driven at the same time.
    ///
    /// The result is unwrapped outside the task because panics within tasks
    /// are swallowed.
    /// https://github.com/tokio-rs/tokio/issues/4516
    pub async fn run_until<E: Debug>(
        self,
        future: impl Future<Output = Result<(), E>>,
    ) -> Self {
        // Drive the whole task set, so the TUI loop runs concurrently
        time::timeout(TIMEOUT, self.local.run_until(future))
            .await
            .unwrap_or_else(|_| panic!("Future timed out after {TIMEOUT:?}"))
            .unwrap(); // Unwrap the future's result
        self
    }

    /// Wait for a condition to be `true`
    ///
    /// Call `cond` repeatedly until it returns `true`. Panic if it doesn't
    /// pass after [TIMEOUT].
    pub async fn wait_for(self, cond: impl Fn() -> bool) -> Self {
        const INTERVAL: Duration = Duration::from_millis(100);
        let future = async {
            let mut interval = time::interval(INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            while !cond() {
                interval.tick().await;
            }
        };
        // Run the future until completion or timeout. This will drive all
        // futures on the local set, so it will run the TUI loop as well
        time::timeout(TIMEOUT, self.local.run_until(future))
            .await
            .unwrap_or_else(|_| {
                panic!("Condition timed out after {TIMEOUT:?}")
            });
        self
    }

    /// Exit the TUI and wait for it to shut down
    ///
    /// Send ctrl-c to tell the TUI to shut down, then wait for it **and all
    /// other tasks** to exit. Return the TUI for future activities. Panic if
    /// the TUI loop fails or takes more than [TIMEOUT] to exit.
    pub async fn done(mut self) -> TestTui {
        // End every input sequence with ctrl-c, to ensure the app is exited
        self = self.send_key_modifiers(KeyCode::Char('c'), KeyModifiers::CTRL);

        // Run the TUI loop until completion
        let future = async {
            // Drive the local set until the TUI loop and ALL tasks are done
            self.local.await;
            // Now get the TUI back from the loop task. Awaiting this does NOT
            // drive the local set, so we have to do it second
            self.join_handle.await.unwrap()
        };
        // Use a short timeout to prevent slow/infinite tests
        #[expect(clippy::match_wild_err_arm)] // I like it better this way
        match time::timeout(TIMEOUT, future).await {
            Ok(Ok(tui)) => tui,
            Ok(Err(error)) => panic!("TUI failed with error: {error:#}"),
            Err(_) => panic!("Test timed out after {TIMEOUT:?}"),
        }
    }
}

/// Wrapper for an mpsc receiver to impl `Stream`
///
/// There's an implementation of this in `tokio_streams`, but it's not worth
/// pulling in a crate for.
struct InputStream(UnboundedReceiver<Event>);

impl Stream for InputStream {
    type Item = terminput::Event;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.0.is_closed() {
            let len = self.0.len();
            (len, Some(len))
        } else {
            (self.0.len(), None)
        }
    }
}

/// TODO comment
#[fixture]
pub fn backend(width: u16, height: u16) -> TestBackend {
    TestBackend::new(width, height)
}

/// Terminal width in chars, for injection to [backend] fixture
#[fixture]
fn width() -> u16 {
    50
}

/// Terminal height in chars, for injection to [backend] fixture
#[fixture]
fn height() -> u16 {
    20
}
