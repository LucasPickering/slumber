use ratatui::style::{Color, Modifier, Style};

/// Configurable settings for the UI
#[derive(Debug)]
pub struct Theme {
    pub pane_border_style: Style,
    pub pane_border_focus_style: Style,
    pub list_highlight_style: Style,
    pub list_highlight_symbol: String,
}

impl Theme {
    pub fn pane_border_style(&self, is_focused: bool) -> Style {
        if is_focused {
            self.pane_border_focus_style
        } else {
            self.pane_border_style
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            pane_border_style: Style::default(),
            pane_border_focus_style: Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
            list_highlight_style: Style::default()
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
            list_highlight_symbol: ">> ".into(),
        }
    }
}
