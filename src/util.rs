use crate::config::{Environment, RequestRecipe};
use crossterm::{event::DisableMouseCapture, terminal::LeaveAlternateScreen};
use ratatui::text::Line;
use std::io;

/// Restore termian state during a panic
pub fn initialize_panic_handler() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        restore_terminal().unwrap();
        original_hook(panic_info);
    }));
}

/// TODO
pub fn restore_terminal() -> io::Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        std::io::stderr(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(())
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
