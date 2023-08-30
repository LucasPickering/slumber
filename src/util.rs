use crate::config::{Environment, RequestRecipe};
use log::error;
use ratatui::text::Line;
use std::fmt::Display;

/// Exit the terminal before panics
pub fn initialize_panic_handler() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        crossterm::execute!(
            std::io::stderr(),
            crossterm::terminal::LeaveAlternateScreen
        )
        .unwrap();
        crossterm::terminal::disable_raw_mode().unwrap();
        original_hook(panic_info);
    }));
}

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
