use crate::{
    http::{RequestConfig, RequestStore},
    message::{Message, MessageSender},
    view::{ComponentMap, Event, InvalidCollection, UpdateContext, View},
};
use anyhow::anyhow;
use ratatui::buffer::Buffer;
use slumber_config::Config;
use slumber_core::{
    collection::{Collection, CollectionError, CollectionFile},
    database::{CollectionDatabase, Database},
};
use std::sync::Arc;

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
    pub collection: CollectionResult,
    /// Handle for the file from which the collection will be loaded
    pub collection_file: CollectionFile,
    /// A map of all components drawn in the most recent draw phase
    pub component_map: ComponentMap,
    /// Persistence database for the current collection
    ///
    /// Stores request history, UI state, etc.
    pub database: CollectionDatabase,
    /// In-memory store of request state. This tracks state for requests
    /// that are in progress, and also serves as a cache for requests from
    /// the DB.
    pub request_store: RequestStore,
    /// UI presentation and state
    pub view: View,

    // Private state - we hang onto this stuff so we can use it to rebuild the
    // view. They should never change between reloads
    config: Arc<Config>,
    messages_tx: MessageSender,
}

impl CollectionState {
    /// Load the collection from the given file. If the load fails, we'll enter
    /// the error state.
    pub fn load(
        config: Arc<Config>,
        collection_file: CollectionFile,
        database: Database,
        messages_tx: MessageSender,
    ) -> Self {
        // If we fail to get a DB handle, there's no way to proceed
        let database = database.into_collection(&collection_file).unwrap();
        let request_store = RequestStore::new(database.clone());

        let collection = map_result(collection_file.load(), &database);

        let view = View::new(
            config.clone(),
            to_view_result(&collection_file, &collection),
            database.clone(),
            messages_tx.clone(),
        );

        Self {
            collection,
            collection_file,
            component_map: ComponentMap::default(),
            database,
            request_store,
            view,
            config,
            messages_tx,
        }
    }

    /// Spawn a background task to load+parse the current collection file
    ///
    /// When the load is done, [Message::CollectionEndReload] will be sent with
    /// the result (`Ok` or `Err`) of the load. Pass that result back to
    /// [Self::set_collection].
    ///
    /// YAML parsing is CPU-bound so do it in a blocking task. In all likelihood
    /// this will be extremely fast, but it's possible there's some edge case
    /// that causes it to be slow and we don't want to block the render loop.
    pub fn reload_collection(&self) {
        let collection_file = self.collection_file.clone();
        self.messages_tx.spawn_blocking(
            move || collection_file.load(),
            // Collection either loaded or failed. Either way, refresh the
            // collection state with the result
            Message::CollectionEndReload,
        );
    }

    /// Switch to a new version of the current collection file
    ///
    /// This does *not* full rebuild state because the collection file hasn't
    /// changed. We can keep the DB, request store, etc.
    pub fn set_collection(
        &mut self,
        result: Result<Collection, CollectionError>,
    ) {
        self.collection = map_result(result, &self.database);

        // Rebuild the whole view, because tons of things can change
        self.view = View::new(
            self.config.clone(),
            to_view_result(&self.collection_file, &self.collection),
            self.database.clone(),
            self.messages_tx.clone(),
        );
        self.view.notify("Reloaded collection");
    }

    /// Update the view in response to a view event
    pub fn handle_event(&mut self, event: Event) {
        let context = UpdateContext {
            component_map: &self.component_map,
            request_store: &mut self.request_store,
        };
        self.view.handle_event(context, event);
        // Persist state after changes
        self.view.persist(self.database.clone());
    }

    /// Draw the view onto the screen
    pub fn draw(&mut self, buffer: &mut Buffer) {
        self.component_map = self.view.draw(buffer);
    }

    /// Get the current request config for the selected recipe. The config
    /// defines how to build a request. If no recipe is selected, this returns
    /// an error. This should only be called in contexts where we can safely
    /// assume that a recipe is selected (e.g. triggered via an action on a
    /// recipe), so an error indicates a bug.
    pub fn request_config(&self) -> anyhow::Result<RequestConfig> {
        self.view
            .request_config()
            .ok_or_else(|| anyhow!("No recipe selected"))
    }
}

/// The result of loading a collection. Both the collection and the error are in
/// `Arc` so they can be shared cheaply with the view.
type CollectionResult = Result<Arc<Collection>, Arc<CollectionError>>;

/// Map the direct result of loading a collection into a [CollectionResult],
/// and update the collection's name in the DB if `Ok`
fn map_result(
    result: Result<Collection, CollectionError>,
    database: &CollectionDatabase,
) -> CollectionResult {
    if let Ok(collection) = &result {
        // Update the DB with the collection's name
        database.set_name(collection);
    }

    result.map(Arc::new).map_err(Arc::new)
}

/// Clone the collection `Result` and map its error to the format the view wants
fn to_view_result(
    collection_file: &CollectionFile,
    collection: &CollectionResult,
) -> Result<Arc<Collection>, InvalidCollection> {
    collection.clone().map_err(|error| InvalidCollection {
        file: collection_file.clone(),
        error,
    })
}
