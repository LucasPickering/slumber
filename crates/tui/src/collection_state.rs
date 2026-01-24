use crate::{
    http::{RequestConfig, RequestStore},
    message::MessageSender,
    view::{
        ComponentMap, InvalidCollection, UpdateContext, View,
        persistent::PersistentStore,
    },
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
    pub collection: Result<Arc<Collection>, Arc<CollectionError>>,
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
        let view = View::new(
            config.clone(),
            view_collection,
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

    /// Switch to a new version of the current collection file
    pub fn set_collection(&mut self, collection: Collection) {
        let collection = Arc::new(collection);

        self.database.set_name(&collection);

        // Rebuild the whole view, because tons of things can change
        self.view = View::new(
            self.config.clone(),
            Ok(Arc::clone(&collection)),
            self.database.clone(),
            self.messages_tx.clone(),
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
