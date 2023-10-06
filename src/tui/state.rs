use crate::{
    config::{Chain, Environment, RequestCollection, RequestRecipe},
    repository::{Repository, RequestRecord},
    template::TemplateContext,
    tui::{
        input::InputTarget,
        view::{
            EnvironmentListPane, ErrorPopup, RecipeListPane, RequestPane,
            ResponsePane,
        },
    },
    util::ResultExt,
};
use chrono::{DateTime, Duration, Utc};
use derive_more::{Display, From};
use ratatui::widgets::*;
use std::{
    fmt::Display,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};
use strum::{EnumIter, IntoEnumIterator};
use tokio::{runtime::Handle, sync::mpsc::UnboundedSender};
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
    pub repository: Repository,
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
    /// The pane that the user has focused, which will receive input events
    /// UNLESS a high-priority popup is open
    pub selected_pane: StatefulSelect<PrimaryPane>,
    pub request_tab: StatefulSelect<RequestTab>,
    pub response_tab: StatefulSelect<ResponseTab>,
    pub environments: StatefulList<Environment>,
    pub recipes: StatefulList<RequestRecipe>,
    pub chains: Vec<Chain>,
}

impl AppState {
    pub fn new(
        collection_file: PathBuf,
        collection: RequestCollection,
        repository: Repository,
        messages_tx: impl Into<MessageSender>,
    ) -> Self {
        Self {
            should_run: true,
            collection_file,
            messages_tx: messages_tx.into(),
            repository,
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

    /// Get whichever request state should currently be shown to the user,
    /// based on whichever recipe is selected.
    ///
    /// The request is loaded from the repository, which means it's async. This
    /// will block on the future, which we expect to be very fast to resolve.
    pub fn active_request(&mut self) -> Option<Arc<RequestRecord>> {
        let selected_recipe_id = self.ui.recipes.selected()?.id.clone();

        // Block until we get a request (should be fast)
        let repository = self.repository.clone();
        let rt_handle = Handle::current();
        let result = rt_handle.block_on(async move {
            repository.get_last(&selected_recipe_id).await
        });
        result.ok_or_apply(|err| self.set_error(err)).flatten()
    }

    /// Expose app state to the templater. Most of the data has to be cloned out
    /// to be passed across async boundaries. This is annoying but in reality
    /// it should be small data.
    pub fn template_context(&self) -> TemplateContext {
        TemplateContext {
            environment: self
                .ui
                .environments
                .selected()
                .map(|e| e.data.clone())
                .unwrap_or_default(),
            repository: self.repository.clone(),
            chains: self.ui.chains.clone(),
            overrides: Default::default(),
        }
    }

    /// Get the stored notification (if any). This requires a mutable reference
    /// because this will check if the notification (if any) is expired, and
    /// if so clear it.
    pub fn notification(&mut self) -> Option<&Notification> {
        // Expire the notification
        if let Some(notification) = &self.ui.notification {
            if notification.expired() {
                self.ui.notification = None;
            }
        }
        self.ui.notification.as_ref()
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
            selected_pane: StatefulSelect::new(),
            request_tab: StatefulSelect::new(),
            response_tab: StatefulSelect::new(),
            environments: StatefulList::with_items(collection.environments),
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
    StartReloadCollection,
    /// Store a reloaded collection value in state
    #[display(fmt = "EndReloadCollection(collection_file:?)")]
    EndReloadCollection {
        collection_file: PathBuf,
        collection: RequestCollection,
    },
    /// Launch an HTTP request from the currently selected recipe. Errors if
    /// the recipe list is empty.
    HttpSendRequest,
    /// An error occurred in some async process and should be shown to the user
    Error { error: anyhow::Error },
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

/// A list of items in the UI
#[derive(Debug)]
pub struct StatefulList<T> {
    pub state: ListState,
    pub items: Vec<T>,
}

impl<T> StatefulList<T> {
    pub fn with_items(items: Vec<T>) -> StatefulList<T> {
        let mut state = ListState::default();
        // Pre-select the first item if possible
        if !items.is_empty() {
            state.select(Some(0));
        }
        StatefulList { state, items }
    }

    /// Get the currently selected item (if any)
    pub fn selected(&self) -> Option<&T> {
        self.items.get(self.state.selected()?)
    }

    pub fn previous(&mut self) {
        let i = match self.state.selected() {
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
        self.state.select(Some(i));
    }

    /// Select the next item in the list
    pub fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => (i + 1) % self.items.len(),
            None => 0,
        };
        self.state.select(Some(i));
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
    #[display(fmt = "Environments")]
    EnvironmentList,
    #[display(fmt = "Recipes")]
    RecipeList,
    Request,
    Response,
}

impl PrimaryPane {
    /// Get a trait object that should handle contextual input for this pane
    pub fn input_handler(self) -> Box<dyn InputTarget> {
        match self {
            Self::EnvironmentList => Box::new(EnvironmentListPane),
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
