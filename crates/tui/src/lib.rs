//! Terminal user interface for Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod context;
mod http;
mod input;
mod message;
mod state;
#[cfg(test)]
mod test_util;
mod util;
mod view;

use crate::{
    context::TuiContext,
    message::{Message, MessageSender},
    state::TuiState,
    util::{CANCEL_TOKEN, ResultReported},
};
use anyhow::Context;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::{StreamExt, pin_mut};
use ratatui::{Terminal, prelude::CrosstermBackend};
use slumber_config::{Action, Config};
use slumber_core::{collection::CollectionFile, database::Database};
use std::{
    io::{self, Stdout},
    ops::Deref,
    path::PathBuf,
    process::Command,
    time::Duration,
};
use tokio::{
    select,
    sync::mpsc::{self, UnboundedReceiver},
    task, time,
};
use tracing::{debug, error, info, info_span, trace};

/// Main controller struct for the TUI. The app uses a React-ish architecture
/// for the view, with a wrapping controller (this struct)
#[derive(Debug)]
pub struct Tui {
    terminal: Term,
    /// Receiver for the async message queue, which allows background tasks and
    /// the view to pass data and trigger side effects. Nobody else gets to
    /// touch this
    messages_rx: UnboundedReceiver<Message>,
    /// Transmitter for the async message queue, which can be freely cloned and
    /// passed around
    messages_tx: MessageSender,
    state: TuiState,
    should_run: bool,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

impl Tui {
    /// Rough **maximum** time for each iteration of the main loop
    const TICK_TIME: Duration = Duration::from_millis(250);

    /// Start the TUI. Any errors that occur during startup will be panics,
    /// because they prevent TUI execution.
    pub async fn start(collection_path: Option<PathBuf>) -> anyhow::Result<()> {
        // Create a message queue for handling async tasks
        let (messages_tx, messages_rx) = mpsc::unbounded_channel();
        let messages_tx = MessageSender::new(messages_tx);

        // Load config file. Failure shouldn't be fatal since we can fall back
        // to default, just show an error to the user
        let config = Config::load().reported(&messages_tx).unwrap_or_default();
        // Initialize global view context
        TuiContext::init(config);

        // Initialize TUI state, which will try to load the collection. If it
        // fails to load, we'll dump the user into an error state that watches
        // the file
        let collection_file = CollectionFile::new(collection_path)?;
        let database = Database::load()?
            .into_collection(&collection_file)
            .context("Error initializing database")?;
        let state =
            TuiState::load(database, collection_file, messages_tx.clone());

        // The code to revert the terminal takeover is in `Tui::drop`, so we
        // shouldn't take over the terminal until right before creating the
        // `Tui`.
        let terminal = initialize_terminal()?;

        let app = Tui {
            terminal,
            messages_rx,
            messages_tx,

            state,
            should_run: true,
        };

        // Run everything in one local set, so that we can use !Send values
        let local = task::LocalSet::new();
        local.spawn_local(app.run());
        local.await;
        Ok(())
    }

    /// Run the main TUI update loop. Any error returned from this is fatal. See
    /// the struct definition for a description of the different phases of the
    /// run loop.
    async fn run(mut self) -> anyhow::Result<()> {
        // Spawn background tasks
        self.listen_for_signals();

        let input_engine = &TuiContext::get().input_engine;
        // Stream of terminal input events
        let input_stream =
            // Events that don't map to a message (cursor move, focus, etc.)
            // should be filtered out entirely so they don't trigger any updates
            EventStream::new().filter_map(|event_result| async move {
                let event = event_result.expect("Error reading terminal input");
                input_engine.event_to_message(event)
            });
        pin_mut!(input_stream);

        self.draw()?; // Initial draw

        // This loop is limited by the rate that messages come in, with a
        // minimum rate enforced by a timeout
        while self.should_run {
            // ===== Message Phase =====
            // Grab one message out of the queue and handle it. This will block
            // while the queue is empty so we don't waste CPU cycles. The
            // timeout here makes sure we don't block forever, so things like
            // time displays during in-flight requests will update.

            let message = select! {
                event_option = input_stream.next() => {
                    if let Some(event) = event_option {
                        Some(event)
                    } else {
                        // We ran out of input, just end the program
                        break;
                    }
                },
                message = self.messages_rx.recv() => {
                    // Error would indicate a very weird and fatal bug so we
                    // wanna know about it
                    Some(message.expect("Message channel dropped while running"))
                },
                () = time::sleep(Self::TICK_TIME) => None,
            };

            // We'll try to skip draws if nothing on the screen has changed, to
            // limit idle CPU usage. If a request is running we always need to
            // update though, because the timer will be ticking.
            let mut needs_draw = self.state.has_active_requests();

            if let Some(message) = message {
                trace!(?message, "Handling message");
                // If an error occurs, store it so we can show the user
                self.handle_message(message).reported(&self.messages_tx);
                needs_draw = true;
            }

            // ===== Event Phase =====
            // Let the view handle all queued events. Trigger a draw if there
            // was anything in the queue.
            needs_draw |= self.state.drain_events();

            // ===== Draw Phase =====
            if needs_draw {
                self.draw()?;
            }
        }

        Ok(())
    }

