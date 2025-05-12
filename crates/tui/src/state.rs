use crate::{
    context::TuiContext,
    http::{RequestState, RequestStore, TuiHttpProvider},
    message::{Callback, Message, MessageSender, RequestConfig},
    util::{self, ResultReported},
    view::{PreviewPrompter, TuiPrompter, UpdateContext, View},
};
use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use itertools::Itertools;
use notify::RecommendedWatcher;
use petitscript::{Process, Source, Value};
use ratatui::Frame;
use slumber_core::{
    collection::{Collection, CollectionFile, ProfileId},
    database::{CollectionDatabase, Database},
    http::{Exchange, RequestError, RequestId, RequestSeed},
    render::{Overrides, Procedure, Prompter, RenderContext, Renderer},
};
use slumber_util::ResultTraced;
use std::{path::Path, sync::Arc};
use tokio::task;
use tracing::info;

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
    /// Panic if the database fails to load, or if the collection fails to load
    /// and we're not able to start a file watcher on it.
    pub fn load(
        collection_file: CollectionFile,
        messages_tx: MessageSender,
    ) -> Self {
        let (collection, process) = match collection_file.load() {
            Ok((collection, process)) => (collection, process),
            Err(error) => {
                return {
                    // Collection failed to load. Store the error so we can show
                    // it to the user, and watch the file for changes so they
                    // don't have to restart the TUI once it's fixed
                    let Ok(watcher) = watch_collection(
                        &[collection_file.path()],
                        messages_tx.clone(),
                    ) else {
                        // If the watcher fails to initialize, there's no point
                        // in sitting in this error state. Show the original
                        // collection error. The watcher error is much less
                        // useful. It's accessible in the logs if needed.
                        panic!("{error:#}")
                    };
                    Self(TuiStateInner::Error {
                        collection_file,
                        error,
                        messages_tx,
                        _watcher: watcher,
                    })
                };
            }
        };

        // Load a database for this particular collection
        let database = Database::load()
            .and_then(|database| database.into_collection(&collection_file))
            // If the DB fails to load, the whole TUI won't work so we should
            // just crash. We could return the error, but then the caller has
            // to distinguish between collection errors (which may be
            // recoverable) and DB errors which are fatal
            .context("Error initializing request database")
            .unwrap();

        let collection = Arc::new(collection);
        let request_store = RequestStore::new(database.clone());
        let view =
            View::new(&collection, database.clone(), messages_tx.clone());
        let watcher =
            watch_collection(&get_source_paths(&process), messages_tx.clone())
                .ok();

        Self(TuiStateInner::Loaded(LoadedState {
            collection_file,
            collection,
            database,
            messages_tx,
            process,
            request_store,
            view,
            _watcher: watcher,
        }))
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
        match &mut self.0 {
            TuiStateInner::Loaded(state) => state.handle_message(message),
            // Nothing to do in the error state
            TuiStateInner::Error {
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
                state.view.draw(frame, &state.request_store)
            }
            TuiStateInner::Error {
                collection_file,
                error,
                ..
            } => {
                // We can't show the real UI without a collection, so just show
                // the error. We have a watcher on the file so when it changes,
                // we'll reload it
                View::draw_collection_load_error(frame, collection_file, error)
            }
        }
    }
}

