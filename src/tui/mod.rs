mod input;
mod state;
mod view;

use crate::{
    config::RequestCollection,
    http::{HttpEngine, Request},
    repository::Repository,
    tui::{
        input::InputManager,
        state::{AppState, Message},
        view::Renderer,
    },
    util::ResultExt,
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
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tracing::{debug, error};

/// Main controller struct for the TUI. The app uses an MVC architecture, and
/// this is the C. The main loop goes through the following phases on each
/// iteration:
///
/// - Input phase: Check for input from the user
/// - Message phase: Process any async messages from input or external sources
///   (HTTP, file system, etc.)
/// - Draw phase: Draw the entire UI
/// - Signal phase: Check for process signals that should trigger an exit
#[derive(Debug)]
pub struct Tui {
    // All state should generally be stored in [AppState]. This stored here
    // are more functionality than data.
    terminal: Terminal<CrosstermBackend<Stdout>>,
    messages_rx: UnboundedReceiver<Message>,
    renderer: Renderer,
    http_engine: HttpEngine,
    state: AppState,
}

impl Tui {
    /// Rough maximum time for each iteration of the main loop
    const TICK_TIME: Duration = Duration::from_millis(100);

    /// Start the TUI. Any errors that occur during startup will be panics,
    /// because they prevent TUI execution.
    pub fn start(collection_file: PathBuf, collection: RequestCollection) {
        initialize_panic_handler();

        // Set up terminal
        enable_raw_mode().expect("Error initializing terminal");
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .expect("Error initializing terminal");
        let backend = CrosstermBackend::new(stdout);
        let terminal =
            Terminal::new(backend).expect("Error initializing terminal");

        // Create a message queue for handling async tasks
        let (messages_tx, messages_rx) = mpsc::unbounded_channel();

        let repository = Repository::load().unwrap();
        let mut app = Tui {
            terminal,
            messages_rx,
            renderer: Renderer::new(),
            http_engine: HttpEngine::new(),
            state: AppState::new(
                collection_file,
                collection,
                repository,
                messages_tx,
            ),
        };

        // Any error during execution that gets this far is fatal. We expect the
        // error to already have context attached so we can just unwrap
        app.run().unwrap();
    }

    /// Run the main TUI update loop. Any error returned from this is fatal. See
    /// the struct definition for a description of the different phases of the
    /// run loop.
    fn run(&mut self) -> anyhow::Result<()> {
        // Listen for signals to stop the program
        let mut quit_signals = Signals::new([SIGHUP, SIGINT, SIGTERM, SIGQUIT])
            .context("Error creating signal handler")?;

        let mut last_tick = Instant::now();

        while self.state.should_run() {
            // ===== Input Phase =====
            let timeout = Self::TICK_TIME
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            // This is where the tick rate is enforced
            if crossterm::event::poll(timeout)? {
                InputManager::instance()
                    .handle_event(&mut self.state, crossterm::event::read()?);
            }
            if last_tick.elapsed() >= Self::TICK_TIME {
                last_tick = Instant::now();
            }

            // ===== Message Phase =====
            while let Ok(message) = self.messages_rx.try_recv() {
                // If an error occurs, store it so we can show the user
                self.handle_message(message)
                    .ok_or_apply(|err| self.state.set_error(err));
            }

            // ===== Draw Phase =====
            self.terminal
                .draw(|f| self.renderer.draw_main(f, &mut self.state))?;

            // ===== Signal Phase =====
            if quit_signals.pending().next().is_some() {
                self.state.quit();
            }
        }
        Ok(())
    }

    /// Handle an incoming message. Any error here will be displayed as a popup
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::StartReloadCollection => {
                let messages_tx = self.state.messages_tx.clone();
                let collection_file = self.state.collection_file().to_owned();
                tokio::spawn(async move {
                    let (_, collection) =
                        RequestCollection::load(Some(&collection_file))
                            .await
                            .ok_or_apply(|err| {
                            messages_tx.send(Message::Error { error: err })
                        })?;
                    messages_tx.send(Message::EndReloadCollection {
                        collection_file,
                        collection,
                    });
                    // Return an option just to allow bailing above
                    None::<()>
                });
            }
            Message::EndReloadCollection {
                collection_file,
                collection,
            } => {
                self.state.reload_collection(collection);
                // Send the notification *after* reloading, otherwise it'll get
                // wiped out immediately
                self.state.notify(format!(
                    "Reloaded collection from {}",
                    collection_file.to_string_lossy()
                ));
            }
            Message::HttpSendRequest => self.send_request()?,
            Message::Error { error } => self.state.set_error(error),
        }
        Ok(())
    }

    /// Launch an HTTP request in a separate task
    fn send_request(&mut self) -> anyhow::Result<()> {
        let recipe = self
            .state
            .ui
            .recipes
            .selected()
            .ok_or_else(|| anyhow!("No recipe selected"))?
            .clone();

        // These clones are all cheap
        let template_context = self.state.template_context();
        let http_engine = self.http_engine.clone();
        let mut repository = self.state.repository.clone();
        let messages_tx = self.state.messages_tx.clone();

        // Launch the request in a separate task so it doesn't block
        tokio::spawn(async move {
            let result = try {
                // Build the request
                let request =
                    Request::build(&recipe, &template_context).await?;
                let request_id = request.id;

                // Pre-create the future because it needs a reference to the
                // request
                let future = http_engine.send(&request);
                repository.add_request(request)?;

                // Execute the request and store the response
                let response_result = future.await;
                repository.add_response(request_id, response_result)?;
            };
            // Report any errors back to the main thread
            if let Err(err) = result {
                messages_tx.send(Message::Error { error: err })
            }
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
    debug!("Restoring terminal");
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        std::io::stderr(),
        LeaveAlternateScreen,
        DisableMouseCapture,
    )?;
    Ok(())
}
