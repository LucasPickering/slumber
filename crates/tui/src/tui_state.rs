use crate::{
    context::TuiContext,
    http::{RequestConfig, RequestState, RequestStore, TuiHttpProvider},
    message::{
        Callback, HttpMessage, Message, MessageSender, RecipeCopyTarget,
    },
    util::{self, ResultReported},
    view::{
        ComponentMap, PreviewPrompter, RequestDisposition, TuiPrompter,
        UpdateContext, View, persistent::PersistentStore,
    },
};
use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use ratatui::buffer::Buffer;
use slumber_core::{
    collection::{Collection, CollectionFile, ProfileId},
    database::CollectionDatabase,
    http::{Exchange, RequestError, RequestId, RequestSeed},
    render::{Prompter, TemplateContext},
};
use slumber_template::{RenderedOutput, Template};
use slumber_util::{ResultTraced, yaml::SourceLocation};
use std::{path::PathBuf, sync::Arc};
use tokio::task;

/// Main TUI state. This is responsible for handling most of the TUI messages
/// and state updates, as well the case of the collection failing to load on
/// initial startup.
#[derive(Debug)]
pub struct TuiState(TuiStateInner);

impl TuiState {
    /// Load the collection file and initialize TUI state. If the collection
    /// fails to load, initialize an error state instead to show the error to
    /// the user. The error state will watch the file so the user can try to
    /// fix it without having to restart the TUI.
    ///
    /// ## Panics
    ///
    /// Panic if the collection fails to load and we're not able to start a file
    /// watcher on it
    pub fn load(
        database: CollectionDatabase,
        collection_file: CollectionFile,
        messages_tx: MessageSender,
    ) -> Self {
        let collection = match collection_file.load() {
            Ok(collection) => collection,
            Err(error) => {
                return {
                    // Collection failed to load. Store the error so we can show
                    // it to the user, and watch the file for changes so they
                    // don't have to restart the TUI once it's fixed
                    Self(TuiStateInner::Error {
                        database,
                        collection_file,
                        error: error.into(),
                        messages_tx,
                    })
                };
            }
        };

        Self(TuiStateInner::Loaded(LoadedState::new(
            database,
            collection_file,
            collection,
            messages_tx,
        )))
    }

    /// Handle an incoming message. Any error here should trigger a subsequent
    /// message with the error, which will display a modal.
    ///
    /// Some messages should be handle in the parent `Tui`, if they require
    /// access to root-level state. Most message types are handled here though.
    ///
    /// ## Panics
    ///
    /// Panic if we receive a message of a type that we expected the root TUI
    /// to handle.
    pub fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        // This message has to be handled specially because it involves a
        // wholesale replacement of the state
        if let Message::CollectionSelect(path) = message {
            return self.select_collection(path);
        }

        match &mut self.0 {
            TuiStateInner::Loaded(state) => state.handle_message(message),
            // Nothing to do in the error state
            TuiStateInner::Error {
                database,
                collection_file,
                messages_tx,
                ..
            } => match message {
                // Try to reload from scratch. If it fails again, we'll just
                // end up with another error state. Unlike the live reloading
                // which runs in a background task, this will block the main
                // thread. It makes the logic simpler and blocking shouldn't
                // be an issue since there isn't anything for the user to do
                Message::CollectionStartReload => {
                    *self = Self::load(
                        database.clone(),
                        collection_file.clone(),
                        messages_tx.clone(),
                    );
                    Ok(())
                }
                // Any other message is not useful to us
                _ => Ok(()),
            },
        }
    }

    /// Get a reference to the [CollectionFile] in use
    pub fn collection_file(&self) -> &CollectionFile {
        match &self.0 {
            TuiStateInner::Loaded(state) => &state.collection_file,
            TuiStateInner::Error {
                collection_file, ..
            } => collection_file,
        }
    }

    /// Are there any active HTTP requests?
    pub fn has_active_requests(&self) -> bool {
        match &self.0 {
            TuiStateInner::Loaded(state) => {
                state.request_store.has_active_requests()
            }
            TuiStateInner::Error { .. } => false,
        }
    }

    /// Handle all events in the queue. Return `true` if at least one event was
    /// consumed, `false` if the queue was empty
    pub fn drain_events(&mut self) -> bool {
        match &mut self.0 {
            TuiStateInner::Loaded(state) => {
                let handled = state.view.handle_events(UpdateContext {
                    component_map: &state.component_map,
                    persistent_store: &mut PersistentStore::new(
                        state.database.clone(),
                    ),
                    request_store: &mut state.request_store,
                });
                // Persist state after changes
                if handled {
                    state.view.persist(state.database.clone());
                }
                handled
            }
            // There is no event queue in the error state
            TuiStateInner::Error { .. } => false,
        }
    }

    /// Draw the view onto the screen
    pub fn draw(&mut self, buffer: &mut Buffer) {
        match &mut self.0 {
            TuiStateInner::Loaded(state) => {
                state.component_map = state.view.draw(buffer);
            }
            TuiStateInner::Error {
                collection_file,
                error,
                ..
            } => {
                // We can't show the real UI without a collection, so just show
                // the error. We have a watcher on the file so when it changes,
                // we'll reload it
                View::draw_collection_load_error(
                    buffer,
                    collection_file,
                    error,
                );
            }
        }
    }

    /// Get a reference to the collection
    ///
    /// Return `None` iff the collection failed to load and we're in an error
    /// state
    pub fn collection(&self) -> Option<&Collection> {
        match &self.0 {
            TuiStateInner::Loaded(state) => Some(&state.collection),
            TuiStateInner::Error { .. } => None,
        }
    }

    /// Get a reference to the database handle
    pub fn database(&self) -> &CollectionDatabase {
        match &self.0 {
            TuiStateInner::Loaded(state) => &state.database,
            TuiStateInner::Error { database, .. } => database,
        }
    }

    /// Select a new collection file, replacing this state entirely
    fn select_collection(&mut self, path: PathBuf) -> anyhow::Result<()> {
        let collection_file = CollectionFile::new(Some(path))?;

        // Reuse the existing DB connection and message channel. We clone
        // because we can't move out of the old state until the new one has
        // replaced it. These are cheap clones.
        let (database, messages_tx) = match &self.0 {
            TuiStateInner::Loaded(state) => {
                (&state.database, &state.messages_tx)
            }
            TuiStateInner::Error {
                database,
                messages_tx,
                ..
            } => (database, messages_tx),
        };
        let database =
            database.root().clone().into_collection(&collection_file)?;

        *self = Self::load(database, collection_file, messages_tx.clone());
        Ok(())
    }
}

