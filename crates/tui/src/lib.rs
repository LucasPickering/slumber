//! Terminal user interface for Slumber.
//!
//! **This crate is not semver compliant**. The version is locked to the root
//! `slumber` crate version. If you choose to depend directly on this crate, you
//! do so at your own risk of breakage.

mod collection_state;
mod http;
mod input;
mod message;
#[cfg(test)]
mod test_util;
mod util;
mod view;

use crate::{
    collection_state::CollectionState,
    http::{RequestConfig, RequestState, TuiHttpProvider},
    input::{InputBindings, InputEvent},
    message::{
        Callback, HttpMessage, Message, MessageSender, RecipeCopyTarget,
    },
    util::ResultReported,
    view::{PreviewPrompter, RequestDisposition, TuiPrompter},
};
use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use crossterm::event::{self, EventStream};
use futures::{Stream, StreamExt, future, pin_mut};
use ratatui::{
    Terminal,
    buffer::Buffer,
    layout::Position,
    prelude::{Backend, CrosstermBackend},
};
use slumber_config::{Action, Config};
use slumber_core::{
    collection::{Collection, CollectionFile, ProfileId},
    database::{CollectionDatabase, Database},
    http::{Exchange, HttpEngine, RequestError, RequestId, RequestSeed},
    render::{Prompter, TemplateContext},
};
use slumber_template::{RenderedOutput, Template};
use slumber_util::{ResultTraced, yaml::SourceLocation};
use std::{
    io::{self, Stdout},
    ops::Deref,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    select,
    sync::mpsc::{self, UnboundedReceiver},
    task, time,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace};

/// Main controller struct for the TUI. The app uses a React-ish architecture
/// for the view, with a wrapping controller (this struct)
///
/// `B` is the terminal backend type; see [Backend]
#[derive(Debug)]
pub struct Tui<B: Backend> {
    /// Token to manage cancellation of the main loop and all background tasks
    ///
    /// If run() is called multiple times (e.g. in a test), a new token will be
    /// generated for each call because they are single-use.
    cancel_token: CancellationToken,
    /// App-wide configuration. This never gets reloaded; it'll be the same for
    /// the entire session. The Arc allows cheap sharing throughout the app.
    config: Arc<Config>,
    /// Persistence database, for storing request state, UI state, etc.
    ///
    /// This is the root database, *not* scoped to a specific collection. We'll
    /// mostly used the scoped version from [CollectionState].
    database: Database,
    /// Make request go brrr
    http_engine: HttpEngine,
    /// Receiver for the async message queue, which allows background tasks and
    /// the view to pass data and trigger side effects. Nobody else gets to
    /// touch this
    messages_rx: UnboundedReceiver<Message>,
    /// Transmitter for the async message queue, which can be freely cloned and
    /// passed around
    messages_tx: MessageSender,
    /// All state related to the current collection file
    ///
    /// This gets replaced wholesale when switching collection files
    state: CollectionState,
    /// Output terminal. Parameterized for testing.
    terminal: Terminal<B>,
}

impl Tui<CrosstermBackend<Stdout>> {
    /// Start the TUI on a real terminal. Any errors that occur during startup
    /// will be panics, because they prevent TUI execution.
    pub async fn start(collection_path: Option<PathBuf>) -> anyhow::Result<()> {
        let app =
            Self::new(CrosstermBackend::new(io::stdout()), collection_path)?;
        // Stream input from the terminal
        let input_stream = EventStream::new().map(|event_result| {
            let event = event_result.expect("Error reading terminal input");
            // Convert from crossterm to the common terminput format. This
            // enables support for multiple terminal backends
            terminput_crossterm::to_terminput(event).unwrap()
        });

        // The code to revert the terminal takeover is in `Tui::drop`, so we
        // shouldn't take over the terminal until right before creating the
        // `Tui`.
        initialize_panic_handler();
        util::initialize_terminal()?;

        // ===== CRITICAL SECTION =====
        // Do not exit from here (other than panic) to ensure the terminal gets
        // restored below
        //
        // Run everything in one local set, so that we can use !Send values
        let local = task::LocalSet::new();
        local.spawn_local(app.run(input_stream));
        local.await;
        // ===== END CRITICAL SECTION =====

        // Restore terminal
        if let Err(err) = util::restore_terminal() {
            error!(error = err.deref(), "Error restoring terminal, sorry!");
        }

        Ok(())
    }
}

