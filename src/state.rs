use crate::{
    config::{Environment, RequestCollection, RequestRecipe},
    http::Request,
    view::{Pane, RecipeListPane},
};
use ratatui::widgets::*;
use std::{collections::VecDeque, rc::Rc};

/// Main app state. All configuration and UI state is stored here. The M in MVC
#[derive(Debug)]
pub struct AppState {
    /// Flag to control the main app loop. Set to false to exit the app
    should_run: bool,
    message_queue: VecDeque<Message>,
    /// The pane that the user has focused, which will receive input events
    pub focused_pane: Rc<dyn Pane>,
    pub environments: StatefulList<Environment>,
    pub recipes: StatefulList<RequestRecipe>,
    /// Most recent HTTP request
    pub active_request: Option<Request>,
    pub response_tab: StatefulSelect<ResponseTab>,
}

impl AppState {
    pub fn new(collection: RequestCollection) -> Self {
        Self {
            should_run: true,
            message_queue: VecDeque::new(),
            focused_pane: Rc::new(RecipeListPane),
            environments: StatefulList::with_items(collection.environments),
            recipes: StatefulList::with_items(collection.requests),
            active_request: None,
            response_tab: StatefulSelect::new(),
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

    /// Stick a message on the end of the queue
    pub fn enqueue(&mut self, message: Message) {
        self.message_queue.push_back(message);
    }

    /// Pop a message off the queue (if it's not empty)
    pub fn dequeue(&mut self) -> Option<Message> {
        self.message_queue.pop_front()
    }

    /// Shift focus to the previous pane
    pub fn focus_previous(&mut self) {
        self.focused_pane = self.focused_pane.previous().into();
    }

    /// Shift focus to the next pane
    pub fn focus_next(&mut self) {
        self.focused_pane = self.focused_pane.next().into();
    }

    /// Check if the given pane is in focus
    pub fn is_focused(&self, pane: &dyn Pane) -> bool {
        self.focused_pane.equals(pane)
    }
}

impl From<RequestCollection> for AppState {
    fn from(collection: RequestCollection) -> Self {
        Self::new(collection)
    }
}

/// A message triggers some *asynchronous* action. Most state modifications can
/// be made synchronously by the input handler, but some require async handling
/// at the top level. The controller is responsible for both triggering and
/// handling messages.
#[derive(Copy, Clone, Debug)]
pub enum Message {
    /// Launch an HTTP request from the currently selected recipe. Errors if
    /// the recipes aren't in focus, or the list is empty
    SendRequest,
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
/// cycle between them using StatefulSelect.
/// TODO replace this with some stuff from strum?
pub trait FixedSelect: Sized {
    fn all() -> Vec<Self>;

    fn title(&self) -> &'static str;
}

#[derive(Debug)]
pub struct StatefulSelect<T: FixedSelect> {
    values: Vec<T>,
    selected: usize,
}

impl<T: FixedSelect> StatefulSelect<T> {
    pub fn new() -> Self {
        let values = FixedSelect::all();
        if values.is_empty() {
            panic!("Cannot create StatefulSelect from empty values");
        }
        Self {
            values,
            selected: 0,
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

#[derive(Debug)]
pub enum ResponseTab {
    Body,
    Headers,
}

impl FixedSelect for ResponseTab {
    fn all() -> Vec<Self> {
        vec![Self::Body, Self::Headers]
    }

    fn title(&self) -> &'static str {
        match self {
            ResponseTab::Body => "Body",
            ResponseTab::Headers => "Headers",
        }
    }
}
