use ratatui::style::{Color, Modifier, Style};

/// Configurable settings for the UI
#[derive(Debug)]
pub struct Theme {
    pub list_highlight_style: Style,
    pub list_highlight_symbol: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            list_highlight_style: Style::default()
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
            list_highlight_symbol: ">> ".into(),
        }
    }
}
