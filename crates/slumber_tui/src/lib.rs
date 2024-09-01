#![forbid(unsafe_code)]
#![deny(clippy::all)]

//! Terminal user interface for Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod context;
mod input;
mod message;
#[cfg(test)]
mod test_util;
mod util;
mod view;

use crate::{
    context::TuiContext,
    message::{Message, MessageSender, RequestConfig},
    util::{
        clear_event_buffer, get_editor_command, save_file, signals,
        ResultReported,
    },
    view::{PreviewPrompter, RequestState, View},
};
use anyhow::{anyhow, Context};
use chrono::Utc;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, EventStream,
    },
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use notify::{event::ModifyKind, RecursiveMode, Watcher};
use ratatui::{prelude::CrosstermBackend, Terminal};
use slumber_config::{Action, Config};
use slumber_core::{
    collection::{Collection, CollectionFile, ProfileId},
    db::{CollectionDatabase, Database},
    http::RequestSeed,
    template::{Prompter, Template, TemplateChunk, TemplateContext},
};
use std::{
    future::Future,
    io::{self, Stdout},
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    select,
    sync::{
        mpsc::{self, UnboundedReceiver},
        Semaphore,
    },
    time,
};
use tracing::{debug, error, info, trace};

/// Main controller struct for the TUI. The app uses a React-ish architecture
/// for the view, with a wrapping controller (this struct)
#[derive(Debug)]
pub struct Tui {
    terminal: Term,
    /// Persistence database, for storing request state, UI state, etc.
    database: CollectionDatabase,
    /// Receiver for the async message queue, which allows background tasks and
    /// the view to pass data and trigger side effects. Nobody else gets to
    /// touch this
    messages_rx: UnboundedReceiver<Message>,
    /// Transmitter for the async message queue, which can be freely cloned and
    /// passed around
    messages_tx: MessageSender,
    view: View,
    collection_file: CollectionFile,
    should_run: bool,
    /// Each active HTTP request should grab one permit from the semaphore. The
    /// primary purpose of this is to track whether any requests are in-flight.
    ///
    /// This is probably overkill because we could just use an `AtomicU8`, but
    /// it simplifies the semantics of incrementing/decrementing correctly.
    http_semaphore: Arc<Semaphore>,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

impl Tui {
    /// Rough **maximum** time for each iteration of the main loop
    const TICK_TIME: Duration = Duration::from_millis(250);
    /// Maximum number of concurrent HTTP requests. This limit is fairly
    /// arbitrary; in practice we don't ever expect to hit it.
    const MAX_HTTP_REQUESTS: usize = 100;

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
        TuiContext::init(config);

        // ===== Initialize collection & view =====

        // If the collection fails to load, create an empty one just so we can
        // move along. We'll watch the file and hopefully the user can fix it
        let collection_file = CollectionFile::load(collection_path.clone())
            .await
            .reported(&messages_tx)
            .unwrap_or_else(|| CollectionFile::with_path(collection_path));
        let view =
            View::new(&collection_file, database.clone(), messages_tx.clone());

        // The code to revert the terminal takeover is in `Tui::drop`, so we
        // shouldn't take over the terminal until right before creating the
        // `Tui`.
        let terminal = initialize_terminal()?;

        let app = Tui {
            terminal,
            database,
            messages_rx,
            messages_tx,

            collection_file,
            should_run: true,

            view,
            http_semaphore: Semaphore::new(Self::MAX_HTTP_REQUESTS).into(),
        };

        app.run().await
    }

