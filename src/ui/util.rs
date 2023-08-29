use log::error;
use ratatui::{
    style::{Color, Style},
    text::Line,
    widgets::{ListItem, ListState},
};
use std::fmt::Display;

pub trait ToLines {
    fn to_lines(&self) -> Vec<Line<'static>>;
}

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

/// If a result is an error, log it. Useful for handling errors in situations
/// where we can't panic or exit.
pub fn log_error<T, E: Display>(result: Result<T, E>) {
    if let Err(err) = result {
        error!("{err}");
    }
}
