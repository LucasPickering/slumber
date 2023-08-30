use crate::{
    config::{Environment, RequestCollection, RequestRecipe},
    util::ToLines,
};
use ratatui::{prelude::*, widgets::*};
use reqwest::Request;
use std::collections::VecDeque;

/// Main app state. All configuration and UI state is stored here. The M in MVC
#[derive(Debug)]
pub struct AppState {
    pub message_queue: VecDeque<Message>,
    pub environments: StatefulList<Environment>,
    pub recipes: StatefulList<RequestRecipe>,
    /// Current in-flight HTTP request
    pub active_request: Option<Request>,
}

impl AppState {
    pub fn new(collection: RequestCollection) -> Self {
        Self {
            message_queue: VecDeque::new(),
            environments: StatefulList::with_items(collection.environments),
            recipes: StatefulList::with_items(collection.requests),
            active_request: None,
        }
    }

    /// Stick a message on the end of the queue
    pub fn enqueue(&mut self, message: Message) {
        self.message_queue.push_back(message);
    }
}

impl From<RequestCollection> for AppState {
    fn from(collection: RequestCollection) -> Self {
        Self::new(collection)
    }
}

/// A message triggers some action in the state. The controller is responsible
/// for both triggering and handling messages
#[derive(Copy, Clone, Debug)]
pub enum Message {
    /// Launch an HTTP request from the currently selected recipe. Errors if
    /// the recipes aren't in focus, or the list is empty
    SendRequest,
    /// Select the previous item in the focused list
    SelectPrevious,
    /// Select the next item in the focused list
    SelectNext,
}

/// A list of items in the UI
#[derive(Debug)]
pub struct StatefulList<T> {
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

    pub fn unselect(&mut self) {
        self.state.select(None);
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
