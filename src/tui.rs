pub mod context;
pub mod input;
pub mod message;
mod util;
pub mod view;

use crate::{
    collection::{Collection, CollectionFile, ProfileId, RecipeId},
    config::Config,
    db::Database,
    http::{Request, RequestBuilder},
    template::{Prompter, Template, TemplateChunk, TemplateContext},
    tui::{
        context::TuiContext,
        input::{Action, InputEngine},
        message::{Message, MessageSender, RequestConfig},
        util::{save_file, signals},
        view::{ModalPriority, PreviewPrompter, RequestState, View},
    },
    util::{Replaceable, ResultExt},
};
use anyhow::{anyhow, Context};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, EventStream},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::{Future, StreamExt};
use notify::{event::ModifyKind, RecursiveMode, Watcher};
use ratatui::{prelude::CrosstermBackend, Terminal};
use std::{
    io::{self, Stdout},
    ops::Deref,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver},
    time,
};
use tracing::{debug, error, info, trace};

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
    /// Replaceable allows us to enforce that the view is dropped before being
    /// recreated. The view persists its state on drop, so that has to happen
    /// before the new one is created.
    view: Replaceable<View>,
    collection_file: CollectionFile,
    should_run: bool,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

impl Tui {
    /// Rough **maximum** time for each iteration of the main loop
    const TICK_TIME: Duration = Duration::from_millis(250);

    /// Start the TUI. Any errors that occur during startup will be panics,
    /// because they prevent TUI execution.
    pub async fn start(collection_path: Option<PathBuf>) -> anyhow::Result<()> {
        initialize_panic_handler();
        let collection_path = CollectionFile::try_path(None, collection_path)?;

        // ===== Initialize global state =====
        // This stuff only needs to be set up *once per session*

        let config = Config::load()?;
        // Create a message queue for handling async tasks
        let (messages_tx, messages_rx) = mpsc::unbounded_channel();
        let messages_tx = MessageSender::new(messages_tx);
        // Load a database for this particular collection
        let database = Database::load()?.into_collection(&collection_path)?;
        // Initialize global view context
        TuiContext::init(config, database.clone());

        // ===== Initialize collection & view =====

        // If the collection fails to load, create an empty one just so we can
        // move along. We'll watch the file and hopefully the user can fix it
        let collection_file = CollectionFile::load(collection_path.clone())
            .await
            .reported(&messages_tx)
            .unwrap_or_else(|| CollectionFile::with_path(collection_path));
        let view = View::new(&collection_file, messages_tx.clone());

        // The code to revert the terminal takeover is in `Tui::drop`, so we
        // shouldn't take over the terminal until right before creating the
        // `Tui`.
        let terminal = initialize_terminal()?;

        let app = Tui {
            terminal,
            messages_rx,
            messages_tx,

            collection_file,
            should_run: true,

            view: Replaceable::new(view),
        };

        app.run().await
    }

    /// Run the main TUI update loop. Any error returned from this is fatal. See
    /// the struct definition for a description of the different phases of the
    /// run loop.
    async fn run(mut self) -> anyhow::Result<()> {
        // Spawn background tasks
        self.listen_for_signals();
        self.listen_for_input();
        // Hang onto this because it stops running when dropped
        let _watcher = self.watch_collection()?;

        // This loop is limited by the rate that messages come in, with a
        // minimum rate enforced by a timeout
        while self.should_run {
            // ===== Draw Phase =====
            // Draw *first* so initial UI state is rendered immediately
            self.terminal.draw(|f| self.view.draw(f))?;

            // ===== Message Phase =====
            // Grab one message out of the queue and handle it. This will block
            // while the queue is empty so we don't waste CPU cycles. The
            // timeout here makes sure we don't block forever, so things like
            // time displays during in-flight requests will update.
            let future =
                time::timeout(Self::TICK_TIME, self.messages_rx.recv());
            if let Ok(message) = future.await {
                // Error would indicate a very weird and fatal bug so we wanna
                // know about it
                let message =
                    message.expect("Message channel dropped while running");
                trace!(?message, "Handling message");
                // If an error occurs, store it so we can show the user
                if let Err(error) = self.handle_message(message) {
                    self.view.open_modal(error, ModalPriority::High);
                }
            }

            // ===== Event Phase =====
            // Let the view handle all queued events
            self.view.handle_events();
        }

        Ok(())
    }