    /// Run the main TUI update loop. Any error returned from this is fatal. See
    /// the struct definition for a description of the different phases of the
    /// run loop.
    async fn run(mut self) -> anyhow::Result<()> {
        // Spawn background tasks
        self.listen_for_signals();
        // Hang onto this because it stops running when dropped
        let _watcher = self.watch_collection()?;

        let input_engine = &TuiContext::get().input_engine;
        // Stream of terminal input events
        let mut input_stream = EventStream::new();

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
                event_result = input_stream.next() => {
                    if let Some(event) = event_result {
                        let event = event.expect("Error reading terminal input");
                        input_engine.event_to_message(event)
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
                _ = time::sleep(Self::TICK_TIME) => None,
            };

            // We'll try to skip draws if nothing on the screen has changed, to
            // limit idle CPU usage. If a request is running we always need to
            // update though, because the timer will be ticking.
            let mut needs_draw = self.has_active_requests();

            if let Some(message) = message {
                trace!(?message, "Handling message");
                // If an error occurs, store it so we can show the user
                if let Err(error) = self.handle_message(message) {
                    self.view.open_modal(error);
                }
                needs_draw = true;
            };

            // ===== Event Phase =====
            // Let the view handle all queued events. Trigger a draw if there
            // was anything in the queue.
            needs_draw |= self.view.handle_events();

            // ===== Draw Phase =====
            // Only draw if something's changed. Skip draws if we have more
            // messages to process. This prevents us from falling behind the
            // message queue, which can happen if the UI gets slow. Interactions
            // will start to look jittery, but it's better than locking the user
            // out. We know the next loop iteration will also have a message to
            // handle, so we don't have to worry about losing the draw entirely.
            let has_more_messages = !self.messages_rx.is_empty()
                || event::poll(Duration::from_millis(0)).unwrap_or(false);
            if needs_draw && !has_more_messages {
                self.draw()?;
            }
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
                self.reload_collection(collection)
            }
            Message::CollectionEdit => {
                let path = self.collection_file.path().to_owned();
                self.edit_file(&path)?
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

            Message::EditFile { path, on_complete } => {
                self.edit_file(&path)?;
                on_complete(path);
            }

            Message::Error { error } => self.view.open_modal(error),

            // Manage HTTP life cycle
            Message::HttpBeginRequest(request_config) => {
                self.send_request(request_config)?
            }
            Message::HttpBuildError { error } => {
                self.view
                    .set_request_state(RequestState::BuildError { error });
            }
            Message::HttpLoading { request } => {
                self.view.set_request_state(RequestState::loading(request))
            }
            Message::HttpComplete(result) => {
                let state = match result {
                    Ok(exchange) => RequestState::response(exchange),
                    Err(error) => RequestState::RequestError { error },
                };
                self.view.set_request_state(state);
            }

            // Force quit short-circuits the view/message cycle, to make sure
            // it doesn't get ate by text boxes
            Message::Input {
                action: Some(Action::ForceQuit),
                ..
            } => self.quit(),
            Message::Input {
                event: Event::Resize(_, _),
                ..
            } => {
                // Immediately redraw the screen
                self.draw()?;
            }
            Message::Input { event, action } => {
                self.view.handle_input(event, action);
            }

            Message::Notify(message) => self.view.notify(message),
            Message::PromptStart(prompt) => {
                self.view.open_modal(prompt);
            }
            Message::SelectStart(select) => {
                self.view.open_modal(select);
            }
            Message::ConfirmStart(confirm) => {
                self.view.open_modal(confirm);
            }

            Message::TemplatePreview {
                template,
                on_complete,
            } => {
                self.render_template_preview(
                    template,
                    // Note: there's a potential bug here, if the selected
                    // profile changed since this message was queued. In
                    // practice is extremely unlikely (potentially impossible),
                    // and this shortcut saves us a lot of plumbing so it's
                    // worth it
                    self.view.selected_profile_id().cloned(),
                    on_complete,
                )?;
            }
            // This message exists just to trigger a draw
            Message::TemplatePreviewComplete => {}

            Message::Quit => self.quit(),
        }
        Ok(())
    }

    fn has_active_requests(&self) -> bool {
        self.http_semaphore.available_permits() < Self::MAX_HTTP_REQUESTS
    }

    /// Get a cheap clone of the message queue transmitter
    fn messages_tx(&self) -> MessageSender {
        self.messages_tx.clone()
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
        self.collection_file.collection = collection.into();

        // Rebuild the whole view, because tons of things can change
        let database = self.database.clone();
        let messages_tx = self.messages_tx();
        let collection_file = &self.collection_file;
        self.view = View::new(collection_file, database, messages_tx);
    }

    /// GOODBYE
    fn quit(&mut self) {
        info!("Initiating graceful shutdown");
        self.should_run = false;
    }

    /// Draw the view onto the screen
    fn draw(&mut self) -> anyhow::Result<()> {
        self.terminal.draw(|frame| self.view.draw(frame))?;
        Ok(())
    }

    /// Open a file in the user's configured editor. **This will  block the main
    /// thread**, because we assume we're opening a terminal editor and
    /// therefore should yield the terminal to the editor.
    fn edit_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let mut command = get_editor_command(path)?;
        let error_context =
            format!("Error spawning editor with command `{command:?}`");

        // Block while the editor runs. Useful for terminal editors since
        // they'll take over the whole screen. Potentially annoying for GUI
        // editors that block, but we'll just hope the command is
        // fire-and-forget. If this becomes an issue we can try to detect if the
        // subprocess took over the terminal and cut it loose if not, or add a
        // config field for it.
        self.terminal.draw(|frame| {
            frame.render_widget("Waiting for editor to close...", frame.area());
        })?;

        let mut stdout = io::stdout();
        crossterm::execute!(stdout, LeaveAlternateScreen)?;
        command.status().context(error_context)?;
        // Some editors *cough* vim *cough* dump garbage to the event buffer on
        // exit. I've never figured out what actually causes it, but a simple
        // solution is to dump all events in the buffer before returning to
        // Slumber. It's possible we lose some real user input here (e.g. if
        // other events were queued behind the event to open the editor).
        clear_event_buffer();
        crossterm::execute!(stdout, EnterAlternateScreen)?;

