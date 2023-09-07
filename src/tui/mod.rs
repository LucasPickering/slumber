mod input;
mod state;
mod view;

use crate::{
    config::RequestCollection,
    http::HttpEngine,
    tui::{
        input::Action,
        state::{AppState, Message, ResponseState},
        view::Renderer,
    },
    util::UnboundedSenderExt,
};
use anyhow::{anyhow, Context};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::CrosstermBackend, Terminal};
use signal_hook::{
    consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM},
    iterator::Signals,
};
use std::{
    error::Error,
    io::{self, Stdout},
    ops::Deref,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tracing::{error, info};

/// Main controller struct for the TUI. The app uses an MVC architecture, and
/// this is the C
#[derive(Debug)]
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    messages_rx: UnboundedReceiver<Message>,
    renderer: Renderer,
    http_engine: HttpEngine,
    state: AppState,
}

impl Tui {
    /// Start the TUI
    pub fn start(collection: RequestCollection) -> anyhow::Result<()> {
        initialize_panic_handler();

        // Set up terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        // Create a message queue for handling async tasks
        let (messages_tx, messages_rx) = mpsc::unbounded_channel();

        let mut app = Tui {
            terminal,
            messages_rx,
            renderer: Renderer::new(),
            http_engine: HttpEngine::new(),
            state: AppState::new(collection, messages_tx),
        };

        app.run()
    }

    /// Run the main TUI update loop
    fn run(&mut self) -> anyhow::Result<()> {
        // Listen for signals to stop the program
        let mut quit_signals = Signals::new([SIGHUP, SIGINT, SIGTERM, SIGQUIT])
            .context("Error creating signal handler")?;

        let tick_rate = Duration::from_millis(250);
        let mut last_tick = Instant::now();

        while self.state.should_run() {
            self.terminal
                .draw(|f| self.renderer.draw_main(f, &mut self.state))?;

            // Handle all messages in the queue before accepting new input
            while let Ok(message) = self.messages_rx.try_recv() {
                // If an error occurs, store it so we can show the user
                if let Err(err) = self.handle_message(message) {
                    error!(error = err.deref(), "Error handling message");
                    self.state.error = Some(err);
                }
            }

            // Check for any new events
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            if crossterm::event::poll(timeout)? {
                if let Some(action) =
                    Action::from_event(crossterm::event::read()?)
                {
                    input::handle_action(&mut self.state, action);
                }
            }

            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
            }

            // Check for exit signals
            if quit_signals.pending().next().is_some() {
                self.state.quit();
            }
        }
        Ok(())
    }

    /// Handle an incoming message. Any error here will be displayed as a popup
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::SendRequest => {
                self.send_request()?;
            }
            Message::Response(response) => {
                if let Some(request_state) = &mut self.state.active_request {
                    request_state.response = response;
                }
            }
        }
        Ok(())
    }

    /// Launch an HTTP request in a separate task
    fn send_request(&mut self) -> anyhow::Result<()> {
        let recipe = self
            .state
            .recipes
            .selected()
            .ok_or_else(|| anyhow!("No recipe selected"))?;

        // Build the request first
        let request = self
            .http_engine
            .build_request(recipe, &(&self.state).into())?;
        self.state.active_request = Some(request.clone().into());
        let messages_tx = self.state.messages_tx.clone();
        let http_engine = self.http_engine.clone();

        // Launch the request in a separate task so it doesn't block
        tokio::spawn(async move {
            let response_state = match http_engine.send_request(request).await {
                Ok(response) => {
                    info!(?response, "HTTP request succeeded");
                    ResponseState::Complete(response)
                }
                Err(err) => {
                    // yikes
                    error!(error = &err as &dyn Error, "HTTP request failed");
                    ResponseState::Error(err)
                }
            };
            // Send the response back to the main thread
            messages_tx.send_unwrap(Message::Response(response_state));
        });
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
        restore_terminal().unwrap();
        original_hook(panic_info);
    }));
}

/// Return terminal to initial state
fn restore_terminal() -> anyhow::Result<()> {
    info!("Restoring terminal");
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        std::io::stderr(),
        LeaveAlternateScreen,
        DisableMouseCapture,
    )?;
    Ok(())
}
