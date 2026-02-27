//! TUI testing utilities

#![allow(unused)] // Not all tests use everything

use futures::Stream;
use ratatui::{
    buffer::{Buffer, Cell},
    layout::{Position, Rect},
    prelude::Backend,
};
use rstest::fixture;
use slumber_config::Config;
use slumber_core::{
    collection::Collection, database::CollectionDatabase, http::RequestId,
};
use slumber_tui::{TerminalBackend, Tui};
use std::{
    cell::{Ref, RefCell},
    convert::Infallible,
    fmt::Debug,
    fs::File,
    iter,
    path::{Path, PathBuf},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    time::Duration,
};
use terminput::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::{JoinHandle, LocalSet},
    time::{self, Instant, MissedTickBehavior},
};
use tracing::{debug, trace};
use unicode_width::UnicodeWidthStr;
use wiremock::MockGuard;

/// Maximum duration to run any async test operations for. Any async operation
/// should resolve in this amount of time. Anything that takes longer will
/// panic.
const TIMEOUT: Duration = Duration::from_millis(1000);
/// Sleep time between loops for repeated check actions
const INTERVAL: Duration = Duration::from_millis(100);

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
    /// A refcounted pointer to the terminal backend. This allows us to access
    /// it while the TUI is running. Concurrent access is safe because this all
    /// runs in one thread.
    backend: TestBackend,
    input_tx: UnboundedSender<Event>,
    /// Join handle for the TUI loop task. We'll await this in [Self::done]
    join_handle: JoinHandle<anyhow::Result<TestTui>>,
}

impl Runner {
    /// Create a new runner with the TUI loop running the background
    ///
    /// Because the TUI is run on a local set, **this method alone** will not
    /// cause it to run. Call [Self::done] to run the loop to completion.
    pub fn new(tui: TestTui) -> Self {
        let local = LocalSet::new();
        let backend = tui.backend().clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let join_handle = local.spawn_local(tui.run(InputStream(rx)));
        Self {
            input_tx: tx,
            backend,
            local,
            join_handle,
        }
    }

    /// Simulate a key press
    pub fn send_key(self, code: KeyCode) -> Self {
        self.send_key_modifiers(code, KeyModifiers::NONE)
    }