        // Redraw immediately. The main loop will probably be in the tick
        // timeout when we go back to it, so that adds a 250ms delay to
        // redrawing the screen that we want to skip.
        self.draw()?;

        Ok(())
    }

    /// Render URL for a request, then copy it to the clipboard
    fn copy_request_url(
        &self,
        RequestConfig {
            profile_id,
            recipe_id,
            options,
        }: RequestConfig,
    ) -> anyhow::Result<()> {
        let seed = RequestSeed::new(recipe_id, options);
        let template_context = self.template_context(profile_id, false)?;
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        self.spawn(async move {
            let url = TuiContext::get()
                .http_engine
                .build_url(seed, &template_context)
                .await?;
            messages_tx.send(Message::CopyText(url.to_string()));
            Ok(())
        });
        Ok(())
    }

    /// Render body for a request, then copy it to the clipboard
    fn copy_request_body(
        &self,
        RequestConfig {
            profile_id,
            recipe_id,
            options,
        }: RequestConfig,
    ) -> anyhow::Result<()> {
        let seed = RequestSeed::new(recipe_id, options);
        let template_context = self.template_context(profile_id, false)?;
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        self.spawn(async move {
            let body = TuiContext::get()
                .http_engine
                .build_body(seed, &template_context)
                .await?
                .ok_or(anyhow!("Request has no body"))?;
            // Clone the bytes :(
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
        RequestConfig {
            profile_id,
            recipe_id,
            options,
        }: RequestConfig,
    ) -> anyhow::Result<()> {
        let seed = RequestSeed::new(recipe_id, options);
        let template_context = self.template_context(profile_id, false)?;
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        self.spawn(async move {
            let ticket = TuiContext::get()
                .http_engine
                .build(seed, &template_context)
                .await?;
            let command = ticket.record().to_curl()?;
            messages_tx.send(Message::CopyText(command));
            Ok(())
        });
        Ok(())
    }

    /// Launch an HTTP request in a separate task
    fn send_request(
        &mut self,
        RequestConfig {
            profile_id,
            recipe_id,
            options,
        }: RequestConfig,
    ) -> anyhow::Result<()> {
        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.

        let template_context =
            self.template_context(profile_id.clone(), false)?;
        let messages_tx = self.messages_tx();

        // Mark request state as building
        let initialized = RequestSeed::new(recipe_id.clone(), options);
        self.view.set_request_state(RequestState::Building {
            id: initialized.id,
            start_time: Utc::now(),
            profile_id,
            recipe_id,
        });

        // We can't use self.spawn here because HTTP errors are handled
        // differently from all other error types
        let database = self.database.clone();
        let semaphore = Arc::clone(&self.http_semaphore);
        tokio::spawn(async move {
            // Track this request so the main loop knows something is in flight
            let permit =
                semaphore.acquire().await.expect("HTTP semaphore closed");
            // Build the request
            let ticket = TuiContext::get()
                .http_engine
                .build(initialized, &template_context)
                .await
                .map_err(|error| {
                    // Report the error, but don't actually return anything
                    messages_tx.send(Message::HttpBuildError { error });
                })?;

            // Report liftoff
            messages_tx.send(Message::HttpLoading {
                request: Arc::clone(ticket.record()),
            });

            // Send the request and report the result to the main thread
            let result = ticket.send(&database).await;
            messages_tx.send(Message::HttpComplete(result));

            drop(permit);
            // By returning an empty result, we can use `?` to break out early.
            // `return` and `break` don't work in an async block :/
            Ok::<(), ()>(())
        });

        Ok(())
    }

    /// Spawn a task to render a template, storing the result in a pre-defined
    /// lock. As this is a preview, the user will *not* be prompted for any
    /// input. A placeholder value will be used for any prompts.
    fn render_template_preview(
        &self,
        template: Template,
        profile_id: Option<ProfileId>,
        on_complete: Box<
            dyn 'static + Send + Sync + FnOnce(Vec<TemplateChunk>),
        >,
    ) -> anyhow::Result<()> {
        let context = self.template_context(profile_id, true)?;
        let messages_tx = self.messages_tx();
        tokio::spawn(async move {
            // Render chunks, then write them to the output destination
            let chunks = template.render_chunks(&context).await;
            on_complete(chunks);
            // Trigger a draw
            messages_tx.send(Message::TemplatePreviewComplete);
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
        is_preview: bool,
    ) -> anyhow::Result<TemplateContext> {
        let context = TuiContext::get();
        let collection = &self.collection_file.collection;
        let (http_engine, prompter): (_, Box<dyn Prompter>) = if is_preview {
            (None, Box::new(PreviewPrompter))
        } else {
            (
                Some(context.http_engine.clone()),
                Box::new(self.messages_tx()),
            )
        };

        Ok(TemplateContext {
            selected_profile: profile_id,
            collection: collection.clone(),
            http_engine,
            database: self.database.clone(),
            overrides: Default::default(),
            prompter,
            state: Default::default(),
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