/// Inner enum for [TuiState] to avoid exposing the variant values
#[derive(Debug)]
#[expect(clippy::large_enum_variant)]
enum TuiStateInner {
    /// TUI loaded successfully and is off and running
    Loaded(LoadedState),
    /// Collection failed to load on startup. Without a collection we can't
    /// show the UI. We'll just sit and wait for it to come back.
    Error {
        database: CollectionDatabase,
        collection_file: CollectionFile,
        /// Error that occurred while loading the collection
        error: anyhow::Error,
        messages_tx: MessageSender,
    },
}

/// Main state for the running TUI. This handles most of the TUI's state
/// management, event processing, and other life cycle activities. This is only
/// initialized one we have a valid initial collection.
#[derive(Debug)]
struct LoadedState {
    /// Handle for the file from which the collection will be loaded
    collection_file: CollectionFile,
    /// Loaded and deserialized request collection
    collection: Arc<Collection>,

    /// Persistence database, for storing request state, UI state, etc.
    database: CollectionDatabase,
    /// Sender for the mpsc message queue
    messages_tx: MessageSender,
    /// A map of all components drawn in the most recent draw phase
    component_map: ComponentMap,
    /// In-memory store of request state. This tracks state for requests that
    /// are in progress, and also serves as a cache for requests from the DB.
    request_store: RequestStore,
    /// UI presentation and state
    view: View,
}

impl LoadedState {
    /// Initialize state when the collection has been loaded successfully
    fn new(
        database: CollectionDatabase,
        collection_file: CollectionFile,
        collection: Collection,
        messages_tx: MessageSender,
    ) -> Self {
        let collection = Arc::new(collection);
        let request_store = RequestStore::new(database.clone());
        let view =
            View::new(&collection, database.clone(), messages_tx.clone());

        let state = LoadedState {
            collection_file,
            collection,
            database,
            messages_tx,
            component_map: ComponentMap::default(),
            request_store,
            view,
        };
        state.update_collection_name();
        state
    }