impl<B> Tui<B>
where
    B: Backend,
    B::Error: 'static + Send + Sync,
{
    /// Rough **maximum** time for each iteration of the main loop
    const TICK_TIME: Duration = Duration::from_millis(250);

    /// Create a new TUI
    ///
    /// This will *not* start the TUI process. It initializes all needed state,
    /// config, etc. but will not write to the terminal or read input yet. Call
    /// [Self::run] to run the main loop.
    pub fn new(
        backend: B,
        collection_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        // Create a message queue for handling async tasks
        let (messages_tx, messages_rx) = mpsc::unbounded_channel();
        let messages_tx = MessageSender::new(messages_tx);

        // Load config file. Failure shouldn't be fatal since we can fall back
        // to default, just show an error to the user
        let config: Arc<Config> = Config::load()
            .reported(&messages_tx)
            .unwrap_or_default()
            .into();
        let http_engine = HttpEngine::new(&config.http);
        let database = Database::load()?;

        // Initialize TUI state, which will try to load the collection. If it
        // fails to load, we'll dump the user into an error state that watches
        // the file
        let collection_file = CollectionFile::new(collection_path)?;
        let state = CollectionState::load(
            config.clone(),
            collection_file,
            database.clone(),
            messages_tx.clone(),
        );

        let terminal = Terminal::new(backend)?;

        Ok(Self {
            cancel_token: CancellationToken::new(),
            config,
            database,
            http_engine,
            messages_rx,
            messages_tx,
            state,
            terminal,
        })
    }

    /// Get a reference to the terminal backend
    pub fn backend(&self) -> &B {
        self.terminal.backend()
    }

    /// Get a reference to the collection
    ///
    /// Return `None` iff the collection failed to load and we're in an error
    /// state
    pub fn collection(&self) -> Option<&Collection> {
        self.state.collection.as_deref().ok()
    }

    /// Get a reference to the database handle
    pub fn database(&self) -> &CollectionDatabase {
        &self.state.database
    }

    /// Run the main TUI update loop
    ///
    /// Any error returned from this is fatal. See the struct definition for a
    /// description of the different phases of the run loop. If the loop exits
    /// gracefully, return `self`. This is useful in integration tests.
    pub async fn run(
        mut self,
        input_stream: impl Stream<Item = terminput::Event>,
    ) -> anyhow::Result<Self> {
        // Spawn background tasks
        self.listen_for_signals();
        self.watch_collection();

        let input_bindings =
            InputBindings::new(self.config.tui.input_bindings.clone());
        // Stream of terminal input events. Events that don't map to a message
        // (cursor move, focus, etc.) should be filtered out entirely so
        // they don't trigger any updates
        let input_stream = input_stream.filter_map(move |event| {
            future::ready(input_bindings.convert_event(event))
        });
        pin_mut!(input_stream);

        self.draw(false)?; // Initial draw

        // This loop is limited by the rate that messages come in, with a
        // minimum rate enforced by a timeout.
        // The loop terminates when the cancel token is set
        loop {
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
                () = self.cancel_token.cancelled() => break,
            };

            // We'll try to skip draws if nothing on the screen has changed, to
            // limit idle CPU usage. If a request is running we always need to
            // update though, because the timer will be ticking.
            let mut needs_draw = self.state.request_store.has_active_requests();

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

        // The loop may be called again (in a test), so leave a fresh token for
        // the next run
        self.cancel_token = CancellationToken::new();

        Ok(self)
    }

    /// Handle an incoming message. Any error here will be displayed as a modal
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::ClearTerminal => self.terminal.clear()?,

            Message::CollectionEndReload(collection) => {
                // Because we're just swapping out the collection value and
                // using the same file, we can just update state instead of
                // replacing it
                self.state.set_collection(collection);
            }
            Message::CollectionSelect(path) => {
                // Collection file has changed, so we have to rebuild state
                let collection_file = CollectionFile::new(Some(path))?;
                self.state = CollectionState::load(
                    self.config.clone(),
                    collection_file,
                    self.database.clone(),
                    self.messages_tx.clone(),
                );
            }
            Message::CollectionStartReload => self.reload_collection(),
            Message::CollectionEdit { location } => {
                self.edit_collection(location)?;
            }

            Message::CopyRecipe(target) => self.copy_recipe(target)?,
            Message::CopyText(text) => self.state.view.copy_text(text)?,

            Message::Error { error } => self.state.view.error(error),

            Message::FileEdit { file, on_complete } => {
                let editor = self.config.editor()?;
                util::yield_terminal(
                    editor.open(file.path()),
                    &self.messages_tx,
                )?;
                on_complete(file);
            }
            Message::FileView { file, mime } => {
                let pager = self.config.tui.pager(mime.as_ref())?;
                util::yield_terminal(
                    pager.open(file.path()),
                    &self.messages_tx,
                )?;
                // Dropping the file deletes it, so we can't do it until after
                // the command is done
                drop(file);
            }

            Message::Http(message) => self.handle_http(message)?,
            Message::HttpGetLatest {
                profile_id,
                recipe_id,
                channel,
            } => {
                let exchange = self
                    .state
                    .request_store
                    .load_latest_exchange(profile_id.as_ref(), &recipe_id)
                    .reported(&self.messages_tx)
                    .flatten()
                    .cloned();
                channel.reply(exchange);
            }

            // Force quit short-circuits the view/message cycle, to make sure
            // it doesn't get ate by text boxes
            Message::Input(InputEvent::Key {
                action: Some(Action::ForceQuit),
                ..
            })
            | Message::Quit => self.quit(),
            Message::Input(InputEvent::Resize { .. }) => {
                // Redraw the entire screen. There are certain scenarios where
                // the terminal gets cleared but ratatui's (e.g. waking from
                // sleep) buffer doesn't, so the two get out of sync
                self.terminal.clear()?;
                self.draw(false)?;
            }
            Message::Input(event) => self.state.view.handle_input(event),

            Message::Notify(message) => self.state.view.notify(message),
            Message::Question(question) => self.state.view.question(question),
            Message::SaveResponseBody { request_id, data } => {
                self.save_response_body(request_id, data).with_context(
                    || {
                        format!(
                            "Error saving response body \
                            for request {request_id}"
                        )
                    },
                )?;
            }
            Message::Spawn(future) => {
                self.spawn(future);
            }
            Message::TemplatePreview {
                template,
                can_stream,
                on_complete,
            } => {
                // Note: there's a potential bug here, if the selected profile
                // changed since this message was queued. In practice is
                // extremely unlikely (potentially impossible), and this
                // shortcut saves us a lot of plumbing so it's worth it
                let profile_id = self.state.view.selected_profile_id().cloned();
                self.render_template_preview(
                    template,
                    profile_id,
                    can_stream,
                    on_complete,
                );
            }
        }
        Ok(())
    }

    /// Handle an [HttpMessage]
    fn handle_http(&mut self, message: HttpMessage) -> anyhow::Result<()> {
        let disposition = match message {
            HttpMessage::Triggered {
                request_id,
                profile_id,
                recipe_id,
            } => {
                self.state
                    .request_store
                    .start(request_id, profile_id, recipe_id, None);
                // Request is triggered in the background. Switching to it could
                // be jarring
                RequestDisposition::Change(request_id)
            }
            HttpMessage::Begin => {
                let id = self.send_request()?;
                // New requests should be shown immediately
                RequestDisposition::Select(id)
            }
            HttpMessage::Prompt { request_id, prompt } => {
                let id =
                    self.state.request_store.prompt(request_id, prompt).id();
                // For any new prompt, jump to the form. This may potentially
                // be annoying for delayed prompts. If so we can change it :)
                RequestDisposition::OpenForm(id)
            }
            HttpMessage::FormSubmit {
                request_id,
                replies: responses,
            } => {
                let id = self
                    .state
                    .request_store
                    .submit_form(request_id, responses)
                    .id();
                RequestDisposition::Change(id)
            }
            HttpMessage::BuildError(error) => {
                let id = self.state.request_store.build_error(error).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::Loading(request) => {
                let id = self.state.request_store.loading(request).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::Complete(result) => {
                let id = self.complete_request(result).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::Cancel(request_id) => {
                let id = self.state.request_store.cancel(request_id).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::DeleteRequest(request_id) => {
                self.state.request_store.delete_request(request_id)?;
                RequestDisposition::Change(request_id)
            }
            HttpMessage::DeleteRecipe {
                recipe_id,
                profile_filter,
            } => {
                let deleted = self
                    .state
                    .request_store
                    .delete_recipe_requests(profile_filter, &recipe_id)?;
                RequestDisposition::ChangeAll(deleted)
            }
        };

        // Tell the UI that *something* changed in the request store, and
        // optionally the disposition will tell it if anything should change.
        // The view is responsible for checking the store to see if the current
        // request was changed at all, and modify the view if so.
        self.state
            .view
            .refresh_request(&mut self.state.request_store, disposition);

        Ok(())
    }

    /// Spawn a task to listen in the background for quit signals
    fn listen_for_signals(&self) {
        let messages_tx = self.messages_tx.clone();
        self.spawn(async move {
            util::signals().await.reported(&messages_tx);
            messages_tx.send(Message::Quit);
        });
    }

    /// Spawn a task to watch the collection file for changes
    fn watch_collection(&self) {
        let path = self.state.collection_file.path().to_owned();
        let messages_tx = self.messages_tx.clone();

        self.spawn(util::watch_file(path, move || {
            messages_tx.send(Message::CollectionStartReload);
        }));
    }

    /// Spawn a background task to load+parse the collection file
    ///
    /// YAML parsing is CPU-bound so do it in a blocking task. In all likelihood
    /// this will be extremely fast, but it's possible there's some edge case
    /// that causes it to be slow and we don't want to block the render loop.
    fn reload_collection(&self) {
        let messages_tx = self.messages_tx.clone();
        let collection_file = self.state.collection_file.clone();
        // We need two tasks here:
        // - Inner blocking task runs on another thread and parses the YAML
        // - Outer local task runs on the main thread and just waits on the
        //   inner task. Once it's done, it sends the result back to the loop
        // We can't do the parsing on the local task because it would block the
        // loop, and we can't send the message from the blocking thread because
        // messages are !Send
        task::spawn_local(async move {
            let result = task::spawn_blocking(move || collection_file.load())
                .await
                .context("Collection loading panicked");
            let message = match result {
                Ok(Ok(collection)) => Message::CollectionEndReload(collection),
                // Load error
                Ok(Err(error)) => Message::Error {
                    error: error.into(),
                },
                // Join error - panic in the thread
                Err(error) => Message::Error { error },
            };
            messages_tx.send(message);
        });
    }

    /// Open the collection file in the user's editor
    fn edit_collection(
        &self,
        location: Option<SourceLocation>,
    ) -> anyhow::Result<()> {
        let editor = self.config.editor()?;
        let command = if let Some(location) = location {
            editor.open_at(location.source, location.line, location.column)
        } else {
            editor.open(self.state.collection_file.path())
        };
        util::yield_terminal(command, &self.messages_tx)
    }

    /// Spawn a task on the main thread
    ///
    /// Because the task is run on the main thread, it can be `!Send`. This
    /// allows view tasks to access the event queue. The task will be
    /// automatically cancelled when the TUI exits.
    fn spawn(&self, future: impl 'static + Future<Output = ()>) {
        task::spawn_local(util::cancellable(&self.cancel_token, future));
    }

    /// GOODBYE
    fn quit(&mut self) {
        info!("Initiating graceful shutdown");
        // Kill the main loop and all background tasks
        self.cancel_token.cancel();
    }

    /// Draw the view onto the screen.
    ///
    /// If `null` is true, the draw will be done with a null buffer. This will
    /// update all state in the component tree, but won't actually write to the
    /// terminal buffer. This should be enabled when we know there will be
    /// subsequent draws (i.e. if there are more events in the queue) to improve
    /// performance.
    fn draw(&mut self, null: bool) -> anyhow::Result<()> {
        if null {
            let size = self.terminal.size()?;
            let mut buffer = Buffer::empty((Position::default(), size).into());
            self.state.draw(&mut buffer);
        } else {
            self.terminal
                .draw(|frame| self.state.draw(frame.buffer_mut()))?;
        }
        Ok(())
    }

    /// Launch an HTTP request in a separate task
    fn send_request(&mut self) -> anyhow::Result<RequestId> {
        let RequestConfig {
            profile_id,
            recipe_id,
            options,
        } = self.state.request_config()?;
        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.

        let seed = RequestSeed::new(recipe_id.clone(), options);
        let request_id = seed.id;
        let template_context =
            self.template_context(profile_id.clone(), Some(request_id));
        let http_engine = self.http_engine.clone();
        let messages_tx = self.messages_tx.clone();

        // Don't use spawn_result here, because errors are handled specially for
        // requests
        let cancel_token = CancellationToken::new();
        let future = async move {
            // Build the request
            let result = http_engine.build(seed, &template_context).await;
            let ticket = match result {
                Ok(ticket) => ticket,
                Err(error) => {
                    // Report the error, but don't actually return anything
                    messages_tx.send(HttpMessage::BuildError(error.into()));
                    return;
                }
            };

            // Report liftoff
            messages_tx.send(HttpMessage::Loading(Arc::clone(ticket.record())));

            // Send the request and report the result to the main thread
            let result = ticket.send().await.map_err(Arc::new);
            messages_tx.send(HttpMessage::Complete(result));
        };
        self.messages_tx
            .spawn(util::cancellable(&cancel_token, future));

        // Add the new request to the store. This has to go after spawning the
        // task so we can include the join handle (for cancellation)
        self.state.request_store.start(
            request_id,
            profile_id,
            recipe_id,
            Some(cancel_token),
        );

        Ok(request_id)
    }

    /// Process the result of an HTTP request
    fn complete_request(
        &mut self,
        result: Result<Exchange, Arc<RequestError>>,
    ) -> &RequestState {
        match result {
            Ok(exchange) => {
                // Shouldn't be reachable if the collection isn't defined
                // TODO there's a bug here if the collection swaps while the
                // request is in flight
                let collection = self.collection().expect("Collection missing");

                // Persist in the DB if not disabled by global config or recipe
                let persist = self.config.tui.persist
                    && collection
                        .recipes
                        .try_get_recipe(&exchange.request.recipe_id)
                        .is_ok_and(|recipe| recipe.persist);
                if persist {
                    let _ =
                        self.state.database.insert_exchange(&exchange).traced();
                }

                self.state.request_store.response(exchange)
            }
            Err(error) => self.state.request_store.request_error(error),
        }
    }

    /// Copy some component of the current recipe. Depending on the target, this
    /// may require rendering some or all of the recipe
    fn copy_recipe(&mut self, target: RecipeCopyTarget) -> anyhow::Result<()> {
        match target {
            // Render+copy URL
            RecipeCopyTarget::Url => {
                let http_engine = self.http_engine.clone();
                self.render_copy(async move |context, seed| {
                    let url = http_engine.build_url(seed, &context).await?;
                    Ok(url.to_string())
                })
            }

            // Render+copy body
            RecipeCopyTarget::Body => {
                let http_engine = self.http_engine.clone();
                self.render_copy(async move |context, seed| {
                    let body = http_engine
                        .build_body(seed, &context)
                        .await?
                        .ok_or(anyhow!("Request has no body"))?;
                    // Clone the bytes :(
                    String::from_utf8(body.into())
                        .context("Cannot copy request body")
                })
            }

            // Copy the recipe as a CLI command. This does *not* require
            // rendering; the render is done when the command is executed
            RecipeCopyTarget::Cli => {
                let command = self
                    .state
                    .request_config()?
                    .to_cli(self.state.collection_file.path());
                self.state.view.copy_text(command)
            }

            // Render request, then copy the equivalent curl command
            RecipeCopyTarget::Curl => {
                let http_engine = self.http_engine.clone();
                self.render_copy(async move |context, seed| {
                    http_engine
                        .build_curl(seed, &context)
                        .await
                        .map_err(anyhow::Error::from)
                })
            }

            RecipeCopyTarget::Python => {
                let code = self
                    .state
                    .request_config()?
                    .to_python(self.state.collection_file.path());
                self.state.view.copy_text(code)
            }
        }
    }

    /// Call an async function to render some part of a request to a string,
    /// then copy that string to the clipboard
    fn render_copy<F>(&self, render: F) -> anyhow::Result<()>
    where
        F: 'static
            + AsyncFnOnce(TemplateContext, RequestSeed) -> anyhow::Result<String>,
    {
        let messages_tx = self.messages_tx.clone();
        let RequestConfig {
            profile_id,
            recipe_id,
            options,
        } = self.state.request_config()?;
        let seed = RequestSeed::new(recipe_id, options);
        // Even though this isn't a real request, we use a real request ID
        // because we may need to show prompts to the user under that ID
        let context = self.template_context(profile_id, Some(seed.id));

        let future = render(context, seed);
        self.messages_tx.spawn_result(async move {
            let text = future.await?;
            messages_tx.send(Message::CopyText(text));
            Ok(())
        });

        Ok(())
    }

    /// Save the body of a response to a file, prompting the user for a file
    /// path. If the body text is provided, that will be used. Useful when
    /// what's being saved differs from the actual response body (because of
    /// prettification/querying). If not provided, we'll pull the body from the
    /// request store.
    fn save_response_body(
        &self,
        request_id: RequestId,
        text: Option<String>,
    ) -> anyhow::Result<()> {
        let Some(request_state) = self.state.request_store.get(request_id)
        else {
            bail!("Request not in store")
        };
        let RequestState::Response { exchange } = request_state else {
            bail!("Request is not complete")
        };
        // Get a suggested file name from the response if possible
        let default_path = exchange.response.file_name();

        let data = text.map(Bytes::from).unwrap_or_else(|| {
            // This is the path we hit for binary and/or large bodies that were
            // never parsed. This clone is cheap so we're being efficient!
            exchange.response.body.bytes().clone()
        });
        self.messages_tx.spawn_result(util::save_file(
            self.messages_tx.clone(),
            default_path,
            data,
        ));
        Ok(())
    }

    /// Spawn a task to render a template, storing the result in a pre-defined
    /// lock. As this is a preview, the user will *not* be prompted for any
    /// input. A placeholder value will be used for any prompts.
    fn render_template_preview(
        &self,
        template: Template,
        profile_id: Option<ProfileId>,
        can_stream: bool,
        on_complete: Callback<RenderedOutput>,
    ) {
        let context = self.template_context(profile_id, None);
        self.messages_tx.spawn(async move {
            // Render chunks, then write them to the output destination
            let chunks = template.render(&context.streaming(can_stream)).await;
            on_complete(chunks);
        });
    }

    /// Expose app state to the templater. Most of the data has to be cloned out
    /// to be passed across async boundaries. This is annoying but in reality
    /// it should be small data.
    fn template_context(
        &self,
        profile_id: Option<ProfileId>,
        // ID of the request being built is needed to group prompts that are
        // generated
        request_id: Option<RequestId>,
    ) -> TemplateContext {
        // Shouldn't be reachable if the collection isn't loaded
        let collection =
            self.state.collection.as_ref().expect("Collection missing");

        // If request_id is given, it's a request build. Otherwise it's a
        // preview
        let is_preview = request_id.is_none();
        let http_provider = TuiHttpProvider::new(
            self.http_engine.clone(),
            self.messages_tx.clone(),
            is_preview,
        );
        let prompter: Box<dyn Prompter> = if let Some(request_id) = request_id {
            Box::new(TuiPrompter::new(request_id, self.messages_tx.clone()))
        } else {
            Box::new(PreviewPrompter)
        };

        TemplateContext {
            selected_profile: profile_id,
            collection: Arc::clone(collection),
            http_provider: Box::new(http_provider),
            prompter,
            overrides: self.state.view.profile_overrides(),
            show_sensitive: !is_preview,
            root_dir: self.state.collection_file.parent().to_owned(),
            state: Default::default(),
        }
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