    /// Handle an incoming message. Any error here will be displayed as a modal
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::Command(command) => self.run_command(command),

            // This message exists just to trigger a draw
            Message::Draw => Ok(()),

            // Force quit short-circuits the view/message cycle, to make sure
            // it doesn't get ate by text boxes
            Message::Input {
                action: Some(Action::ForceQuit),
                ..
            }
            | Message::Quit => {
                self.quit();
                Ok(())
            }
            Message::Input {
                event: Event::Resize(_, _),
                ..
            } => {
                // Redraw the entire screen. There are certain scenarios where
                // the terminal gets cleared but ratatui's (e.g. waking from
                // sleep) buffer doesn't, so the two get out of sync
                self.terminal.clear()?;
                self.draw()?;
                Ok(())
            }

            // Defer everything else to the inner state
            message => self.state.handle_message(message),
        }
    }

    /// Get a cheap clone of the message queue transmitter
    fn messages_tx(&self) -> MessageSender {
        self.messages_tx.clone()
    }

    /// Spawn a task to listen in the background for quit signals
    fn listen_for_signals(&self) {
        let messages_tx = self.messages_tx();
        util::spawn_result(async move {
            util::signals().await?;
            messages_tx.send(Message::Quit);
            Ok(())
        });
    }

    /// GOODBYE
    fn quit(&mut self) {
        info!("Initiating graceful shutdown");
        self.should_run = false;
        // Kill all background tasks
        CANCEL_TOKEN.cancel();
    }

    /// Draw the view onto the screen
    fn draw(&mut self) -> anyhow::Result<()> {
        self.terminal.draw(|frame| self.state.draw(frame))?;
        Ok(())
    }

    /// Run a **blocking** subprocess that will take over the terminal. Used
    /// for opening an external editor or pager.
    fn run_command(&mut self, mut command: Command) -> anyhow::Result<()> {
        let span = info_span!("Running command", ?command).entered();
        let error_context = format!("Error spawning command `{command:?}`");

        // Block while the editor runs. Useful for terminal editors since
        // they'll take over the whole screen. Potentially annoying for GUI
        // editors that block, but we'll just hope the command is
        // fire-and-forget. If this becomes an issue we can try to detect if the
        // subprocess took over the terminal and cut it loose if not, or add a
        // config field for it.
        self.terminal.draw(|frame| {
            frame.render_widget(
                "Waiting for subprocess to close...",
                frame.area(),
            );
        })?;

        let mut stdout = io::stdout();
        crossterm::execute!(stdout, LeaveAlternateScreen)?;
        command.status().context(error_context)?;
        // Some editors *cough* vim *cough* dump garbage to the event buffer on
        // exit. I've never figured out what actually causes it, but a simple
        // solution is to dump all events in the buffer before returning to
        // Slumber. It's possible we lose some real user input here (e.g. if
        // other events were queued behind the event to open the editor).
        util::clear_event_buffer();
        crossterm::execute!(stdout, EnterAlternateScreen)?;
        drop(span);

        // Redraw immediately. The main loop will probably be in the tick
        // timeout when we go back to it, so that adds a 250ms delay to
        // redrawing the screen that we want to skip.
        self.draw()?;

        Ok(())
    }
}

/// Restore terminal on app exit
impl Drop for Tui {
    fn drop(&mut self) {
        if let Err(err) = restore_terminal() {
            error!(error = err.deref(), "Error restoring terminal, sorry!");
        }
    }
}

/// Restore terminal state during a panic
fn initialize_panic_handler() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));
}

/// Set up terminal for TUI
fn initialize_terminal() -> anyhow::Result<Term> {
    initialize_panic_handler();
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

/// Return terminal to initial state
fn restore_terminal() -> anyhow::Result<()> {
    debug!("Restoring terminal");
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(())
}
