//! TUI state, which constitues the M in MVC. This does *not* include the
//! repository, which is kept separate to make sure all state is stored in
//! memory. The controller is responsible for bridging state with the
//! repository.

use crate::{
    config::{
        Chain, Profile, RequestCollection, RequestRecipe, RequestRecipeId,
    },
    http::RequestRecord,
    tui::{
        input::InputTarget,
        view::{
            ErrorPopup, ProfileListPane, RecipeListPane, RequestPane,
            ResponsePane,
        },
    },
};
use chrono::{DateTime, Duration, Utc};
use derive_more::{Display, From};
use ratatui::widgets::*;
use std::{
    cell::RefCell,
    collections::{hash_map, HashMap},
    fmt::Display,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};
use strum::{EnumIter, IntoEnumIterator};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info, trace};

/// Main app state. All configuration and UI state is stored here. The M in MVC
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
    pub messages_tx: MessageSender,
    /// All the stuff that directly affects what's shown on screen
    pub ui: UiState,
}

/// UI-related state fields. This is separated so it can be recreated when the
/// request collection is reloaded.
#[derive(Debug)]
pub struct UiState {
    /// Any error that should be shown to the user in a popup
    error: Option<anyhow::Error>,
    notification: Option<Notification>,
    /// Each recipe can have one request in flight at a time
    active_requests: HashMap<RequestRecipeId, RequestState>,
    /// The pane that the user has focused, which will receive input events
    /// UNLESS a high-priority popup is open
    pub selected_pane: StatefulSelect<PrimaryPane>,
    pub request_tab: StatefulSelect<RequestTab>,
    pub response_tab: StatefulSelect<ResponseTab>,
    pub profiles: StatefulList<Profile>,
    pub recipes: StatefulList<RequestRecipe>,
    pub chains: Vec<Chain>,
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
            ui: UiState::new(collection),
        }
    }

    /// The file that the request collection was loaded from
    pub fn collection_file(&self) -> &Path {
        &self.collection_file
    }

    /// Recreate UI state based on a new request collection
    pub fn reload_collection(&mut self, collection: RequestCollection) {
        self.ui = UiState::new(collection);
    }

    /// Should the app keep running?
    pub fn should_run(&self) -> bool {
        self.should_run
    }

    /// Set the app to exit on next loop
    pub fn quit(&mut self) {
        self.should_run = false;
    }

    /// Get the active local input handler, based on pane focus and popup state
    pub fn input_handler(&self) -> Box<dyn InputTarget> {
        match self.error() {
            Some(_) => Box::new(ErrorPopup),
            None => self.ui.selected_pane.selected().input_handler(),
        }
    }

    /// Get the stored notification (if any).
    pub fn notification(&self) -> Option<&Notification> {
        match &self.ui.notification {
            // If the notification is expired, don't show it
            Some(notification) if !notification.expired() => Some(notification),
            _ => None,
        }
    }

    /// Get whichever request state should currently be shown to the user,
    /// based on whichever recipe is selected.
    pub fn active_request(&self) -> Option<&RequestState> {
        let selected_recipe_id = &self.ui.recipes.selected()?.id;
        let active_request = self.ui.active_requests.get(selected_recipe_id);

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
        match self.ui.active_requests.entry(recipe_id) {
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
        match self.ui.active_requests.get_mut(recipe_id) {
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
        match self.ui.active_requests.get_mut(recipe_id) {
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
        self.ui
            .active_requests
            .entry(record.request.recipe_id.clone())
            .or_insert(RequestState::response(record));
    }

    /// Show a notification to the user
    pub fn notify(&mut self, message: impl Into<String>) {
        let message: String = message.into();
        let timestamp = Utc::now();
        trace!(message, %timestamp, "Notification");
        self.ui.notification = Some(Notification { message, timestamp })
    }

    /// Get the stored error (if any)
    pub fn error(&self) -> Option<&anyhow::Error> {
        self.ui.error.as_ref()
    }

    /// Store an error in state, to be shown to the user
    pub fn set_error(&mut self, err: anyhow::Error) {
        error!(error = err.deref());
        self.ui.error = Some(err);
    }

    /// Close the error popup
    pub fn clear_error(&mut self) {
        info!("Clearing error state");
        self.ui.error = None;
    }
}

impl UiState {
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

/// Wrapper around a sender for async messages. Cheap to clone and pass around
#[derive(Clone, Debug, From)]
pub struct MessageSender(UnboundedSender<Message>);

impl MessageSender {
    /// Send an async message, to be handled by the main loop
    pub fn send(&self, message: Message) {
        trace!(%message, "Queueing message");
        self.0.send(message).expect("Message queue is closed")
    }
}

/// A message triggers some *asynchronous* action. Most state modifications can
/// be made synchronously by the input handler, but some require async handling
/// at the top level. The controller is responsible for both triggering and
/// handling messages.
#[derive(Debug, Display)]
pub enum Message {
    /// Trigger collection reload
    CollectionStartReload,
    /// Store a reloaded collection value in state
    #[display(fmt = "EndReloadCollection(collection_file:?)")]
    CollectionEndReload {
        collection_file: PathBuf,
        collection: RequestCollection,
    },

    /// Launch an HTTP request from the currently selected recipe. Errors if
    /// the recipe list is empty.
    HttpSendRequest,
    /// We received an HTTP response
    #[display(
        fmt = "HttpResponse(id={}, status={})",
        "record.id()",
        "record.response.status"
    )]
    HttpResponse { record: RequestRecord },
    #[display(fmt = "HttpError(recipe={}, error={})", recipe_id, error)]
    HttpError {
        recipe_id: RequestRecipeId,
        error: anyhow::Error,
    },

    /// Load the most recent response for a recipe from the repository
    RepositoryStartLoad { recipe_id: RequestRecipeId },
    /// TODO
    #[display(fmt = "RepositoryEndLoad(id={})", "record.id()")]
    RepositoryEndLoad { record: RequestRecord },

    /// An error occurred in some async process and should be shown to the user
    Error { error: anyhow::Error },
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

/// A notification is an ephemeral informational message generated by some async
/// action. It doesn't grab focus, but will be useful to the user nonetheless.
/// It should be shown for a short period of time, then disappear on its own.
#[derive(Debug)]
pub struct Notification {
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

impl Notification {
    /// Amount of time a notification stays on screen before disappearing
    const NOTIFICATION_DECAY: Duration = Duration::milliseconds(5000);

    /// Has this notification overstayed its welcome?
    fn expired(&self) -> bool {
        Utc::now() - self.timestamp >= Self::NOTIFICATION_DECAY
    }
}

// TODO move some of these to a submodule

/// A list of items in the UI
#[derive(Debug)]
pub struct StatefulList<T> {
    /// Use interior mutability because this needs to be modified during the
    /// draw phase, by [Frame::render_stateful_widget]. This means we don't
    /// have to pass a mutable reference to [AppState] everywhere during
    /// the draw phase just so list state can be modified.
    state: RefCell<ListState>,
    pub items: Vec<T>,
}

impl<T> StatefulList<T> {
    pub fn with_items(items: Vec<T>) -> StatefulList<T> {
        let mut state = ListState::default();
        // Pre-select the first item if possible
        if !items.is_empty() {
            state.select(Some(0));
        }
        StatefulList {
            state: RefCell::new(state),
            items,
        }
    }

    /// Get the currently selected item (if any)
    pub fn selected(&self) -> Option<&T> {
        self.items.get(self.state.borrow().selected()?)
    }

    /// Get a mutable reference to state. This uses `RefCell` underneath so it
    /// will panic if aliased. Only call this during the draw phase!
    pub fn state_mut(&self) -> impl DerefMut<Target = ListState> + '_ {
        self.state.borrow_mut()
    }

    /// Select the previous item in the list. This should only be called during
    /// the message phase, so we can take `&mut self`.
    pub fn previous(&mut self) {
        let state = self.state.get_mut();
        let i = match state.selected() {
            Some(i) => {
                // Avoid underflow here
                if i == 0 {
                    self.items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        state.select(Some(i));
    }

    /// Select the next item in the list. This should only be called during the
    /// message phase, so we can take `&mut self`.
    pub fn next(&mut self) {
        let state = self.state.get_mut();
        let i = match state.selected() {
            Some(i) => (i + 1) % self.items.len(),
            None => 0,
        };
        state.select(Some(i));
    }
}

/// A fixed-size collection of selectable items, e.g. panes or tabs. User can
/// cycle between them.
#[derive(Debug)]
pub struct StatefulSelect<T: FixedSelect> {
    values: Vec<T>,
    selected: usize,
}

/// Friendly little trait indicating a type can be cycled through, e.g. a set
/// of panes or tabs
pub trait FixedSelect: Display + IntoEnumIterator + PartialEq {
    /// Initial item to select
    const DEFAULT_INDEX: usize = 0;
}

impl<T: FixedSelect> StatefulSelect<T> {
    pub fn new() -> Self {
        let values: Vec<T> = T::iter().collect();
        if values.is_empty() {
            panic!("Cannot create StatefulSelect from empty values");
        }
        Self {
            values,
            selected: T::DEFAULT_INDEX,
        }
    }

    /// Get the index of the selected element
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Get the selected element
    pub fn selected(&self) -> &T {
        &self.values[self.selected]
    }

    /// Is the given item selected?
    pub fn is_selected(&self, item: &T) -> bool {
        self.selected() == item
    }

    /// Select previous item
    pub fn previous(&mut self) {
        // Prevent underflow
        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or(self.values.len() - 1);
    }

    /// Select next item
    pub fn next(&mut self) {
        self.selected = (self.selected + 1) % self.values.len();
    }
}

impl<T: FixedSelect> Default for StatefulSelect<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Copy, Clone, Debug, derive_more::Display, EnumIter, PartialEq)]
pub enum PrimaryPane {
    #[display(fmt = "Profiles")]
    ProfileList,
    #[display(fmt = "Recipes")]
    RecipeList,
    Request,
    Response,
}

impl PrimaryPane {
    /// Get a trait object that should handle contextual input for this pane
    pub fn input_handler(self) -> Box<dyn InputTarget> {
        match self {
            Self::ProfileList => Box::new(ProfileListPane),
            Self::RecipeList => Box::new(RecipeListPane),
            Self::Request => Box::new(RequestPane),
            Self::Response => Box::new(ResponsePane),
        }
    }
}

impl FixedSelect for PrimaryPane {
    const DEFAULT_INDEX: usize = 1;
}

#[derive(Copy, Clone, Debug, derive_more::Display, EnumIter, PartialEq)]
pub enum RequestTab {
    Body,
    Query,
    Headers,
}

impl FixedSelect for RequestTab {}

#[derive(Copy, Clone, Debug, derive_more::Display, EnumIter, PartialEq)]
pub enum ResponseTab {
    Body,
    Headers,
}

impl FixedSelect for ResponseTab {}
