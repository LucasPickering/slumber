mod config;
mod http;
mod input;
mod state;
mod template;
mod view;

use crate::{
    config::RequestCollection,
    http::HttpEngine,
    input::Action,
    state::{AppState, Message},
    view::Renderer,
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
    io::{self, Stdout},
    ops::Deref,
    path::PathBuf,
    time::{Duration, Instant},
};
use tracing::{error, info};
use tracing_subscriber::{filter::EnvFilter, prelude::*};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_panic_handler();
    initialize_tracing()?;
    let collection = RequestCollection::load(None).await?;
    App::start(collection)
}

/// Main controller struct. The app uses an MVC architecture, and this is the C
#[derive(Debug)]
pub struct App {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    renderer: Renderer,
    http_engine: HttpEngine,
    state: AppState,
}

impl App {
    /// Start the TUI
    pub fn start(collection: RequestCollection) -> anyhow::Result<()> {
        // Set up terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut app = App {
            terminal,
            renderer: Renderer::new(),
            http_engine: HttpEngine::new(),
            state: collection.into(),
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

            // Handle all messages in the queue before accepting new input.
            // Can't use a for loop because that maintains a mutable ref to self
            while let Some(message) = self.state.dequeue() {
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
                let recipe =
                    self.state.recipes.selected().ok_or_else(|| {
                        anyhow!("Cannot send request with no recipe selected")
                    })?;

                // Build the request, then launch it
                self.state.active_request = Some(
                    self.http_engine
                        .build_request(recipe, &(&self.state).into())?,
                );
                // Unwrap is safe because we *just* populated it
                self.http_engine
                    .send_request(self.state.active_request.as_ref().unwrap());
            }
        }
        Ok(())
    }
}

/// Restore terminal on app exit
impl Drop for App {
    fn drop(&mut self) {
        if let Err(err) = restore_terminal() {
            error!(error = err.deref(), "Error restoring terminal, sorry!");
        }
    }
}

/// Set up tracing to log to a file
fn initialize_tracing() -> anyhow::Result<()> {
    let directory = PathBuf::from("./log/");
    std::fs::create_dir_all(directory.clone())
        .context(format!("Error creating log directory {directory:?}"))?;
    let log_path = directory.join("ratatui-app.log");
    let log_file = std::fs::File::create(log_path)?;
    let file_subscriber = tracing_subscriber::fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_writer(log_file)
        .with_target(false)
        .with_ansi(false)
        .with_filter(EnvFilter::from_default_env());
    tracing_subscriber::registry().with(file_subscriber).init();
    Ok(())
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
