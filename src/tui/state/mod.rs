//! TUI state, which constitues the M in MVC. This does *not* include the
//! repository, which is kept separate to make sure all state is stored in
//! memory. The controller is responsible for bridging state with the
//! repository.

mod message;
mod ui;

pub use message::*;
pub use ui::*;

use crate::{
    config::{
        Chain, Profile, RequestCollection, RequestRecipe, RequestRecipeId,
    },
    http::RequestRecord,
    tui::{input::InputTarget, view::component::ErrorPopup},
};
use chrono::{DateTime, Duration, Utc};
use std::{
    collections::{hash_map, HashMap},
    ops::Deref,
    path::{Path, PathBuf},
};
use tracing::{error, info, trace};

/// Main app state. All configuration and UI state is stored here. The M in MVC.
/// There should be only one instance of this per TUI process, and it should
/// live the entire life of the process. Anything that needs to be reloaded
/// should live in [EphemeralState].
#[derive(Debug)]
pub struct AppState {
    // Global app state
    /// Flag to control the main app loop. Set to false to exit the app
    should_run: bool,
    /// The file that the current collection was loaded from. Needed in order
    /// to reload from it
    collection_file: PathBuf,
    /// Sender end of the message queue. Anything can use this to pass async
    /// messages back to the main thread to be handled. We use an unbounded
    /// sender because we don't ever expect the queue to get that large, and it
    /// allows for synchronous enqueueing.
    messages_tx: MessageSender,
    /// Don't get too attached to it...
    ephemeral: EphemeralState,
}

/// All the state that's dependent upon the collection. This gets recreated
/// whenever the collection is reloaded. Nothing in here is exposed directly,
/// it's only accessible via methods on [AppState].
#[derive(Debug)]
struct EphemeralState {
    /// Any error that should be shown to the user in a popup
    error: Option<anyhow::Error>,
    notification: Option<Notification>,
    /// Each recipe can have one request in flight at a time
    active_requests: HashMap<RequestRecipeId, RequestState>,
    /// We need this for template context. We dismantle the collection instance
    /// to build the UI state, so we need to store this separately
    chains: Vec<Chain>,
    /// The pane that the user has focused, which will receive input events
    /// UNLESS a high-priority popup is open
    selected_pane: StatefulSelect<PrimaryPane>,
    request_tab: StatefulSelect<RequestTab>,
    response_tab: StatefulSelect<ResponseTab>,
    profiles: StatefulList<Profile>,
    recipes: StatefulList<RequestRecipe>,
}

impl AppState {
    pub fn new(
        collection_file: PathBuf,
        collection: RequestCollection,
        messages_tx: impl Into<MessageSender>,
    ) -> Self {
        Self {
            should_run: true,
            collection_file,
            messages_tx: messages_tx.into(),
            ephemeral: EphemeralState::new(collection),
        }
    }

    /// Should the app keep running?
    pub fn should_run(&self) -> bool {
        self.should_run
    }

    /// Set the app to exit on next loop
    pub fn quit(&mut self) {
        self.should_run = false;
    }

    /// The file that the request collection was loaded from
    pub fn collection_file(&self) -> &Path {
        &self.collection_file
    }

    /// Get a clone of the message sender, which can be passed around between
    /// tasks
    pub fn messages_tx(&self) -> MessageSender {
        self.messages_tx.clone()
    }

    /// Recreate UI state based on a new request collection
    pub fn reload_collection(&mut self, collection: RequestCollection) {
        self.ephemeral = EphemeralState::new(collection);
    }

    /// Get the active local input handler, based on pane focus and popup state
    pub fn input_handler(&self) -> Box<dyn InputTarget> {
        match self.error() {
            Some(_) => Box::new(ErrorPopup),
            None => self.ephemeral.selected_pane.selected().input_handler(),
        }
    }

    /// Get the stored notification (if any).
    pub fn notification(&self) -> Option<&Notification> {
        match &self.ephemeral.notification {
            // If the notification is expired, don't show it
            Some(notification) if !notification.expired() => Some(notification),
            _ => None,
        }
    }