    /// Handle an incoming message. Any error here will be displayed as a modal
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::CollectionStartReload => {
                let future = self.collection_file.reload();
                let messages_tx = self.messages_tx();
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
            Message::SaveFile { default_path, data } => {
                self.spawn(save_file(self.messages_tx(), default_path, data));
            }

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

            // Force quit short-circuits the view/message cycle, to make sure
            // it doesn't get ate by text boxes
            Message::Input {
                action: Some(Action::ForceQuit),
                ..
            } => self.quit(),
            Message::Input { event, action } => {
                self.view.handle_input(event, action);
            }

            Message::RequestLoad {
                profile_id,
                recipe_id,
            } => {
                self.load_request(profile_id.as_ref(), &recipe_id)?;
            }

            Message::Notify(message) => self.view.notify(message),
            Message::PromptStart(prompt) => {
                self.view.open_modal(prompt, ModalPriority::Low);
            }
            Message::ConfirmStart(confirm) => {
                self.view.open_modal(confirm, ModalPriority::Low);
            }

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

            Message::Quit => self.quit(),
        }
        Ok(())
    }

    /// Get a cheap clone of the message queue transmitter
    fn messages_tx(&self) -> MessageSender {
        self.messages_tx.clone()
    }

    /// Spawn a task to read input from the terminal. Each event is pushed
    /// into the message queue for handling by the main loop.
    fn listen_for_input(&self) {
        let messages_tx = self.messages_tx();

        tokio::spawn(async move {
            let mut stream = EventStream::new();
            while let Some(result) = stream.next().await {
                // Failure to read input is both weird and fatal, so panic
                let event = result.expect("Error reading terminal input");

                // Filter out junk events so we don't clog the message queue
                if InputEngine::should_kill(&event) {
                    continue;
                }

                let action = TuiContext::get().input_engine.action(&event);
                messages_tx.send(Message::Input { event, action });
            }
        });
    }

    /// Spawn a task to listen in the backgrouns for quit signals
    fn listen_for_signals(&self) {
        let messages_tx = self.messages_tx();
        self.spawn(async move {
            signals().await?;
            messages_tx.send(Message::Quit);
            Ok(())
        });
    }

    /// Spawn a watcher to automatically reload the collection when the file
    /// changes. Return the watcher because it stops when dropped.
    fn watch_collection(&self) -> anyhow::Result<impl Watcher> {
        // Spawn a watcher for the collection file
        let messages_tx = self.messages_tx();
        let f = move |result: notify::Result<_>| {
            match result {
                // Only reload if the file *content* changes
                Ok(
                    event @ notify::Event {
                        kind: notify::EventKind::Modify(ModifyKind::Data(_)),
                        ..
                    },
                ) => {
                    info!(?event, "Collection file changed, reloading");
                    messages_tx.send(Message::CollectionStartReload);
                }
                // Do nothing for other event kinds
                Ok(_) => {}
                Err(err) => {
                    error!(error = %err, "Error watching collection file");
                }
            }
        };
        let mut watcher = notify::recommended_watcher(f)?;
        watcher
            .watch(self.collection_file.path(), RecursiveMode::NonRecursive)?;
        info!(
            path = ?self.collection_file.path(), ?watcher,
            "Watching collection file for changes"
        );
        Ok(watcher)
    }

    /// Reload state with a new collection
    fn reload_collection(&mut self, collection: Collection) {
        self.collection_file.collection = collection;

        // Rebuild the whole view, because tons of things can change. Drop the
        // old one *first* to make sure UI state is saved before being restored
        let messages_tx = self.messages_tx();
        let collection_file = &self.collection_file;
        self.view.replace(move |old| {
            drop(old);
            View::new(collection_file, messages_tx)
        });
    }

    /// GOODBYE
    fn quit(&mut self) {
        info!("Initiating graceful shutdown");
        self.should_run = false;
    }

    /// Render URL for a request, then copy it to the clipboard
    fn copy_request_url(
        &self,
        request_config: RequestConfig,
    ) -> anyhow::Result<()> {
        let builder = self.get_request_builder(request_config.clone())?;
        let template_context =
            self.template_context(request_config.profile_id, true)?;
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
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
        let template_context =
            self.template_context(request_config.profile_id, true)?;
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        self.spawn(async move {
            let body = builder
                .build_body(&template_context)
                .await?
                .ok_or(anyhow!("Request has no body"))?;
            // Clone the bytes :(
            let body = String::from_utf8(body.into_bytes().into())
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
        let template_context =
            self.template_context(request_config.profile_id, true)?;
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
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

        let builder = self.get_request_builder(request_config.clone())?;

        let template_context =
            self.template_context(request_config.profile_id.clone(), true)?;
        let messages_tx = self.messages_tx();
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
            let context = TuiContext::get();

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
            let result = context.http_engine.clone().send(request).await;
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
        let database = &TuiContext::get().database;
        if let Some(record) =
            database.get_last_request(profile_id, recipe_id)?
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
        let messages_tx = self.messages_tx();
        tokio::spawn(async move { future.await.reported(&messages_tx) });
    }

    /// Expose app state to the templater. Most of the data has to be cloned out
    /// to be passed across async boundaries. This is annoying but in reality
    /// it should be small data.
    fn template_context(
        &self,
        profile_id: Option<ProfileId>,
        real_prompt: bool,
    ) -> anyhow::Result<TemplateContext> {
        let context = TuiContext::get();
        let prompter: Box<dyn Prompter> = if real_prompt {
            Box::new(self.messages_tx())
        } else {
            Box::new(PreviewPrompter)
        };
        let collection = &self.collection_file.collection;

        Ok(TemplateContext {
            selected_profile: profile_id,
            collection: collection.clone(),
            http_engine: Some(context.http_engine.clone()),
            database: context.database.clone(),
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
