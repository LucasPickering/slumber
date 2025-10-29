use crate::{
    context::TuiContext,
    http::{RequestState, RequestStore, TuiHttpProvider},
    message::{Callback, Message, MessageSender, RequestConfig},
    util::{self, ResultReported},
    view::{PreviewPrompter, TuiPrompter, UpdateContext, View},
};
use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache,
};
use ratatui::Frame;
use slumber_core::{
    collection::{Collection, CollectionFile, ProfileId},
    database::CollectionDatabase,
    http::{Exchange, RequestError, RequestId, RequestSeed},
    render::{Prompter, TemplateContext},
};
use slumber_template::{RenderedOutput, Template};
use slumber_util::{ResultTraced, yaml::SourceLocation};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::task;
use tracing::{error, info};

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
                    let Ok(watcher) = watch_collection(
                        collection_file.path(),
                        messages_tx.clone(),
                    ) else {
                        // If the watcher fails to initialize, there's no point
                        // in sitting in this error state. Show the original
                        // collection error. The watcher error is much less
                        // useful. It's accessible in the logs if needed.
                        panic!("{error:#}")
                    };
                    Self(TuiStateInner::Error {
                        database,
                        collection_file,
                        error: error.into(),
                        messages_tx,
                        _watcher: watcher,
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
                state.view.handle_events(UpdateContext {
                    request_store: &mut state.request_store,
                })
            }
            // There is no event queue in the error state
            TuiStateInner::Error { .. } => false,
        }
    }

    /// Draw the view onto the screen
    pub fn draw(&self, frame: &mut Frame) {
        match &self.0 {
            TuiStateInner::Loaded(state) => {
                state.view.draw(frame, &state.request_store);
            }
            TuiStateInner::Error {
                collection_file,
                error,
                ..
            } => {
                // We can't show the real UI without a collection, so just show
                // the error. We have a watcher on the file so when it changes,
                // we'll reload it
                View::draw_collection_load_error(frame, collection_file, error);
            }
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
        /// Watch the collection file and wait for changes that will hopefully
        /// fix it. We have to hang onto this because watching stops when it's
        /// dropped.
        _watcher: FileWatcher,
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
    /// In-memory store of request state. This tracks state for requests that
    /// are in progress, and also serves as a cache for requests from the DB.
    request_store: RequestStore,
    /// UI presentation and state
    view: View,

    /// Watcher for changes to the collection file. Whenever the file changes,
    /// the collection will be reloaded. This will be `None` iff the watcher
    /// fails to initialize
    _watcher: Option<FileWatcher>,
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
        let watcher =
            watch_collection(collection_file.path(), messages_tx.clone()).ok();

        let state = LoadedState {
            collection_file,
            collection,
            database,
            messages_tx,
            request_store,
            view,
            _watcher: watcher,
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
            Message::CollectionStartReload => {
                let messages_tx = self.messages_tx();
                let collection_file = self.collection_file.clone();
                // YAML parsing is CPU-bound so do it in a blocking task. In all
                // likelihood this will be extremely fast, but it's possible
                // there's some edge case that causes it to be slow and we don't
                // want to block the render loop
                task::spawn_blocking(move || {
                    let message = match collection_file.load() {
                        Ok(collection) => {
                            Message::CollectionEndReload(collection)
                        }
                        Err(error) => Message::Error {
                            error: error.into(),
                        },
                    };
                    messages_tx.send(message);
                });
            }
            Message::CollectionEndReload(collection) => {
                self.reload_collection(collection);
            }
            Message::CollectionEdit { location } => {
                self.edit_collection(location)?;
            }
            Message::CopyRequestUrl => self.copy_request_url()?,
            Message::CopyRequestBody => self.copy_request_body()?,
            Message::CopyRequestCurl => self.copy_request_curl()?,
            Message::CopyText(text) => self.view.copy_text(text),
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
            Message::HttpBeginRequest => self.send_request()?,
            Message::HttpBuildingTriggered {
                id,
                profile_id,
                recipe_id,
            } => self.request_store.start(id, profile_id, recipe_id, None),
            Message::HttpBuildError { error } => {
                self.request_store.build_error(error);
            }
            Message::HttpLoading { request } => {
                self.request_store.loading(request);
            }
            Message::HttpComplete(result) => self.complete_request(result),
            Message::HttpCancel(request_id) => {
                self.request_store.cancel(request_id);
            }
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
                channel.respond(exchange);
            }
            Message::Input { event, action } => {
                self.view.handle_input(event, action);
            }
            Message::Notify(message) => self.view.notify(message),
            Message::PromptStart(prompt) => self.view.prompt(prompt),
            Message::SelectStart(select) => self.view.select(select),
            Message::ConfirmStart(confirm) => self.view.confirm(confirm),
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
            | Message::Quit
            | Message::Draw => {
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

    /// Render URL for a request, then copy it to the clipboard
    fn copy_request_url(&self) -> anyhow::Result<()> {
        let RequestConfig {
            profile_id,
            recipe_id,
            options,
        } = self.request_config()?;
        let seed = RequestSeed::new(recipe_id, options);
        let template_context = self.template_context(profile_id, false);
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        util::spawn_result(async move {
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
    fn copy_request_body(&self) -> anyhow::Result<()> {
        let RequestConfig {
            profile_id,
            recipe_id,
            options,
        } = self.request_config()?;
        let seed = RequestSeed::new(recipe_id, options);
        let template_context = self.template_context(profile_id, false);
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        util::spawn_result(async move {
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
    fn copy_request_curl(&self) -> anyhow::Result<()> {
        let RequestConfig {
            profile_id,
            recipe_id,
            options,
        } = self.request_config()?;
        let seed = RequestSeed::new(recipe_id, options);
        let template_context = self.template_context(profile_id, false);
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        util::spawn_result(async move {
            let command = TuiContext::get()
                .http_engine
                .build_curl(seed, &template_context)
                .await?;
            messages_tx.send(Message::CopyText(command));
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
    fn send_request(&mut self) -> anyhow::Result<()> {
        let RequestConfig {
            profile_id,
            recipe_id,
            options,
        } = self.request_config()?;
        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.

        let template_context = self.template_context(profile_id.clone(), false);
        let messages_tx = self.messages_tx();

        let seed = RequestSeed::new(recipe_id.clone(), options);
        let request_id = seed.id;

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
                    messages_tx.send(Message::HttpBuildError {
                        error: error.into(),
                    });
                    return;
                }
            };

            // Report liftoff
            messages_tx.send(Message::HttpLoading {
                request: Arc::clone(ticket.record()),
            });

            // Send the request and report the result to the main thread
            let result = ticket.send().await.map_err(Arc::new);
            messages_tx.send(Message::HttpComplete(result));
        });

        // Add the new request to the store. This has to go after spawning the
        // task so we can include the join handle (for cancellation)
        self.request_store.start(
            request_id,
            profile_id,
            recipe_id,
            Some(join_handle.abort_handle()),
        );

        // New requests should get shown in the UI
        self.view
            .select_request(&mut self.request_store, request_id);

        Ok(())
    }

    /// Process the result of an HTTP request
    fn complete_request(
        &mut self,
        result: Result<Exchange, Arc<RequestError>>,
    ) {
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

                self.request_store.response(exchange);
            }
            Err(error) => {
                self.request_store.request_error(error);
            }
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
        let context = self.template_context(profile_id, true);
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
        is_preview: bool,
    ) -> TemplateContext {
        let collection = &self.collection;
        let http_provider =
            TuiHttpProvider::new(self.messages_tx(), is_preview);
        let prompter: Box<dyn Prompter> = if is_preview {
            Box::new(PreviewPrompter)
        } else {
            Box::new(TuiPrompter::new(self.messages_tx()))
        };

        TemplateContext {
            selected_profile: profile_id,
            collection: Arc::clone(collection),
            http_provider: Box::new(http_provider),
            prompter,
            overrides: Default::default(),
            show_sensitive: !is_preview,
            root_dir: self.collection_file.parent().to_owned(),
            state: Default::default(),
        }
    }
}

type FileWatcher = Debouncer<RecommendedWatcher, RecommendedCache>;

/// Spawn a file system watcher that watches the collection file for changes.
/// When it changes, trigger a collection reload by sending a message. **The
/// watcher will stop when it is dropped, so hang onto the return value!!**
fn watch_collection(
    path: &Path,
    messages_tx: MessageSender,
) -> notify::Result<FileWatcher> {
    /// Should this event trigger a reload?
    fn should_reload(event: &DebouncedEvent) -> bool {
        // Only reload if the file is modified. Some editors may truncrate
        // and recreate files instead of modifying
        // https://docs.rs/notify/latest/notify/#editor-behaviour
        // Modify/create type is useless on Windows
        // https://github.com/notify-rs/notify/issues/633
        matches!(
            event.event.kind,
            notify::EventKind::Modify(_) | notify::EventKind::Create(_),
        )
    }

    let on_file_event = move |result: DebounceEventResult| {
        match result {
            Ok(events) if events.iter().any(should_reload) => {
                info!(?events, "Collection file changed, reloading");
                messages_tx.send(Message::CollectionStartReload);
            }
            // Do nothing for other event kinds
            Ok(_) => {}
            Err(errors) => {
                error!(?errors, "Error watching file");
            }
        }
    };

    // Spawn the watcher
    let mut debouncer = notify_debouncer_full::new_debouncer(
        // Collection loading is very fast so we can use a short debounce. If
        // the user is saving several times rapidly, we can afford to reload
        // after each one. We just want to batch together related events that
        // happen simultaneously
        Duration::from_millis(100),
        None,
        on_file_event,
    )?;
    debouncer.watch(path, RecursiveMode::NonRecursive)?;
    info!(path = ?path, ?debouncer, "Watching file for changes");
    Ok(debouncer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TestHarness, harness};
    use rstest::rstest;
    use slumber_util::{TempDir, assert_matches, temp_dir};
    use std::fs;

    /// Load a collection, then change the file to trigger a reload
    #[rstest]
    #[tokio::test]
    async fn test_reload_collection(
        temp_dir: TempDir,
        mut harness: TestHarness,
    ) {
        // Start with an empty collection
        let file = collection_file(&temp_dir);

        let mut state = TuiState::load(
            harness.database.clone(),
            file.clone(),
            harness.messages_tx().clone(),
        );
        // Make sure it loaded correctly
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 0);
        // Collection name should be set in the DB
        assert_eq!(
            harness.database.metadata().unwrap().name.as_deref(),
            Some("Test")
        );

        // Update the file, make sure it's reflected
        fs::write(
            file.path(),
            r#"
name: Test Reloaded

requests:
    test:
        method: "GET"
        url: "test"
"#,
        )
        .unwrap();

        // We need to manually plumb messages through. Normally the TUI loop
        // does this
        let message = harness.pop_message_wait().await;
        assert_matches!(message, Message::CollectionStartReload);
        state.handle_message(message).unwrap();
        let message = harness.pop_message_wait().await;
        assert_matches!(message, Message::CollectionEndReload { .. });
        state.handle_message(message).unwrap();

        // And it's done!
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 1);
        // Name was updatd too
        assert_eq!(
            harness.database.metadata().unwrap().name.as_deref(),
            Some("Test Reloaded")
        );
    }

    /// Test an error in the collection during initial load. Should shove us
    /// into an error state. After fixing the error, it will reload with the
    /// valid collection.
    #[rstest]
    #[tokio::test]
    async fn test_initial_load_error(
        temp_dir: TempDir,
        mut harness: TestHarness,
    ) {
        // Start with an invalid collection
        let file = collection_file(&temp_dir);
        fs::write(file.path(), "requests: 3").unwrap();

        // Should load into an error state
        let mut state = TuiState::load(
            harness.database.clone(),
            file.clone(),
            harness.messages_tx().clone(),
        );
        assert_matches!(&state.0, TuiStateInner::Error { error, .. });

        // Update the file, make sure it's reflected
        fs::write(file.path(), "requests: {}").unwrap();

        // We need to manually plumb messages through. Normally the TUI loop
        // does this
        let message = harness.pop_message_wait().await;
        assert_matches!(message, Message::CollectionStartReload);
        state.handle_message(message).unwrap();
        // The error state loads the collection in the main thread so there's no
        // CollectionEndReload message

        // And it's done!
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 0);
    }

    /// Collection is loaded successfully on startup, but then changed to have
    /// an error. The old collection should remain in use but the error is
    /// shown.
    #[rstest]
    #[tokio::test]
    async fn test_reload_error(temp_dir: TempDir, mut harness: TestHarness) {
        // Start with an empty collection
        let file = collection_file(&temp_dir);

        let mut state = TuiState::load(
            harness.database.clone(),
            file.clone(),
            harness.messages_tx().clone(),
        );
        // Make sure it loaded correctly
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 0);

        // Update the file with an invalid colletion
        fs::write(file.path(), "requests: 3").unwrap();

        // We need to manually plumb messages through. Normally the TUI loop
        // does this
        let message = harness.pop_message_wait().await;
        assert_matches!(message, Message::CollectionStartReload);
        state.handle_message(message).unwrap();
        // Load failed!!
        let message = harness.pop_message_wait().await;
        assert_matches!(message, Message::Error { .. });
        state.handle_message(message).unwrap();

        // We remain in valid mode with the original collection
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 0);
    }

    /// Switch the selected request, which should rebuild the state entirely
    #[rstest]
    #[tokio::test]
    async fn test_collection_switch(temp_dir: TempDir, harness: TestHarness) {
        // Start with an empty collection
        let file = collection_file(&temp_dir);

        // Create a second collection
        let other_collection = temp_dir.join("other_slumber.yml");
        fs::write(
            &other_collection,
            r#"requests: {"r1": {"method": "GET", "url": "http://localhost"}}"#,
        )
        .unwrap();

        let mut state = TuiState::load(
            harness.database.clone(),
            file.clone(),
            harness.messages_tx().clone(),
        );
        // Make sure it loaded correctly
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 0);

        state
            .handle_message(Message::CollectionSelect(other_collection.clone()))
            .unwrap();
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 1);
    }

    /// Get a path to a collection file in a directory. The file will be created
    /// with an empty collection
    fn collection_file(directory: &Path) -> CollectionFile {
        let path = directory.join("slumber.yml");
        fs::write(&path, "name: Test").unwrap();
        CollectionFile::new(Some(path)).unwrap()
    }
}