/// Inner enum for [TuiState] to avoid exposing the variant values
#[derive(Debug)]
enum TuiStateInner {
    /// TUI loaded successfully and is off and running
    Loaded(LoadedState),
    /// Collection failed to load on startup. Without a collection we can't
    /// show the UI. We'll just sit and wait for it to come back.
    Error {
        collection_file: CollectionFile,
        /// Error that occurred while loading the collection
        error: anyhow::Error,
        messages_tx: MessageSender,
        /// Watch the collection file and wait for changes that will hopefully
        /// fix it. We have to hang onto this because watching stops when it's
        /// dropped.
        _watcher: RecommendedWatcher,
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
    /// PS process in which the collection file was loaded. We'll use this to
    /// execute render functions from the collection. This is `None` iff the
    /// collection failed to load. In that case, it should be impossible to
    /// trigger any logic that requires a process. That means we can just
    /// unwrap this when it's needed.
    process: Process,
    /// In-memory store of request state. This tracks state for requests that
    /// are in progress, and also serves as a cache for requests from the DB.
    request_store: RequestStore,
    /// UI presentation and state
    view: View,

    /// Watcher for changes to the collection file. This will watch the entire
    /// source tree of the collection, i.e. the root collection file and any
    /// other files it imports. PS is designed to have minimal source trees
    /// with no third party dependencies, so this should be very few files.
    ///
    /// Whenever any file changes, the collection will be reloaded, which will
    /// kill this watcher and start a new one. We need a new watcher because
    /// the set of files in the tree may change. The watcher never needs to be
    /// accessed after init, but we have to hang onto it because when it's
    /// dropped, it stops running.
    ///
    /// This will be `None` iff the watcher fails to initialize
    _watcher: Option<RecommendedWatcher>,
}

impl LoadedState {
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
                // These clones are cheap and necessary
                let file = self.collection_file.clone();
                let messages_tx = self.messages_tx();
                task::spawn_blocking(move || match file.load() {
                    Ok((collection, process)) => {
                        messages_tx.send(Message::CollectionEndReload {
                            collection,
                            process,
                        });
                    }
                    // Show the error in the UI
                    Err(error) => messages_tx.send(Message::Error { error }),
                });
            }
            Message::CollectionEndReload {
                collection,
                process,
            } => self.reload_collection(collection, process),
            Message::CollectionEdit => {
                let path = self.collection_file.path().to_owned();
                let command = util::get_editor_command(&path)?;
                self.messages_tx.send(Message::Command(command));
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

            Message::FileEdit { path, on_complete } => {
                let command = util::get_editor_command(&path)?;
                self.messages_tx.send(Message::Command(command));
                on_complete(path);
                // The callback may queue an event to read the file, so we can't
                // delete it yet. Caller is responsible for cleaning up
            }
            Message::FileView { path, mime } => {
                let command = util::get_pager_command(&path, mime.as_ref())?;
                self.messages_tx.send(Message::Command(command));
                // We don't need to read the contents back so we can clean up
                util::delete_temp_file(&path);
            }

            Message::Error { error } => self.view.open_modal(error),

            // Manage HTTP life cycle
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
            Message::PromptStart(prompt) => {
                self.view.open_modal(prompt);
            }
            Message::SelectStart(select) => {
                self.view.open_modal(select);
            }
            Message::ConfirmStart(confirm) => {
                self.view.open_modal(confirm);
            }

            Message::Preview {
                procedure,
                on_complete,
            } => {
                self.render_preview(
                    procedure,
                    // Note: there's a potential bug here, if the selected
                    // profile changed since this message was queued. In
                    // practice is extremely unlikely (potentially impossible),
                    // and this shortcut saves us a lot of plumbing so it's
                    // worth it
                    self.view.selected_profile_id().cloned(),
                    on_complete,
                )?;
            }

            // All other messages are handled by the root TUI and should never
            // get here
            Message::Command(_) | Message::Draw | Message::Quit => {
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
    fn reload_collection(&mut self, collection: Collection, process: Process) {
        // Kick off a new watcher, since the set of files in the source tree may
        // have changed
        let watcher =
            watch_collection(&get_source_paths(&process), self.messages_tx())
                .ok();
        self.collection = collection.into();
        self.process = process;
        self._watcher = watcher; // Dropping the old watcher will stop it

        // Rebuild the whole view, because tons of things can change
        self.view = View::new(
            &self.collection,
            self.database.clone(),
            self.messages_tx(),
        );
        self.view.notify(format!(
            "Reloaded collection from {}",
            self.collection_file.path().to_string_lossy()
        ));
    }

    /// Render URL for a request, then copy it to the clipboard
    fn copy_request_url(&self) -> anyhow::Result<()> {
        let RequestConfig {
            profile_id,
            recipe_id,
            overrides,
        } = self.request_config()?;
        let seed = RequestSeed::new(recipe_id);
        let messages_tx = self.messages_tx();
        let renderer = self.renderer(profile_id, overrides, false)?;
        // Spawn a task to do the render+copy
        util::spawn_result(messages_tx.clone(), async move {
            let url = TuiContext::get()
                .http_engine
                .build_url(seed, &renderer)
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
            overrides,
        } = self.request_config()?;
        let seed = RequestSeed::new(recipe_id);
        let renderer = self.renderer(profile_id, overrides, false)?;
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        util::spawn_result(messages_tx.clone(), async move {
            let body = TuiContext::get()
                .http_engine
                .build_body(seed, &renderer)
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
            overrides,
        } = self.request_config()?;
        let seed = RequestSeed::new(recipe_id);
        let renderer = self.renderer(profile_id, overrides, false)?;
        let messages_tx = self.messages_tx();
        // Spawn a task to do the render+copy
        util::spawn_result(messages_tx.clone(), async move {
            let command = TuiContext::get()
                .http_engine
                .build_curl(seed, &renderer)
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
        util::spawn_result(
            self.messages_tx(),
            util::save_file(self.messages_tx(), default_path, data),
        );
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
            overrides,
        } = self.request_config()?;
        // Launch the request in a separate task so it doesn't block.
        // These clones are all cheap.

        let renderer = self.renderer(profile_id.clone(), overrides, false)?;
        let messages_tx = self.messages_tx();

        let seed = RequestSeed::new(recipe_id.clone());
        let request_id = seed.id;

        // Don't use spawn_result here, because errors are handled specially for
        // requests
        let join_handle = util::spawn(messages_tx.clone(), async move {
            // Build the request
            let result =
                TuiContext::get().http_engine.build(seed, &renderer).await;
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
                let persist = TuiContext::get().config.persist
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

    /// Spawn a task to render a procedure, storing the result in a pre-defined
    /// lock. As this is a preview, the user will *not* be prompted for any
    /// input. A placeholder value will be used for any prompts.
    fn render_preview(
        &self,
        procedure: Procedure,
        profile_id: Option<ProfileId>,
        on_complete: Callback<Result<Value, ()>>,
    ) -> anyhow::Result<()> {
        let renderer = self.renderer(profile_id, Overrides::default(), true)?;
        util::spawn(self.messages_tx(), async move {
            // Send an empty error to the caller so it can show an
            // inline error message. The error will be traced, but never
            // shown in a modal because that would be disruptive.
            let result = renderer
                .render::<Value>(&procedure)
                .await
                .traced()
                .map_err(|_| ());
            on_complete(result);
        });
        Ok(())
    }

    /// Build a renderer. Most of the data has to be cloned out to be passed
    /// across async boundaries. This is annoying but in reality it should be
    /// small data.
    fn renderer(
        &self,
        profile_id: Option<ProfileId>,
        overrides: Overrides,
        is_preview: bool,
    ) -> anyhow::Result<Renderer> {
        let collection = &self.collection;
        let http_provider =
            TuiHttpProvider::new(self.messages_tx(), is_preview);
        let prompter: Box<dyn Prompter> = if is_preview {
            Box::new(PreviewPrompter)
        } else {
            Box::new(TuiPrompter::new(self.messages_tx()))
        };

        let context = RenderContext {
            selected_profile: profile_id,
            collection: collection.clone(),
            http_provider: Box::new(http_provider),
            prompter,
            overrides,
            show_sensitive: !is_preview,
        };
        Ok(Renderer::new(self.process.clone(), context))
    }
}

/// Get a list of all source files loaded by the process
fn get_source_paths(process: &Process) -> Vec<&Path> {
    process.sources().filter_map(Source::path).collect_vec()
}

/// Spawn a file system watcher that watches all source files for the
/// collection. If any of them change, trigger a collection reload by sending a
/// message.
fn watch_collection(
    paths: &[&Path],
    messages_tx: MessageSender,
) -> notify::Result<RecommendedWatcher> {
    util::watch_files(paths, move |event| {
        info!(?event, "Collection file changed, reloading");
        messages_tx.send(Message::CollectionStartReload);
    })
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

        let mut state =
            TuiState::load(file.clone(), harness.messages_tx().clone());
        // Make sure it loaded correctly
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 0);

        // Update the file, make sure it's reflected
        fs::write(
            file.path(),
            r#"export const requests = {
                test: {
                    type: "request",
                    method: "GET",
                    url: 'test',
                },
            };"#,
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
        fs::write(file.path(), "export requests = [];").unwrap();

        // Should load into an error state
        let mut state =
            TuiState::load(file.clone(), harness.messages_tx().clone());
        assert_matches!(&state.0, TuiStateInner::Error { error, .. });

        // Update the file, make sure it's reflected
        fs::write(file.path(), "export const requests = {};").unwrap();

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

        let mut state =
            TuiState::load(file.clone(), harness.messages_tx().clone());
        // Make sure it loaded correctly
        let collection = assert_matches!(
            &state.0,
            TuiStateInner::Loaded(LoadedState { collection, ..}) => collection,
        );
        assert_eq!(collection.recipes.iter().count(), 0);

        // Update the file with an invalid colletion
        fs::write(file.path(), "export const requests = [1, 2];").unwrap();

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

    /// Get a path to a collection file in a directory. The file will be created
    /// with an empty collection
    fn collection_file(directory: &Path) -> CollectionFile {
        let path = directory.join("slumber.js");
        fs::write(&path, "export const requests = {};").unwrap();
        CollectionFile::new(Some(path)).unwrap()
    }
}
