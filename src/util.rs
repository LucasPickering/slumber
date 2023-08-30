use crate::config::{Environment, RequestRecipe};
use log::error;
use ratatui::text::Line;
use std::fmt::Display;

/// If a result is an error, log it. Useful for handling errors in situations
/// where we can't panic or exit.
pub fn log_error<T, E: Display>(result: Result<T, E>) {
    if let Err(err) = result {
        error!("{err}");
    }
}

pub trait ToLines {
    fn to_lines(&self) -> Vec<Line<'static>>;
}

// Getting lazy with the lifetimes here...
impl ToLines for Environment {
    fn to_lines(&self) -> Vec<Line<'static>> {
        vec![self.name.clone().into()]
    }
}

impl ToLines for RequestRecipe {
    fn to_lines(&self) -> Vec<Line<'static>> {
        vec![
            self.name.clone().into(),
            format!("{} {}", self.method, self.url).into(),
        ]
    }
}