    /// Handle an incoming message. Any error here should trigger a subsequent
    /// message with the error, which will display a modal.
    ///
    /// Some messages should be handle in the parent `Tui`, if they require
    /// access to root-level state. Most message types are handled here though.
    ///
    /// ## Panics
    ///
    /// Panic if we receive a message of a type that we expected the root TUI
    /// to handle.
    fn handle_message(&mut self, message: Message) -> anyhow::Result<()> {
        match message {
            Message::CollectionStartReload => self.load_collection(),
            Message::CollectionEndReload(collection) => {
                self.reload_collection(collection);
            }
            Message::CollectionEdit { location } => {
                self.edit_collection(location)?;
            }
            Message::CopyRecipe(target) => self.copy_recipe(target)?,
            Message::CopyText(text) => self.view.copy_text(text)?,
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
            Message::FileEdit { file, on_complete } => {
                let editor = TuiContext::get().config.editor()?;
                util::yield_terminal(
                    editor.open(file.path()),
                    &self.messages_tx,
                )?;
                on_complete(file);
            }
            Message::FileView { file, mime } => {
                let pager =
                    TuiContext::get().config.tui.pager(mime.as_ref())?;
                util::yield_terminal(
                    pager.open(file.path()),
                    &self.messages_tx,
                )?;
                // Dropping the file deletes it, so we can't do it until after
                // the command is done
                drop(file);
            }
            Message::Error { error } => self.view.error(error),
            Message::Http(message) => self.handle_http(message)?,
            Message::HttpGetLatest {
                profile_id,
                recipe_id,
                channel,
            } => {
                let exchange = self
                    .request_store
                    .load_latest_exchange(profile_id.as_ref(), &recipe_id)
                    .reported(&self.messages_tx)
                    .flatten()
                    .cloned();
                channel.reply(exchange);
            }
            Message::Input(event) => self.view.handle_input(event),
            Message::Notify(message) => self.view.notify(message),
            Message::Question(question) => self.view.question(question),
            Message::TemplatePreview {
                template,
                can_stream,
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
                    can_stream,
                    on_complete,
                );
            }

            // All other messages are handled by the root TUI and should never
            // get here
            Message::CollectionSelect(_)
            | Message::ClearTerminal
            | Message::Quit => {
                panic!(
                    "Unexpected message in TuiState; should have been handled \
                    by parent: {message:?}"
                )
            }
        }
        Ok(())
    }

    /// Get a cheap clone of the message queue transmitter
    fn messages_tx(&self) -> MessageSender {
        self.messages_tx.clone()
    }

    /// Spawn a background task to load+parse the collection file
    ///
    /// YAML parsing is CPU-bound so do it in a blocking task. In all likelihood
    /// this will be extremely fast, but it's possible there's some edge case
    /// that causes it to be slow and we don't want to block the render loop.
    fn load_collection(&self) {
        let messages_tx = self.messages_tx();
        let collection_file = self.collection_file.clone();
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

    /// Reload state with a new collection
    fn reload_collection(&mut self, collection: Collection) {
        self.collection = collection.into();
        self.update_collection_name();

        // Rebuild the whole view, because tons of things can change
        self.view = View::new(
            &self.collection,
            self.database.clone(),
            self.messages_tx(),
        );
        self.view.notify("Reloaded collection");
    }

    /// Open the collection file in the user's editor
    fn edit_collection(
        &self,
        location: Option<SourceLocation>,
    ) -> anyhow::Result<()> {
        let editor = TuiContext::get().config.editor()?;
        let command = if let Some(location) = location {
            editor.open_at(location.source, location.line, location.column)
        } else {
            editor.open(self.collection_file.path())
        };
        util::yield_terminal(command, &self.messages_tx)
    }

    /// Update the collection name in the DB according to the loaded collection.
    /// Call this whenever the collection is successfully loaded to ensure the
    /// DB is kept up to date.
    fn update_collection_name(&self) {
        self.database.set_name(self.collection.name.as_deref());
    }

    /// Copy some component of the current recipe. Depending on the target, this
    /// may require rendering some or all of the recipe
    fn copy_recipe(&mut self, target: RecipeCopyTarget) -> anyhow::Result<()> {
        match target {
            // Render+copy URL
            RecipeCopyTarget::Url => self.render_copy(async |context, seed| {
                let http_engine = &TuiContext::get().http_engine;
                let url = http_engine.build_url(seed, &context).await?;
                Ok(url.to_string())
            }),

            // Render+copy body
            RecipeCopyTarget::Body => {
                self.render_copy(async |context, seed| {
                    let http_engine = &TuiContext::get().http_engine;
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
                let command =
                    self.request_config()?.to_cli(self.collection_file.path());
                self.view.copy_text(command)
            }

            // Render request, then copy the equivalent curl command
            RecipeCopyTarget::Curl => {
                self.render_copy(async |context, seed| {
                    let http_engine = &TuiContext::get().http_engine;
                    http_engine
                        .build_curl(seed, &context)
                        .await
                        .map_err(anyhow::Error::from)
                })
            }

            RecipeCopyTarget::Python => {
                let code = self
                    .request_config()?
                    .to_python(self.collection_file.path());
                self.view.copy_text(code)
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
        let messages_tx = self.messages_tx();
        let RequestConfig {
            profile_id,
            recipe_id,
            options,
        } = self.request_config()?;
        let seed = RequestSeed::new(recipe_id, options);
        // Even though this isn't a real request, we use a real request ID
        // because we may need to show prompts to the user under that ID
        let context = self.template_context(profile_id, Some(seed.id));

        let future = render(context, seed);
        util::spawn_result(async move {
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
        let Some(request_state) = self.request_store.get(request_id) else {
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
        util::spawn_result(util::save_file(
            self.messages_tx(),
            default_path,
            data,
        ));
        Ok(())
    }

    /// Get the current request config for the selected recipe. The config
    /// defines how to build a request. If no recipe is selected, this returns
    /// an error. This should only be called in contexts where we can safely
    /// assume that a recipe is selected (e.g. triggered via an action on a
    /// recipe), so an error indicates a bug.
    fn request_config(&self) -> anyhow::Result<RequestConfig> {
        self.view
            .request_config()
            .ok_or_else(|| anyhow!("No recipe selected"))
    }

    /// Launch an HTTP request in a separate task
    fn send_request(&mut self) -> anyhow::Result<RequestId> {
        let RequestConfig {
            profile_id,
            recipe_id,
            options,
        } = self.request_config()?;
        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.

        let seed = RequestSeed::new(recipe_id.clone(), options);
        let request_id = seed.id;
        let template_context =
            self.template_context(profile_id.clone(), Some(request_id));
        let messages_tx = self.messages_tx();

        // Don't use spawn_result here, because errors are handled specially for
        // requests
        let join_handle = util::spawn(async move {
            // Build the request
            let result = TuiContext::get()
                .http_engine
                .build(seed, &template_context)
                .await;
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
        });

        // Add the new request to the store. This has to go after spawning the
        // task so we can include the join handle (for cancellation)
        self.request_store.start(
            request_id,
            profile_id,
            recipe_id,
            Some(join_handle.abort_handle()),
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
                // Persist in the DB if not disabled by global config or recipe
                let persist = TuiContext::get().config.tui.persist
                    && self
                        .collection
                        .recipes
                        .try_get_recipe(&exchange.request.recipe_id)
                        .is_ok_and(|recipe| recipe.persist);
                if persist {
                    let _ = self.database.insert_exchange(&exchange).traced();
                }

                self.request_store.response(exchange)
            }
            Err(error) => self.request_store.request_error(error),
        }
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
        util::spawn(async move {
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
        let collection = &self.collection;
        // If request_id is given, it's a request build. Otherwise it's a
        // preview
        let is_preview = request_id.is_none();
        let http_provider =
            TuiHttpProvider::new(self.messages_tx(), is_preview);
        let prompter: Box<dyn Prompter> = if let Some(request_id) = request_id {
            Box::new(TuiPrompter::new(request_id, self.messages_tx()))
        } else {
            Box::new(PreviewPrompter)
        };

        TemplateContext {
            selected_profile: profile_id,
            collection: Arc::clone(collection),
            http_provider: Box::new(http_provider),
            prompter,
            overrides: self.view.profile_overrides(),
            show_sensitive: !is_preview,
            root_dir: self.collection_file.parent().to_owned(),
            state: Default::default(),
        }
    }

    /// Handle an [HttpMessage]
    fn handle_http(&mut self, message: HttpMessage) -> anyhow::Result<()> {
        let disposition = match message {
            HttpMessage::Triggered {
                request_id,
                profile_id,
                recipe_id,
            } => {
                self.request_store
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
                let id = self.request_store.prompt(request_id, prompt).id();
                // For any new prompt, jump to the form. This may potentially
                // be annoying for delayed prompts. If so we can change it :)
                RequestDisposition::OpenForm(id)
            }
            HttpMessage::FormSubmit {
                request_id,
                replies: responses,
            } => {
                let id =
                    self.request_store.submit_form(request_id, responses).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::BuildError(error) => {
                let id = self.request_store.build_error(error).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::Loading(request) => {
                let id = self.request_store.loading(request).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::Complete(result) => {
                let id = self.complete_request(result).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::Cancel(request_id) => {
                let id = self.request_store.cancel(request_id).id();
                RequestDisposition::Change(id)
            }
            HttpMessage::DeleteRequest(request_id) => {
                self.request_store.delete_request(request_id)?;
                RequestDisposition::Change(request_id)
            }
            HttpMessage::DeleteRecipe {
                recipe_id,
                profile_filter,
            } => {
                let deleted = self
                    .request_store
                    .delete_recipe_requests(profile_filter, &recipe_id)?;
                RequestDisposition::ChangeAll(deleted)
            }
        };

        // Tell the UI that *something* changed in the request store, and
        // optionally the disposition will tell it if anything should change.
        // The view is responsible for checking the store to see if the current
        // request was changed at all, and modify the view if so.
        self.view
            .refresh_request(&mut self.request_store, disposition);

        Ok(())
    }
}
