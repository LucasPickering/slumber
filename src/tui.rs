mod input;
mod message;
mod view;

use crate::{
    collection::{ProfileId, RequestCollection, RequestRecipeId},
    http::{HttpEngine, Repository, RequestBuilder},
    template::{Prompter, Template, TemplateChunk, TemplateContext},
    tui::{
        input::{Action, InputEngine},
        message::{Message, MessageSender},
        view::{ModalPriority, PreviewPrompter, RequestState, View},
    },
};
use anyhow::{anyhow, Context};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use futures::Future;
use indexmap::IndexMap;
use notify::{RecursiveMode, Watcher};
use ratatui::{prelude::CrosstermBackend, Terminal};
use signal_hook::{
    consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM},
    iterator::Signals,
};
use std::{
    io::{self, Stdout},
    ops::Deref,
    path::PathBuf,
    sync::{Arc, OnceLock},
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
    terminal: Term,
    messages_rx: UnboundedReceiver<Message>,
    messages_tx: MessageSender,
    http_engine: HttpEngine,
    input_engine: InputEngine,
    view: View,
    collection: RequestCollection,
    repository: Repository,
    should_run: bool,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

impl Tui {
    /// Rough maximum time for each iteration of the main loop
    const TICK_TIME: Duration = Duration::from_millis(250);

    /// Start the TUI. Any errors that occur during startup will be panics,
    /// because they prevent TUI execution.
    pub async fn start(collection_file: PathBuf) {
        initialize_panic_handler();
        let terminal =
            initialize_terminal().expect("Error initializing terminal");

        // Create a message queue for handling async tasks
        let (messages_tx, messages_rx) = mpsc::unbounded_channel();
        let messages_tx = MessageSender::new(messages_tx);

        // If the collection fails to load, create an empty one just so we can
        // move along. We'll watch the file and hopefully the user can fix it
        let collection = RequestCollection::load(collection_file.clone())
            .await
            .unwrap_or_else(|error| {
                messages_tx.send(Message::Error { error });
                RequestCollection::<()>::default().with_source(collection_file)
            });

        let view = View::new(&collection, messages_tx.clone());
        let repository = Repository::load(&collection.id).unwrap();
        let app = Tui {
            terminal,
            messages_rx,
            messages_tx,
            http_engine: HttpEngine::new(repository.clone()),
            input_engine: InputEngine::new(),

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

        // Hang onto this because it stops running when dropped
        let _watcher = self.watch_collection()?;

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
                if let Some(Action::ForceQuit) = action {
                    // Short-circuit the view/message cycle, to make sure this
                    // doesn't get ate
                    self.quit();
                } else {
                    self.view.handle_input(event, action);
                }
            }
            if last_tick.elapsed() >= Self::TICK_TIME {
                last_tick = Instant::now();
            }

            // ===== Message Phase =====
            while let Ok(message) = self.messages_rx.try_recv() {
                // If an error occurs, store it so we can show the user
                if let Err(error) = self.handle_message(message) {
                    self.view.open_modal(error, ModalPriority::High);
                }
            }

            // ===== Draw Phase =====
            self.terminal.draw(|f| {
                self.view
                    .draw(&self.input_engine, self.messages_tx.clone(), f)
            })?;

            // ===== Signal Phase =====
            if quit_signals.pending().next().is_some() {
                self.quit();
            }
        }

        Ok(())
    }

    /// GOODBYE
    fn quit(&mut self) {
        self.should_run = false;
    }

    /// Handle an incoming message. Any error here will be displayed as a modal
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::CollectionStartReload => {
                let messages_tx = self.messages_tx.clone();
                let future = self.collection.reload();
                self.spawn(async move {
                    let collection = future.await?;
                    messages_tx.send(Message::CollectionEndReload(collection));
                    Ok(())
                });
            }
            Message::CollectionEndReload(collection) => {
                self.reload_collection(collection);
            }

            Message::Error { error } => {
                self.view.open_modal(error, ModalPriority::High)
            }

            // Manage HTTP life cycle
            Message::HttpBeginRequest {
                recipe_id,
                profile_id,
            } => self.send_request(recipe_id, profile_id.as_ref())?,
            Message::HttpBuildError { recipe_id, error } => {
                self.view.set_request_state(
                    recipe_id,
                    RequestState::BuildError { error },
                );
            }
            Message::HttpLoading {
                recipe_id,
                request_id,
            } => {
                self.view.set_request_state(
                    recipe_id,
                    RequestState::loading(request_id),
                );
            }
            Message::HttpComplete(result) => {
                let (recipe_id, state) = match result {
                    Ok(record) => (
                        record.request.recipe_id.clone(),
                        RequestState::response(record),
                    ),
                    Err(error) => (
                        error.request.recipe_id.clone(),
                        RequestState::RequestError { error },
                    ),
                };
                self.view.set_request_state(recipe_id, state);
            }

            Message::RepositoryStartLoad { recipe_id } => {
                self.load_request(recipe_id);
            }
            Message::RepositoryEndLoad { record } => {
                self.view.set_request_state(
                    record.request.recipe_id.clone(),
                    RequestState::response(record),
                );
            }

            Message::PromptStart(prompt) => {
                self.view.open_modal(prompt, ModalPriority::Low);
            }

            Message::Quit => self.quit(),

            Message::TemplatePreview {
                template,
                profile_id,
                destination,
            } => {
                self.render_template_preview(
                    template,
                    profile_id.as_ref(),
                    destination,
                )?;
            }

            Message::ToggleMouseCapture { capture } => {
                let mut stdout = io::stdout();
                if capture {
                    stdout.execute(EnableMouseCapture)?;
                } else {
                    stdout.execute(DisableMouseCapture)?;
                }
            }
        }
        Ok(())
    }

    /// Spawn a watcher to automatically reload the collection when the file
    /// changes. Return the watcher because it stops when dropped.
    fn watch_collection(&self) -> anyhow::Result<impl Watcher> {
        // Spawn a watcher for the collection file
        let messages_tx = self.messages_tx.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<_>| {
                match result {
                    Ok(_) => messages_tx.send(Message::CollectionStartReload),
                    Err(err) => {
                        error!(error = %err, "Error watching collection file");
                    }
                }
            })?;
        watcher.watch(self.collection.path(), RecursiveMode::NonRecursive)?;
        Ok(watcher)
    }

    /// Reload state with a new collection
    fn reload_collection(&mut self, collection: RequestCollection) {
        self.collection = collection;

        // Rebuild the whole view, because tons of things can change
        self.view = View::new(&self.collection, self.messages_tx.clone());
        self.view.notify(format!(
            "Reloaded collection from {}",
            self.collection.path().to_string_lossy()
        ));
    }

    /// Launch an HTTP request in a separate task
    fn send_request(
        &mut self,
        recipe_id: RequestRecipeId,
        profile_id: Option<&ProfileId>,
    ) -> anyhow::Result<()> {
        let recipe = self
            .collection
            .recipes
            .iter()
            .find(|recipe| recipe.id == recipe_id)
            .ok_or_else(|| anyhow!("No recipe with ID `{recipe_id}`"))?
            .clone();

        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.
        let template_context =
            self.template_context(profile_id, self.messages_tx.clone())?;
        let http_engine = self.http_engine.clone();
        let builder = RequestBuilder::new(recipe, template_context);
        let messages_tx = self.messages_tx.clone();

        // Mark request state as building
        let request_id = builder.id();
        self.view.set_request_state(
            recipe_id.clone(),
            RequestState::building(request_id),
        );

        // We can't use self.spawn here because HTTP errors are handled
        // differently from all other error types
        tokio::spawn(async move {
            // Build the request
            let request = builder.build().await.map_err(|error| {
                // Report the error, but don't actually return anything
                messages_tx.send(Message::HttpBuildError {
                    recipe_id: recipe_id.clone(),
                    error,
                });
            })?;

            // Report liftoff
            messages_tx.send(Message::HttpLoading {
                recipe_id,
                request_id,
            });

            // Send the request and report the result to the main thread
            let result = http_engine.send(request).await;
            messages_tx.send(Message::HttpComplete(result));

            // By returning an empty result, we can use `?` to break out early.
            // `return` and `break` don't work in an async block :/
            Ok::<(), ()>(())
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

    /// Spawn a task to render a template, storing the result in a pre-defined
    /// lock. As this is a preview, the user will *not* be prompted for any
    /// input. A placeholder value will be used for any prompts.
    fn render_template_preview(
        &self,
        template: Template,
        profile_id: Option<&ProfileId>,
        destination: Arc<OnceLock<Vec<TemplateChunk>>>,
    ) -> anyhow::Result<()> {
        let context = self.template_context(profile_id, PreviewPrompter)?;
        self.spawn(async move {
            // Render chunks, then write them to the output destination
            let chunks = template.render_chunks(&context).await;
            // If this fails, it's a logic error somewhere. Only one task should
            // exist per lock
            destination.set(chunks).map_err(|_| {
                anyhow!("Multiple writes to template preview lock")
            })
        });
        Ok(())
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
    ///
    /// Fails if the given profile ID doesn't match any profiles
    fn template_context(
        &self,
        profile_id: Option<&ProfileId>,
        prompter: impl 'static + Prompter,
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
                        anyhow!("No profile with ID `{profile_id}`")
                    })?;
                profile.data.clone()
            }
            None => IndexMap::new(),
        };

        Ok(TemplateContext {
            profile,
            repository: self.repository.clone(),
            chains: self.collection.chains.to_owned(),
            overrides: Default::default(),
            prompter: Box::new(prompter),
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

/// Set up terminal for TUI
fn initialize_terminal() -> anyhow::Result<Term> {
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
