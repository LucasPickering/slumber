use crate::{
    config::{Chain, Environment, RequestCollection, RequestRecipe},
    history::{RequestHistory, RequestRecord},
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
use derive_more::From;
use ratatui::widgets::*;
use std::{
    fmt::Display,
    ops::Deref,
    path::{Path, PathBuf},
};
use strum::{EnumIter, IntoEnumIterator};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info};

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
    pub history: RequestHistory,
    /// All the stuff that directly affects what's shown on screen
    pub ui: UiState,
}

/// UI-related state fields. This is separated so it can be recreated when the
/// request collection is reloaded.
#[derive(Debug)]
pub struct UiState {
    /// Any error that should be shown to the user in a popup
    error: Option<anyhow::Error>,
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
        history: RequestHistory,
        messages_tx: impl Into<MessageSender>,
    ) -> Self {
        Self {
            should_run: true,
            collection_file,
            messages_tx: messages_tx.into(),
            history,
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
        match self.ui.error() {
            Some(_) => Box::new(ErrorPopup),
            None => self.ui.selected_pane.selected().input_handler(),
        }
    }

    /// Get whichever request state should currently be shown to the user,
    /// based on whichever recipe is selected. Only returns `None` if there has
    /// never been request sent for the current recipe.
    pub fn active_request(&mut self) -> Option<RequestRecord> {
        self.history
            .get_last(&self.ui.recipes.selected()?.id)
            .ok_or_apply(|err| self.ui.set_error(err))
            .flatten()
    }

    /// Expose app state to the templater
    pub fn template_context(&self) -> TemplateContext {
        TemplateContext {
            environment: self.ui.environments.selected().map(|e| &e.data),
            overrides: None,
            history: &self.history,
            chains: &self.ui.chains,
        }
    }
}

impl UiState {
    fn new(collection: RequestCollection) -> Self {
        Self {
            error: None,
            selected_pane: StatefulSelect::new(),
            request_tab: StatefulSelect::new(),
            response_tab: StatefulSelect::new(),
            environments: StatefulList::with_items(collection.environments),
            recipes: StatefulList::with_items(collection.requests),
            chains: collection.chains,
        }
    }

    /// Get the stored error (if any)
    pub fn error(&self) -> Option<&anyhow::Error> {
        self.error.as_ref()
    }

    /// Store an error in state, to be shown to the user
    pub fn set_error(&mut self, err: anyhow::Error) {
        error!(error = err.deref());
        self.error = Some(err);
    }

    /// Close the error popup
    pub fn clear_error(&mut self) {
        info!("Clearing error state");
        self.error = None;
    }
}

/// Wrapper around a sender for async messages. Cheap to clone and pass around
#[derive(Clone, Debug, From)]
pub struct MessageSender(UnboundedSender<Message>);

impl MessageSender {
    /// Send an async message, to be handled by the main loop
    pub fn send(&self, message: Message) {
        self.0.send(message).expect("Message queue is closed")
    }
}

/// A message triggers some *asynchronous* action. Most state modifications can
/// be made synchronously by the input handler, but some require async handling
/// at the top level. The controller is responsible for both triggering and
/// handling messages.
#[derive(Debug)]
pub enum Message {
    /// Trigger collection reload
    StartReloadCollection,
    /// Store a reloaded collection value in state
    EndReloadCollection { collection: RequestCollection },
    /// Launch an HTTP request from the currently selected recipe. Errors if
    /// the recipes aren't in focus, or the list is empty
    SendRequest,
    /// An error occurred in some async process and should be shown to the user
    Error { error: anyhow::Error },
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
