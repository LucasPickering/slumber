use crate::{
    context::TuiContext,
    http::{RequestConfig, RequestState, RequestStore, TuiHttpProvider},
    message::{
        Callback, HttpMessage, Message, MessageSender, RecipeCopyTarget,
    },
    util,
    view::{
        ComponentMap, InvalidCollection, PreviewPrompter, RequestDisposition,
        TuiPrompter, UpdateContext, View, persistent::PersistentStore,
    },
};
use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use ratatui::buffer::Buffer;
use slumber_core::{
    collection::{Collection, CollectionError, CollectionFile, ProfileId},
    database::{CollectionDatabase, Database},
    http::{Exchange, RequestError, RequestId, RequestSeed},
    render::{Prompter, TemplateContext},
};
use slumber_template::{RenderedOutput, Template};
use slumber_util::ResultTraced;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Collection-specific top-level state
///
/// This encapsulates everything that should change when the collection changes.
///
/// The decision between what goes in here and what goes in the parent struct is
/// simple: if it should be rebuilt when switching collection files, it goes in
/// here. If not, take it upstairs.
#[derive(Debug)]
pub struct CollectionState {
    /// Result of loading and deserializing the request collection
    ///
    /// If the collection loading failed, we store the error here so we can
    /// show it in the view. We'll display the error until the user fixes the
    /// issue or exits.
    ///
    /// Both variants are wrapped in an `Arc` so we can share them cheaply with
    /// the view.
    pub collection: Result<Arc<Collection>, Arc<CollectionError>>,
    /// Handle for the file from which the collection will be loaded
    pub collection_file: CollectionFile,
    /// A map of all components drawn in the most recent draw phase
    pub component_map: ComponentMap,
    /// Persistence database for the current collection
    ///
    /// Stores request history, UI state, etc.
    pub database: CollectionDatabase,
    /// Message tx channel
    ///
    /// This isn't actually collection-specific, but holding it here allows us
    /// to handling a lot of operations on behalf of the root struct
    pub messages_tx: MessageSender,
    /// In-memory store of request state. This tracks state for requests
    /// that are in progress, and also serves as a cache for requests from
    /// the DB.
    pub request_store: RequestStore,
    /// UI presentation and state
    pub view: View,
}

impl CollectionState {
    /// Load the collection from the given file. If the load fails, we'll enter
    /// the error state.
    pub fn load(
        collection_file: CollectionFile,
        database: Database,
        messages_tx: MessageSender,
    ) -> Self {
        // If we fail to get a DB handle, there's no way to proceed
        let database = database.into_collection(&collection_file).unwrap();
        let request_store = RequestStore::new(database.clone());

        // Wrap the collection in Arc so it can be shared cheaply
        let collection = collection_file.load().map(Arc::new).map_err(Arc::new);
        if let Ok(collection) = &collection {
            // Update the DB with the collection's name
            database.set_name(collection);
        }

        let view_collection =
            collection.clone().map_err(|error| InvalidCollection {
                file: collection_file.clone(),
                error,
            });
        let view =
            View::new(view_collection, database.clone(), messages_tx.clone());

        Self {
            collection,
            collection_file,
            component_map: ComponentMap::default(),
            database,
            messages_tx,
            request_store,
            view,
        }
    }

    /// Switch to a new version of the current collection file
    pub fn set_collection(
        &mut self,
        collection: Collection,
        messages_tx: MessageSender,
    ) {
        let collection = Arc::new(collection);

        self.database.set_name(&collection);

        // Rebuild the whole view, because tons of things can change
        self.view = View::new(
            Ok(Arc::clone(&collection)),
            self.database.clone(),
            messages_tx,
        );
        self.view.notify("Reloaded collection");

        self.collection = Ok(collection);
    }

    /// Handle all events in the queue. Return `true` if at least one event was
    /// consumed, `false` if the queue was empty
    pub fn drain_events(&mut self) -> bool {
        let context = UpdateContext {
            component_map: &self.component_map,
            persistent_store: &mut PersistentStore::new(self.database.clone()),
            request_store: &mut self.request_store,
        };
        let handled = self.view.handle_events(context);
        // Persist state after changes
        if handled {
            self.view.persist(self.database.clone());
        }
        handled
    }

    /// Draw the view onto the screen
    pub fn draw(&mut self, buffer: &mut Buffer) {
        self.component_map = self.view.draw(buffer);
    }

    /// Handle an [HttpMessage]
    pub fn handle_http(&mut self, message: HttpMessage) -> anyhow::Result<()> {
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
        let messages_tx = self.messages_tx.clone();

        // Don't use spawn_result here, because errors are handled specially for
        // requests
        let cancel_token = CancellationToken::new();
        let future = async move {
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
        };
        self.messages_tx
            .spawn(util::cancellable(&cancel_token, future));

        // Add the new request to the store. This has to go after spawning the
        // task so we can include the join handle (for cancellation)
        self.request_store.start(
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
                let collection =
                    self.collection.as_ref().expect("Collection missing");

                // Persist in the DB if not disabled by global config or recipe
                let persist = TuiContext::get().config.tui.persist
                    && collection
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

    /// Copy some component of the current recipe. Depending on the target, this
    /// may require rendering some or all of the recipe
    pub fn copy_recipe(
        &mut self,
        target: RecipeCopyTarget,
    ) -> anyhow::Result<()> {
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
        let messages_tx = self.messages_tx.clone();
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
    pub fn save_response_body(
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
        self.messages_tx.spawn_result(util::save_file(
            self.messages_tx.clone(),
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

    /// Spawn a task to render a template, storing the result in a pre-defined
    /// lock. As this is a preview, the user will *not* be prompted for any
    /// input. A placeholder value will be used for any prompts.
    pub fn render_template_preview(
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
        let collection = self.collection.as_ref().expect("Collection missing");

        // If request_id is given, it's a request build. Otherwise it's a
        // preview
        let is_preview = request_id.is_none();
        let http_provider =
            TuiHttpProvider::new(self.messages_tx.clone(), is_preview);
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
            overrides: self.view.profile_overrides(),
            show_sensitive: !is_preview,
            root_dir: self.collection_file.parent().to_owned(),
            state: Default::default(),
        }
    }
}
