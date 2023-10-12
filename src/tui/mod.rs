mod input;
mod state;
mod view;

use crate::{
    config::{RequestCollection, RequestRecipeId},
    http::{HttpEngine, Repository},
    template::TemplateContext,
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
use futures::Future;
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
use tokio::{
    sync::mpsc::{self, UnboundedReceiver},
    task,
};
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
    repository: Repository,
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
        let app = Tui {
            terminal,
            messages_rx,
            renderer: Renderer::new(),
            http_engine: HttpEngine::new(repository.clone()),
            state: AppState::new(collection_file, collection, messages_tx),
            repository,
        };

        // Any error during execution that gets this far is fatal. We expect the
        // error to already have context attached so we can just unwrap
        task::block_in_place(|| app.run().unwrap());
    }

    /// Run the main TUI update loop. Any error returned from this is fatal. See
    /// the struct definition for a description of the different phases of the
    /// run loop.
    fn run(mut self) -> anyhow::Result<()> {
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
                .draw(|f| self.renderer.draw_main(f, &self.state))?;

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
            Message::CollectionStartReload => {
                let messages_tx = self.state.messages_tx();
                let collection_file = self.state.collection_file().to_owned();
                self.spawn(async move {
                    let (_, collection) =
                        RequestCollection::load(Some(&collection_file)).await?;
                    messages_tx.send(Message::CollectionEndReload {
                        collection_file,
                        collection,
                    });
                    Ok(())
                });
            }
            Message::CollectionEndReload {
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

            Message::HttpSendRequest => {
                if self.state.can_send_request() {
                    self.send_request()?;
                }
            }
            Message::HttpResponse { record } => {
                self.state.finish_request(record);
            }
            Message::HttpError { recipe_id, error } => {
                self.state.fail_request(&recipe_id, error);
            }

            Message::RepositoryStartLoad { recipe_id } => {
                self.load_request(recipe_id);
            }
            Message::RepositoryEndLoad { record } => {
                self.state.load_request(record);
            }

            Message::Error { error } => self.state.set_error(error),
        }
        Ok(())
    }

    /// Launch an HTTP request in a separate task
    fn send_request(&mut self) -> anyhow::Result<()> {
        let recipe = self
            .state
            .recipes()
            .selected()
            .ok_or_else(|| anyhow!("No recipe selected"))?
            .clone();

        // Mark request state as loading
        self.state.start_request(recipe.id.clone());

        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.
        let template_context = self.template_context();
        let http_engine = self.http_engine.clone();
        let messages_tx = self.state.messages_tx();

        // We can't use self.spawn here because HTTP errors are handled
        // differently from all other error types
        tokio::spawn(async move {
            let result: anyhow::Result<()> = try {
                // Build the request
                let request =
                    HttpEngine::build_request(&recipe, &template_context)
                        .await?;

                // Send the request
                let record = http_engine.send(request).await?;
                messages_tx.send(Message::HttpResponse { record });
            };

            // Report any errors back to the main thread
            if let Err(err) = result {
                messages_tx.send(Message::HttpError {
                    recipe_id: recipe.id,
                    error: err,
                })
            }
        });

        Ok(())
    }

    /// Load the most recent request+response for a particular recipe from the
    /// repository, and store it in state.
    fn load_request(&self, recipe_id: RequestRecipeId) {
        let repository = self.repository.clone();
        let messages_tx = self.state.messages_tx();
        self.spawn(async move {
            if let Some(record) = repository.get_last(&recipe_id).await? {
                messages_tx.send(Message::RepositoryEndLoad { record });
            }
            Ok(())
        });
    }

    /// Helper for spawning a fallible task. Any error in the resolved future
    /// will be shown to the user in a popup.
    fn spawn(
        &self,
        future: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        let messages_tx = self.state.messages_tx();
        tokio::spawn(async move {
            if let Err(err) = future.await {
                messages_tx.send(Message::Error { error: err })
            }
        });
    }

    /// Expose app state to the templater. Most of the data has to be cloned out
    /// to be passed across async boundaries. This is annoying but in reality
    /// it should be small data.
    fn template_context(&self) -> TemplateContext {
        TemplateContext {
            profile: self
                .state
                .profiles()
                .selected()
                .map(|e| e.data.clone())
                .unwrap_or_default(),
            repository: self.repository.clone(),
            chains: self.state.chains().to_owned(),
            overrides: Default::default(),
        }
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