    /// Get whichever request state should currently be shown to the user,
    /// based on whichever recipe is selected.
    pub fn active_request(&self) -> Option<&RequestState> {
        let selected_recipe_id = &self.ephemeral.recipes.selected()?.id;
        let active_request =
            self.ephemeral.active_requests.get(selected_recipe_id);

        // If we don't have a request for this recipe, load the most recent from
        // the repository. It should be there by the next frame.
        if active_request.is_none() {
            self.messages_tx.send(Message::RepositoryStartLoad {
                recipe_id: selected_recipe_id.clone(),
            });
        }

        active_request
    }

    /// Can a request be sent for the currently selected recipe? Requests can
    /// only be sent if there isn't already one in progress.
    pub fn can_send_request(&self) -> bool {
        !self
            .active_request()
            .map(|request_state| request_state.is_loading())
            .unwrap_or_default()
    }

    /// Start a new HTTP request
    pub fn start_request(&mut self, recipe_id: RequestRecipeId) {
        let state = RequestState::Loading {
            start_time: Utc::now(),
        };
        // This shouldn't ever be called if there's already a pending request,
        // but just double check
        match self.ephemeral.active_requests.entry(recipe_id) {
            hash_map::Entry::Occupied(entry) if entry.get().is_loading() => {
                error!(
                    recipe = %entry.key(),
                    "Cannot set pending request for recipe, \
                    one is already in progress"
                )
            }
            hash_map::Entry::Occupied(mut entry) => {
                *entry.get_mut() = state;
            }
            hash_map::Entry::Vacant(entry) => {
                entry.insert(state);
            }
        }
    }

    /// Store response for a request
    pub fn finish_request(&mut self, record: RequestRecord) {
        let recipe_id = &record.request.recipe_id;
        match self.ephemeral.active_requests.get_mut(recipe_id) {
            Some(state) if state.is_loading() => {
                // We know this request corresponds to this response because we
                // only allow one request at a time for each recipe

                *state = RequestState::response(record);
            }
            other => {
                error!(
                    "Expected loading state for recipe {}, but got {:?}",
                    recipe_id, other
                );
            }
        }
    }

    /// Store error for a request
    pub fn fail_request(
        &mut self,
        recipe_id: &RequestRecipeId,
        err: anyhow::Error,
    ) {
        match self.ephemeral.active_requests.get_mut(recipe_id) {
            Some(state) if state.is_loading() => {
                // We know this request corresponds to this response because we
                // only allow one request at a time for each recipe
                *state = RequestState::Error {
                    error: err,
                    start_time: state.start_time(),
                    end_time: Utc::now(),
                };
            }
            other => {
                error!(
                    "Expected loading state for recipe {}, but got {:?}",
                    recipe_id, other
                );
            }
        }
    }

    /// Store a completed request that was loaded from the repository
    pub fn load_request(&mut self, record: RequestRecord) {
        // If the user spawned a request between when we started the load and
        // now, don't overwrite it
        self.ephemeral
            .active_requests
            .entry(record.request.recipe_id.clone())
            .or_insert(RequestState::response(record));
    }

    /// Show a notification to the user
    pub fn notify(&mut self, message: impl Into<String>) {
        let message: String = message.into();
        let timestamp = Utc::now();
        trace!(message, %timestamp, "Notification");
        self.ephemeral.notification = Some(Notification { message, timestamp })
    }

    /// Get the stored error (if any)
    pub fn error(&self) -> Option<&anyhow::Error> {
        self.ephemeral.error.as_ref()
    }

    /// Store an error in state, to be shown to the user
    pub fn set_error(&mut self, err: anyhow::Error) {
        error!(error = err.deref());
        self.ephemeral.error = Some(err);
    }

    /// Close the error popup
    pub fn clear_error(&mut self) {
        info!("Clearing error state");
        self.ephemeral.error = None;
    }
}

