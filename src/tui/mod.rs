mod input;
mod message;
mod view;

use crate::{
    config::{ProfileId, RequestCollection, RequestRecipeId},
    http::{HttpEngine, Repository},
    template::TemplateContext,
    tui::{
        input::InputEngine,
        message::{Message, MessageSender},
        view::View,
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
use indexmap::IndexMap;
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

/// Main controller struct for the TUI. The app uses a React-like architecture
/// for the view, with a wrapping controller (this struct). The main loop goes
/// through the following phases on each iteration:
///
/// - Input phase: Check for input from the user
/// - Message phase: Process any async messages from input or external sources
///   (HTTP, file system, etc.)
/// - Draw phase: Draw the entire UI
/// - Signal phase: Check for process signals that should trigger an exit
#[derive(Debug)]
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    messages_rx: UnboundedReceiver<Message>,
    messages_tx: MessageSender,
    http_engine: HttpEngine,
    input_engine: InputEngine,
    view: View,
    /// The file that the current collection was loaded from. Needed in order
    /// to reload from it
    collection_file: PathBuf,
    collection: RequestCollection,
    repository: Repository,
    should_run: bool,
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
        let messages_tx = MessageSender::new(messages_tx);

        let view = View::new(&collection, messages_tx.clone());
        let repository = Repository::load().unwrap();
        let app = Tui {
            terminal,
            messages_rx,
            messages_tx,
            http_engine: HttpEngine::new(repository.clone()),
            input_engine: InputEngine::new(),

            collection_file,
            collection,
            should_run: true,

            view,
            repository,
        };

        // Any error during execution that gets this far is fatal. We expect the
        // error to already have context attached so we can just unwrap
        app.run().unwrap();
    }

    /// Run the main TUI update loop. Any error returned from this is fatal. See
    /// the struct definition for a description of the different phases of the
    /// run loop.
    fn run(mut self) -> anyhow::Result<()> {
        // Listen for signals to stop the program
        let mut quit_signals = Signals::new([SIGHUP, SIGINT, SIGTERM, SIGQUIT])
            .context("Error creating signal handler")?;

        let mut last_tick = Instant::now();

        while self.should_run {
            // ===== Input Phase =====
            let timeout = Self::TICK_TIME
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            // This is where the tick rate is enforced
            if crossterm::event::poll(timeout)? {
                // Forward input to the view. Include the raw event for text
                // editors and such
                let event = crossterm::event::read()?;
                let action = self.input_engine.action(&event);
                self.view.handle_input(event, action);
            }
            if last_tick.elapsed() >= Self::TICK_TIME {
                last_tick = Instant::now();
            }

            // ===== Message Phase =====
            while let Ok(message) = self.messages_rx.try_recv() {
                // If an error occurs, store it so we can show the user
                self.handle_message(message)
                    .ok_or_apply(|err| self.view.set_error(err));
            }

            // ===== Draw Phase =====
            self.terminal
                .draw(|f| self.view.draw(&self.input_engine, f))?;

            // ===== Signal Phase =====
            if quit_signals.pending().next().is_some() {
                self.should_run = false;
            }
        }
        Ok(())
    }

    /// Handle an incoming message. Any error here will be displayed as a modal
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::CollectionStartReload => {
                let messages_tx = self.messages_tx.clone();
                let collection_file = self.collection_file.clone();
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
                self.reload_collection(collection_file, collection);
            }

            Message::HttpSendRequest {
                recipe_id,
                profile_id,
            } => self.send_request(recipe_id, profile_id)?,
            Message::HttpResponse { record } => {
                self.view.finish_request(record);
            }
            Message::HttpError { recipe_id, error } => {
                self.view.fail_request(recipe_id, error);
            }

            Message::RepositoryStartLoad { recipe_id } => {
                self.load_request(recipe_id);
            }
            Message::RepositoryEndLoad { record } => {
                self.view.load_request(record);
            }

            Message::PromptStart(prompt) => {
                self.view.set_prompt(prompt);
            }

            Message::Error { error } => self.view.set_error(error),
            Message::Quit => self.should_run = false,
        }
        Ok(())
    }

    /// Reload state with a new collection file
    fn reload_collection(
        &mut self,
        collection_file: PathBuf,
        collection: RequestCollection,
    ) {
        // TODO can we store these fields together in a wrapper struct?
        self.collection_file = collection_file;
        self.collection = collection;

        // Rebuild the whole view, because tons of things can change
        self.view = View::new(&self.collection, self.messages_tx.clone());
        self.view.notify(format!(
            "Reloaded collection from {}",
            self.collection_file.to_string_lossy()
        ));
    }

    /// Launch an HTTP request in a separate task
    fn send_request(
        &mut self,
        recipe_id: RequestRecipeId,
        profile_id: Option<ProfileId>,
    ) -> anyhow::Result<()> {
        let recipe = self
            .collection
            .requests
            .iter()
            .find(|recipe| recipe.id == recipe_id)
            .ok_or_else(|| anyhow!("No recipe with ID {recipe_id:?}"))?
            .clone();

        // Mark request state as loading
        self.view.start_request(recipe_id);

        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.
        let template_context = self.template_context(profile_id.as_ref())?;
        let http_engine = self.http_engine.clone();
        let messages_tx = self.messages_tx.clone();

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
        let messages_tx = self.messages_tx.clone();
        self.spawn(async move {
            if let Some(record) = repository.get_last(&recipe_id).await? {
                messages_tx.send(Message::RepositoryEndLoad { record });
            }
            Ok(())
        });
    }

    /// Helper for spawning a fallible task. Any error in the resolved future
    /// will be shown to the user in a modal.
    fn spawn(
        &self,
        future: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        let messages_tx = self.messages_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = future.await {
                messages_tx.send(Message::Error { error: err })
            }
        });
    }

    /// Expose app state to the templater. Most of the data has to be cloned out
    /// to be passed across async boundaries. This is annoying but in reality
    /// it should be small data.
    fn template_context(
        &self,
        profile_id: Option<&ProfileId>,
    ) -> anyhow::Result<TemplateContext> {
        // Find profile by ID
        let profile = match profile_id {
            Some(profile_id) => {
                let profile = self
                    .collection
                    .profiles
                    .iter()
                    .find(|profile| &profile.id == profile_id)
                    .ok_or_else(|| {
                        anyhow!("No profile with ID {profile_id:?}")
                    })?;
                profile.data.clone()
            }
            None => IndexMap::new(),
        };

        Ok(TemplateContext {
            profile,
            repository: self.repository.clone(),
            chains: self.collection.chains.clone(),
            overrides: Default::default(),
            prompter: Box::new(self.messages_tx.clone()),
        })
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
