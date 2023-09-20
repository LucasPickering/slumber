use crate::{
    config::{Environment, RequestCollection, RequestRecipe},
    history::{RequestHistory, RequestId, RequestRecord},
    http::Response,
    template::TemplateContext,
    tui::{
        input::InputTarget,
        view::{
            EnvironmentListPane, ErrorPopup, RecipeListPane, RequestPane,
            ResponsePane,
        },
    },
};
use ratatui::widgets::*;
use std::{fmt::Display, ops::Deref};
use strum::{EnumIter, IntoEnumIterator};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info};

/// Main app state. All configuration and UI state is stored here. The M in MVC
#[derive(Debug)]
pub struct AppState {
    // Global app state
    /// Flag to control the main app loop. Set to false to exit the app
    should_run: bool,
    /// Sender end of the message queue. Anything can use this to pass async
    /// messages back to the main thread to be handled. We use an unbounded
    /// sender because we don't ever expect the queue to get that large, and it
    /// allows for synchronous enqueueing.
    pub messages_tx: MessageSender,

    // UI state
    /// Any error that should be shown to the user in a popup
    error: Option<anyhow::Error>,
    /// The pane that the user has focused, which will receive input events
    /// UNLESS a high-priority popup is open
    pub selected_pane: StatefulSelect<PrimaryPane>,
    pub request_tab: StatefulSelect<RequestTab>,
    pub response_tab: StatefulSelect<ResponseTab>,
    pub environments: StatefulList<Environment>,
    pub recipes: StatefulList<RequestRecipe>,

    // HTTP state
    pub history: RequestHistory,
}

impl AppState {
    pub fn new(
        collection: RequestCollection,
        messages_tx: UnboundedSender<Message>,
    ) -> Self {
        Self {
            should_run: true,
            messages_tx: MessageSender(messages_tx),
            error: None,
            selected_pane: StatefulSelect::new(),
            request_tab: StatefulSelect::new(),
            response_tab: StatefulSelect::new(),
            environments: StatefulList::with_items(collection.environments),
            recipes: StatefulList::with_items(collection.requests),
            history: RequestHistory::load().unwrap(),
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

    /// Get the active local input handler, based on pane focus and popup state
    pub fn input_handler(&self) -> Box<dyn InputTarget> {
        match self.error() {
            Some(_) => Box::new(ErrorPopup),
            None => self.selected_pane.selected().input_handler(),
        }
    }

    /// Get whichever request state should currently be shown to the user,
    /// based on whichever recipe is selected. Only returns `None` if there has
    /// never been request sent for the current recipe.
    pub fn active_request(&mut self) -> Option<RequestRecord> {
        match self.history.get_last(&self.recipes.selected()?.id) {
            Ok(record) => record,
            Err(err) => {
                // Hide the error and act natural
                self.set_error(err);
                None
            }
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

/// Expose app state to the templater
impl<'a> From<&'a AppState> for TemplateContext<'a> {
    fn from(state: &'a AppState) -> Self {
        Self {
            environment: state.environments.selected().map(|e| &e.data),
            overrides: None,
        }
    }
}

/// Wrapper around a sender for async messages. Free to clone and pass around
#[derive(Clone, Debug)]
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
    /// Launch an HTTP request from the currently selected recipe. Errors if
    /// the recipes aren't in focus, or the list is empty
    SendRequest,
    /// An HTTP response was received (or the request failed), and we should
    /// update state accordingly
    Response {
        /// ID of the originating request
        request_id: RequestId,
        response: anyhow::Result<Response>,
    },
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

/// Friendly little trait indicating a type can be cycled through
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