    /// Simulate multiple key presses in sequence
    pub fn send_keys(
        mut self,
        codes: impl IntoIterator<Item = KeyCode>,
    ) -> Self {
        for code in codes {
            self = self.send_key(code);
        }
        self
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

    /// Send some text as a series of key events, handling each event and
    /// re-drawing after each character
    pub fn send_text(self, text: &str) -> Self {
        self.send_keys(text.chars().map(KeyCode::Char))
    }

    /// Simulate user input
    pub fn send_input(self, event: terminput::Event) -> Self {
        self.input_tx
            .send(event)
            .expect("Cannot send input; TUI has dropped the receiver");
        self
    }

    /// Open the action menu, select an action by index, and execute it
    ///
    /// The path is one or more indices, one per layer. Each index is the number
    /// from the top (starting at 0), *excluding* disabled items.
    pub fn action(self, path: &[usize]) -> Self {
        // Theoretically this could take an action by name and examine the
        // screen to find its index, but that's way more work
        self.send_key(KeyCode::Char('x'))
            // For each layer, go down n times, then submit. For parents this
            // will open the next layer, for leaves it will send the action
            .send_keys(path.iter().flat_map(|index| {
                iter::repeat_n(KeyCode::Down, *index)
                    .chain(iter::once(KeyCode::Enter))
            }))
    }

    /// Open the recipe list, select a recipe by index, and send it
    ///
    /// The index is the number from the top (starting at 0)
    pub fn send_request(self, index: usize) -> Self {
        self.send_key(KeyCode::Char('r'))
            .send_keys(iter::repeat_n(KeyCode::Down, index))
            .send_key(KeyCode::Enter)
    }

    /// Run a fallible future the local task set
    ///
    /// Use this for futures that have to run concurrently with the TUI.
    pub async fn run_until<E: Debug>(
        self,
        future: impl Future<Output = Result<(), E>>,
    ) -> Self {
        // Yield the thread momentarily to the TUI loop. This helps ensure the
        // TUI has all background tasks spawned before we do any operations.
        self.local.run_until(time::sleep(Duration::ZERO)).await;

        // Drive the whole task set, so the TUI loop runs concurrently
        time::timeout(TIMEOUT, self.local.run_until(future))
            .await
            .unwrap_or_else(|_| panic!("Future timed out after {TIMEOUT:?}"))
            // The result is unwrapped outside the task because panics within
            // tasks are swallowed
            // https://github.com/tokio-rs/tokio/issues/4516
            .unwrap();
        self
    }

    /// Wait for the response to a mocked request
    pub async fn wait_for_request(self, mock_guard: MockGuard) -> Self {
        self.run_until(async move {
            mock_guard.wait_until_satisfied().await;
            // Eww stinky sleep bad. We need to wait for the TUI loop task to
            // receive the response message. Otherwise, it's possible to exit
            // this call and immediatelly call done(), which will terminate the
            // loop before it has a chance to receive the message in the queue.
            //
            // Using a sleep sucks, but I couldn't find any other async
            // conditions to wait on. We could repeatedly check the DB, but that
            // ends up taking longer and hogs the main thread. This sleep went
            // 1000/1000 in a test so I think we're good :)
            time::sleep(Duration::from_millis(100)).await;
            Ok::<_, Infallible>(())
        })
        .await
    }

    /// Wait for the terminal to contain specific text at a location
    pub async fn wait_for_content(self, expected: &str, at: Position) -> Self {
        // Each time the check fails, store the error message. We'll panic with
        // the final error message to show the user the failure state
        let mut error: String = String::new();
        let future = async {
            let start = Instant::now();
            let mut i = 1;
            while let Err(e) = self.backend.try_buffer_contains(expected, at) {
                error = e;
                time::sleep(INTERVAL).await;
                i += 1;
            }
            let elapsed = Instant::elapsed(&start);
            debug!("Content available after {elapsed:?} ({i} checks)");
        };

        // Run the future until completion or timeout. This will drive all
        // futures on the local set, so it will run the TUI loop as well
        time::timeout(TIMEOUT, self.local.run_until(future))
            .await
            // If we time out, panic with the most recent error message
            .unwrap_or_else(|_| panic!("After {TIMEOUT:?}: {error}"));

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

/// Create a config file with the given data and return its path
pub fn config_file(directory: &Path, config: &Config) -> PathBuf {
    let path = directory.join("config.yml");
    let mut file = File::create_new(&path).unwrap();
    serde_yaml::to_writer(&file, config).unwrap();
    path
}

/// Create a collection file with the given data, and return its path
pub fn collection_file(directory: &Path, collection: &Collection) -> PathBuf {
    let path = directory.join("slumber.yml");
    let mut file = File::create_new(&path).unwrap();
    serde_yaml::to_writer(&file, collection).unwrap();
    path
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

/// A wrapper around [ratatui::backend::TestBackend] that allows shared mutable
/// access
///
/// We have to pass an owned copy of this to the TUI, but we also need access
/// during test combinator functions. Because the TUI is run on a local thread,
/// the futures can be `!Send`, allowing `Rc<RefCell<>>`.
#[derive(Clone)]
pub struct TestBackend {
    backend: Rc<RefCell<ratatui::backend::TestBackend>>,
    /// Text that has been copied to the clipboard
    clipboard: Vec<String>,
}

impl TestBackend {
    pub fn new(width: u16, height: u16) -> Self {
        let backend = ratatui::backend::TestBackend::new(width, height);
        Self {
            backend: Rc::new(RefCell::new(backend)),
            clipboard: vec![],
        }
    }

    /// Assert that the screen buffer contains specific text at a location
    #[track_caller]
    pub fn assert_buffer_contains(&self, expected: &str, at: Position) {
        if let Err(error) = self.try_buffer_contains(expected, at) {
            panic!("{error}"); // Panic with Display instead of Debug impl
        }
    }

    /// Get the sequence of texts copied to the clipboard
    pub fn clipboard(&self) -> &[String] {
        &self.clipboard
    }

    /// Check if the screen buffer contains specific text at a location
    fn try_buffer_contains(
        &self,
        expected: &str,
        at: Position,
    ) -> Result<(), String> {
        let width = expected.width(); // Use char count, not byte len
        let area = Rect {
            x: at.x,
            y: at.y,
            width: width as u16,
            height: 1, // Text has to all be on one line
        };
        assert_eq!(area.area() as usize, expected.width()); // Sanity check
        let buffer = self.buffer();
        let actual = area
            .positions()
            .filter_map(|pos| buffer.cell(pos).map(Cell::symbol))
            .collect::<String>();

        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "Expected buffer to contain {expected:?} at {at}, \
                but was {actual:?}: {buffer:#?}"
            ))
        }
    }

    /// Get a reference to the screen buffer. This borrows the `RefCell`, so
    /// don't hold it longer than you need it.
    pub fn buffer(&'_ self) -> Ref<'_, Buffer> {
        Ref::map(self.backend.borrow(), |backend| backend.buffer())
    }
}

impl Backend for TestBackend {
    type Error = Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        self.backend.borrow_mut().draw(content)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.backend.borrow_mut().hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.backend.borrow_mut().show_cursor()
    }

    fn get_cursor_position(
        &mut self,
    ) -> Result<ratatui::prelude::Position, Self::Error> {
        self.backend.borrow_mut().get_cursor_position()
    }

    fn set_cursor_position<P: Into<ratatui::prelude::Position>>(
        &mut self,
        position: P,
    ) -> Result<(), Self::Error> {
        self.backend.borrow_mut().set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.backend.borrow_mut().clear()
    }

    fn clear_region(
        &mut self,
        clear_type: ratatui::prelude::backend::ClearType,
    ) -> Result<(), Self::Error> {
        self.backend.borrow_mut().clear_region(clear_type)
    }

    fn size(&self) -> Result<ratatui::prelude::Size, Self::Error> {
        self.backend.borrow().size()
    }

    fn window_size(
        &mut self,
    ) -> Result<ratatui::prelude::backend::WindowSize, Self::Error> {
        self.backend.borrow_mut().window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.backend.borrow_mut().flush()
    }
}

impl TerminalBackend for TestBackend {
    fn copy_to_clipboard(&mut self, text: String) -> anyhow::Result<()> {
        self.clipboard.push(text);
        Ok(())
    }
}

/// Test fixture to create a test backend, which can be used to initialize the
/// TUI
#[fixture]
pub fn backend(width: u16, height: u16) -> TestBackend {
    TestBackend::new(width, height)
}

/// Terminal width in chars, for injection to [backend] fixture
#[fixture]
fn width() -> u16 {
    60
}

/// Terminal height in chars, for injection to [backend] fixture
#[fixture]
fn height() -> u16 {
    20
}
