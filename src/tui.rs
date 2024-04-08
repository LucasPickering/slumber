mod context;
pub mod input;
mod message;
mod view;

use crate::{
    collection::{Collection, CollectionFile, ProfileId, RecipeId},
    config::Config,
    db::{CollectionDatabase, Database},
    http::{HttpEngine, Request, RequestBuilder},
    template::{Prompter, Template, TemplateChunk, TemplateContext},
    tui::{
        context::TuiContext,
        input::Action,
        message::{Message, MessageSender, RequestConfig},
        view::{ModalPriority, PreviewPrompter, RequestState, View},
    },
    util::Replaceable,
};
use anyhow::{anyhow, Context};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::Future;
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
    /// Replaceable allows us to enforce that the view is dropped before being
    /// recreated. The view persists its state on drop, so that has to happen
    /// before the new one is created.
    view: Replaceable<View>,
    collection_file: CollectionFile,
    /// We only ever need to run DB ops related to our collection, so we can
    /// use a collection-restricted DB handle
    database: CollectionDatabase,
    should_run: bool,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

impl Tui {
    /// Rough maximum time for each iteration of the main loop
    const TICK_TIME: Duration = Duration::from_millis(250);

    /// Start the TUI. Any errors that occur during startup will be panics,
    /// because they prevent TUI execution.
    pub async fn start(collection_path: Option<PathBuf>) -> anyhow::Result<()> {
        initialize_panic_handler();
        let collection_path = CollectionFile::try_path(collection_path)?;

        // ===== Initialize global state =====
        // This stuff only needs to be set up *once per session*

        let config = Config::load()?;
        // Create a message queue for handling async tasks
        let (messages_tx, messages_rx) = mpsc::unbounded_channel();
        let messages_tx = MessageSender::new(messages_tx);
        // Load a database for this particular collection
        let database = Database::load()?.into_collection(&collection_path)?;
        let http_engine = HttpEngine::new(&config, database.clone());
        // Initialize global view context
        TuiContext::init(config, messages_tx.clone(), database.clone());

        // ===== Initialize collection & view =====

        // If the collection fails to load, create an empty one just so we can
        // move along. We'll watch the file and hopefully the user can fix it
        let collection_file = CollectionFile::load(collection_path.clone())
            .await
            .unwrap_or_else(|error| {
                messages_tx.send(Message::Error { error });
                CollectionFile::with_path(collection_path)
            });
        let view = View::new(&collection_file.collection);

        // The code to revert the terminal takeover is in `Tui::drop`, so we
        // shouldn't take over the terminal until right before creating the
        // `Tui`.
        let terminal = initialize_terminal()?;

        let app = Tui {
            terminal,
            messages_rx,
            messages_tx,
            http_engine,

            collection_file,
            should_run: true,

            view: Replaceable::new(view),
            database,
        };

        app.run()
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
                let action = TuiContext::get().input_engine.action(&event);
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
            self.terminal.draw(|f| self.view.draw(f))?;

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
                let future = self.collection_file.reload();
                self.spawn(async move {
                    let collection = future.await?;
                    messages_tx.send(Message::CollectionEndReload(collection));
                    Ok(())
                });
            }
            Message::CollectionEndReload(collection) => {
                self.reload_collection(collection);
            }
            Message::CollectionEdit => {
                let path = self.collection_file.path();
                open::that_detached(path).context("Error opening {path:?}")?;
            }

            Message::CopyRequestUrl(request_config) => {
                self.copy_request_url(request_config)?;
            }
            Message::CopyRequestBody(request_config) => {
                self.copy_request_body(request_config)?;
            }
            Message::CopyRequestCurl(request_config) => {
                self.copy_request_curl(request_config)?;
            }
            Message::CopyText(text) => self.view.copy_text(text),

            Message::Error { error } => {
                self.view.open_modal(error, ModalPriority::High)
            }

            // Manage HTTP life cycle
            Message::HttpBeginRequest(request_config) => {
                self.send_request(request_config)?
            }
            Message::HttpBuildError {
                profile_id,
                recipe_id,
                error,
            } => {
                self.view.set_request_state(
                    profile_id,
                    recipe_id,
                    RequestState::BuildError { error },
                );
            }
            Message::HttpLoading {
                profile_id,
                recipe_id,
                request,
            } => {
                self.view.set_request_state(
                    profile_id,
                    recipe_id,
                    RequestState::loading(request),
                );
            }
            Message::HttpComplete(result) => {
                let (profile_id, recipe_id, state) = match result {
                    Ok(record) => (
                        record.request.profile_id.clone(),
                        record.request.recipe_id.clone(),
                        RequestState::response(record),
                    ),
                    Err(error) => (
                        error.request.profile_id.clone(),
                        error.request.recipe_id.clone(),
                        RequestState::RequestError { error },
                    ),
                };
                self.view.set_request_state(profile_id, recipe_id, state);
            }