/// Put all the boring getters in their own block for organization. This is
/// tedious but it hides the existence of `EphemeralState` which is convenient
/// for the user.
impl AppState {
    pub fn chains(&self) -> &[Chain] {
        &self.ephemeral.chains
    }

    pub fn selected_pane(&self) -> &StatefulSelect<PrimaryPane> {
        &self.ephemeral.selected_pane
    }

    pub fn selected_pane_mut(&mut self) -> &mut StatefulSelect<PrimaryPane> {
        &mut self.ephemeral.selected_pane
    }

    pub fn request_tab(&self) -> &StatefulSelect<RequestTab> {
        &self.ephemeral.request_tab
    }

    pub fn request_tab_mut(&mut self) -> &mut StatefulSelect<RequestTab> {
        &mut self.ephemeral.request_tab
    }

    pub fn response_tab(&self) -> &StatefulSelect<ResponseTab> {
        &self.ephemeral.response_tab
    }

    pub fn response_tab_mut(&mut self) -> &mut StatefulSelect<ResponseTab> {
        &mut self.ephemeral.response_tab
    }

    pub fn profiles(&self) -> &StatefulList<Profile> {
        &self.ephemeral.profiles
    }

    pub fn profiles_mut(&mut self) -> &mut StatefulList<Profile> {
        &mut self.ephemeral.profiles
    }

    pub fn recipes(&self) -> &StatefulList<RequestRecipe> {
        &self.ephemeral.recipes
    }

    pub fn recipes_mut(&mut self) -> &mut StatefulList<RequestRecipe> {
        &mut self.ephemeral.recipes
    }
}

impl EphemeralState {
    fn new(collection: RequestCollection) -> Self {
        Self {
            error: None,
            notification: None,
            active_requests: HashMap::new(),
            selected_pane: StatefulSelect::new(),
            request_tab: StatefulSelect::new(),
            response_tab: StatefulSelect::new(),
            profiles: StatefulList::with_items(collection.profiles),
            recipes: StatefulList::with_items(collection.requests),
            chains: collection.chains,
        }
    }
}

/// State of an HTTP response, which can be pending or completed
#[derive(Debug)]
pub enum RequestState {
    /// Request is in flight, or is *about* to be sent. There's no way to
    /// initiate a request that doesn't immediately launch it, so Loading is
    /// the initial state.
    Loading { start_time: DateTime<Utc> },

    /// A resolved HTTP response, with all content loaded and ready to be
    /// displayed. This does *not necessarily* have a 2xx/3xx status code, any
    /// received response is considered a "success".
    Response {
        record: RequestRecord,
        pretty_body: Option<String>,
    },

    /// Error occurred sending the request or receiving the response.
    Error {
        error: anyhow::Error,
        start_time: DateTime<Utc>,
        /// When did the error occur?
        end_time: DateTime<Utc>,
    },
}

impl RequestState {
    pub fn is_loading(&self) -> bool {
        matches!(self, RequestState::Loading { .. })
    }

    /// When was the active request launched?
    pub fn start_time(&self) -> DateTime<Utc> {
        match self {
            Self::Loading { start_time, .. } => *start_time,
            Self::Response { record, .. } => record.start_time,
            Self::Error { start_time, .. } => *start_time,
        }
    }

    /// Elapsed time for the active request. If pending, this is a running
    /// total. Otherwise end time - start time.
    pub fn duration(&self) -> Duration {
        match self {
            Self::Loading { start_time, .. } => Utc::now() - start_time,
            Self::Response { record, .. } => record.duration(),
            Self::Error {
                start_time,
                end_time,
                ..
            } => *end_time - *start_time,
        }
    }

    /// Create a request state from a completed response.
    pub fn response(record: RequestRecord) -> Self {
        // Prettification might get slow on large responses, maybe we
        // want to punt this into a separate task?
        let pretty_body = record.response.prettify_body().ok();
        Self::Response {
            record,
            pretty_body,
        }
    }
}
