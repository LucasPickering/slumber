use crate::{
    config::{Environment, RequestCollection, RequestRecipe},
    http::Request,
    ui::Element,
    util::ToLines,
};
use ratatui::{prelude::*, widgets::*};
use std::collections::VecDeque;

/// Main app state. All configuration and UI state is stored here. The M in MVC
#[derive(Debug)]
pub struct AppState {
    /// Flag to control the main app loop. Set to false to exit the app
    should_run: bool,
    message_queue: VecDeque<Message>,
    /// The element that the user has focused, which will receive input events
    pub focused_element: Element,
    pub environments: StatefulList<Environment>,
    pub recipes: StatefulList<RequestRecipe>,
    /// Most recent HTTP request
    pub active_request: Option<Request>,
}

impl AppState {
    pub fn new(collection: RequestCollection) -> Self {
        Self {
            should_run: true,
            message_queue: VecDeque::new(),
            focused_element: Element::RecipeList,
            environments: StatefulList::with_items(collection.environments),
            recipes: StatefulList::with_items(collection.requests),
            active_request: None,
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

    /// Check if the given element is in focus
    pub fn is_focused(&self, element: &Element) -> bool {
        &self.focused_element == element
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
    /// TODO include request here
    SendRequest,
    // TODO add message for response
}

/// A list of items in the UI
#[derive(Debug)]
pub struct StatefulList<T: ToLines> {
    pub state: ListState,
    pub items: Vec<T>,
}

impl<T: ToLines> StatefulList<T> {
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
            Some(i) => {
                if i >= self.items.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    pub fn to_items(&self) -> Vec<ListItem<'static>> {
        self.items
            .iter()
            .map(|element| {
                ListItem::new(element.to_lines())
                    .style(Style::default().fg(Color::Black).bg(Color::White))
            })
            .collect()
    }
}