            Message::RequestLoad {
                profile_id,
                recipe_id,
            } => {
                self.load_request(profile_id.as_ref(), &recipe_id)?;
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
                    profile_id,
                    destination,
                )?;
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
        watcher
            .watch(self.collection_file.path(), RecursiveMode::NonRecursive)?;
        Ok(watcher)
    }

    /// Reload state with a new collection
    fn reload_collection(&mut self, collection: Collection) {
        self.collection_file.collection = collection;

        // Rebuild the whole view, because tons of things can change. Drop the
        // old one *first* to make sure UI state is saved before being restored
        self.view.replace(|old| {
            drop(old);
            View::new(&self.collection_file.collection)
        });
        self.view.notify(format!(
            "Reloaded collection from {}",
            self.collection_file.path().to_string_lossy()
        ));
    }

    /// Render URL for a request, then copy it to the clipboard
    fn copy_request_url(
        &self,
        request_config: RequestConfig,
    ) -> anyhow::Result<()> {
        let builder = self.get_request_builder(request_config.clone())?;
        let messages_tx = self.messages_tx.clone();
        // Spawn a task to do the render+copy
        let template_context =
            self.template_context(request_config.profile_id, true)?;
        self.spawn(async move {
            let url = builder.build_url(&template_context).await?;
            messages_tx.send(Message::CopyText(url.to_string()));
            Ok(())
        });
        Ok(())
    }

    /// Render body for a request, then copy it to the clipboard
    fn copy_request_body(
        &self,
        request_config: RequestConfig,
    ) -> anyhow::Result<()> {
        let builder = self.get_request_builder(request_config.clone())?;
        let messages_tx = self.messages_tx.clone();
        // Spawn a task to do the render+copy
        let template_context =
            self.template_context(request_config.profile_id, true)?;
        self.spawn(async move {
            let body = builder
                .build_body(&template_context)
                .await?
                .ok_or(anyhow!("Request has no body"))?;
            let body = String::from_utf8(body.into())
                .context("Cannot copy request body")?;
            messages_tx.send(Message::CopyText(body));
            Ok(())
        });
        Ok(())
    }

    /// Render a request, then copy the equivalent curl command to the clipboard
    fn copy_request_curl(
        &self,
        request_config: RequestConfig,
    ) -> anyhow::Result<()> {
        let builder = self.get_request_builder(request_config.clone())?;
        let messages_tx = self.messages_tx.clone();
        // Spawn a task to do the render+copy
        let template_context =
            self.template_context(request_config.profile_id, true)?;
        self.spawn(async move {
            let request = builder.build(&template_context).await?;
            let command = request.to_curl()?;
            messages_tx.send(Message::CopyText(command));
            Ok(())
        });
        Ok(())
    }

    /// Launch an HTTP request in a separate task
    fn send_request(
        &mut self,
        request_config: RequestConfig,
    ) -> anyhow::Result<()> {
        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.

        let http_engine = self.http_engine.clone();
        let builder = self.get_request_builder(request_config.clone())?;
        let messages_tx = self.messages_tx.clone();

        let template_context =
            self.template_context(request_config.profile_id.clone(), true)?;
        let RequestConfig {
            profile_id,
            recipe_id,
            ..
        } = request_config;

        // Mark request state as building
        let request_id = builder.id();
        self.view.set_request_state(
            profile_id.clone(),
            recipe_id.clone(),
            RequestState::building(request_id),
        );

        // We can't use self.spawn here because HTTP errors are handled
        // differently from all other error types
        tokio::spawn(async move {
            // Build the request
            let request: Arc<Request> = builder
                .build(&template_context)
                .await
                .map_err(|error| {
                    // Report the error, but don't actually return anything
                    messages_tx.send(Message::HttpBuildError {
                        profile_id: profile_id.clone(),
                        recipe_id: recipe_id.clone(),
                        error,
                    });
                })?
                .into();

            // Report liftoff
            messages_tx.send(Message::HttpLoading {
                profile_id,
                recipe_id,
                request: Arc::clone(&request),
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
    /// database, and store it in state.
    fn load_request(
        &mut self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> anyhow::Result<()> {
        if let Some(record) =
            self.database.get_last_request(profile_id, recipe_id)?
        {
            self.view.set_request_state(
                profile_id.cloned(),
                record.request.recipe_id.clone(),
                RequestState::response(record),
            );
        }
        Ok(())
    }

    /// Helper to create a [RequestBuilder] based on request parameters
    fn get_request_builder(
        &self,

        RequestConfig {
            recipe_id, options, ..
        }: RequestConfig,
    ) -> anyhow::Result<RequestBuilder> {
        let recipe = self
            .collection_file
            .collection
            .recipes
            .get_recipe(&recipe_id)
            .ok_or_else(|| anyhow!("No recipe with ID `{recipe_id}`"))?
            .clone();
        Ok(RequestBuilder::new(recipe, options))
    }

    /// Spawn a task to render a template, storing the result in a pre-defined
    /// lock. As this is a preview, the user will *not* be prompted for any
    /// input. A placeholder value will be used for any prompts.
    fn render_template_preview(
        &self,
        template: Template,
        profile_id: Option<ProfileId>,
        destination: Arc<OnceLock<Vec<TemplateChunk>>>,
    ) -> anyhow::Result<()> {
        let context = self.template_context(profile_id, false)?;
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
    fn template_context(
        &self,
        profile_id: Option<ProfileId>,
        real_prompt: bool,
    ) -> anyhow::Result<TemplateContext> {
        let prompter: Box<dyn Prompter> = if real_prompt {
            Box::new(self.messages_tx.clone())
        } else {
            Box::new(PreviewPrompter)
        };
        let collection = &self.collection_file.collection;

        Ok(TemplateContext {
            selected_profile: profile_id,
            collection: collection.clone(),
            http_engine: Some(self.http_engine.clone()),
            database: self.database.clone(),
            overrides: Default::default(),
            prompter,
            recursion_count: Default::default(),
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
