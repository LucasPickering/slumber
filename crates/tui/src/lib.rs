//! Terminal user interface for Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod context;
mod http;
mod input;
mod message;
#[cfg(test)]
mod test_util;
mod tui_state;
mod util;
mod view;

use crate::{
    context::TuiContext,
    input::InputEvent,
    message::{Message, MessageSender},
    tui_state::TuiState,
    util::{CANCEL_TOKEN, ResultReported},
};
use anyhow::Context;
use crossterm::event::{self, EventStream};
use futures::{StreamExt, pin_mut};
use ratatui::{
    Terminal,
    backend::{ClearType, WindowSize},
    layout::{Position, Size},
    prelude::{Backend, CrosstermBackend},
};
use slumber_config::{Action, Config};
use slumber_core::{collection::CollectionFile, database::Database};
use std::{
    convert::Infallible,
    io::{self, Stdout},
    ops::Deref,
    path::PathBuf,
    time::Duration,
};
use tokio::{
    select,
    sync::mpsc::{self, UnboundedReceiver},
    task, time,
};
use tracing::{error, info, trace};

/// Main controller struct for the TUI. The app uses a React-ish architecture
/// for the view, with a wrapping controller (this struct)
#[derive(Debug)]
pub struct Tui {
    terminal: Term,
    /// Null terminal for state-only draws
    null_terminal: Terminal<NullBackend>,
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
        initialize_panic_handler();
        util::initialize_terminal()?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

        let app = Tui {
            terminal,
            null_terminal: Terminal::new(NullBackend)?,
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
                // Convert from crossterm to the common terminput format. This
                // enables support for multiple terminal backends
                let event = terminput_crossterm::to_terminput(event).unwrap();
                input_engine.convert_event(event)
            });
        pin_mut!(input_stream);

        self.draw(false)?; // Initial draw

        // This loop is limited by the rate that messages come in, with a
        // minimum rate enforced by a timeout
        while self.should_run {
            // ===== Message Phase =====
            // Wait for one of 3 things to happen:
            // - Message appears in the queue
            // - Input event from the terminal
            // - Timeout (to ensure we show state updates while a request is
            //   ticking)
            //
            // The goal is to only do work when there's something to do, to
            // minimize the idle CPU usage

            let message = select! {
                // The ordering and usage of `biased` is very important here:
                // if there's a message in the queue, we want to handle it
                // immediately *before* the input stream is polled. If the
                // message triggers a subprocess that yields the terminal, then
                // polling the input stream can interfere with the spawned
                // process. By checking the message queue first, we ensure the
                // input stream only gets polled when there are no messages.
                // See https://github.com/LucasPickering/slumber/issues/506 and
                // associated PR
                biased;
                message = self.messages_rx.recv() => {
                    // Error would indicate a very weird and fatal bug so we
                    // wanna know about it
                    Some(message.expect("Message channel dropped while running"))
                },
                event_option = input_stream.next() => {
                    if let Some(event) = event_option {
                        Some(Message::Input(event))
                    } else {
                        // We ran out of input, just end the program
                        break;
                    }
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
                // Skip the terminal render if we have more messages/events in
                // the queue
                let has_message = !self.messages_rx.is_empty();
                let has_input = event::poll(Duration::ZERO).unwrap_or(false);
                self.draw(has_message || has_input)?;
            }
        }

        Ok(())
    }

    /// Handle an incoming message. Any error here will be displayed as a modal
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::ClearTerminal => {
                self.terminal.clear()?;
                Ok(())
            }

            // This message exists just to trigger a draw
            Message::Draw => Ok(()),

            // Force quit short-circuits the view/message cycle, to make sure
            // it doesn't get ate by text boxes
            Message::Input(InputEvent::Key {
                action: Some(Action::ForceQuit),
                ..
            })
            | Message::Quit => {
                self.quit();
                Ok(())
            }
            Message::Input(InputEvent::Resize { .. }) => {
                // Redraw the entire screen. There are certain scenarios where
                // the terminal gets cleared but ratatui's (e.g. waking from
                // sleep) buffer doesn't, so the two get out of sync
                self.terminal.clear()?;
                self.draw(false)?;
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

    /// Draw the view onto the screen. If `null` is true, the draw will be done
    /// with a null backend. This will update all state in the component tree,
    /// but won't actually write to the terminal buffer. This should be enabled
    /// when we know there will be subsequent draws (i.e. if there are more
    /// events in the queue) to improve performance.
    fn draw(&mut self, null: bool) -> anyhow::Result<()> {
        if null {
            self.null_terminal.draw(|frame| self.state.draw(frame))?;
        } else {
            self.terminal.draw(|frame| self.state.draw(frame))?;
        }
        Ok(())
    }
}

/// Restore terminal on app exit
impl Drop for Tui {
    fn drop(&mut self) {
        if let Err(err) = util::restore_terminal() {
            error!(error = err.deref(), "Error restoring terminal, sorry!");
        }
    }
}

/// A null implementation of [Backend] that does nothing for all operations
#[derive(Debug)]
struct NullBackend;

impl Backend for NullBackend {
    type Error = Infallible;

    fn draw<'a, I>(&mut self, _content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(Position::default())
    }

    fn set_cursor_position<P: Into<Position>>(
        &mut self,
        _position: P,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn clear_region(
        &mut self,
        _clear_type: ClearType,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(Size::default())
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(WindowSize {
            columns_rows: Size::default(),
            pixels: Size::default(),
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Restore terminal state during a panic
fn initialize_panic_handler() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = util::restore_terminal();
        original_hook(panic_info);
    }));
}
