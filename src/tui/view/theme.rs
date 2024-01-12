use ratatui::style::{Color, Modifier, Style};

// Ideally these should be part of the theme, but that requires some sort of
// two-stage themeing
pub const PRIMARY_COLOR: Color = Color::LightGreen;
pub const ERROR_COLOR: Color = Color::Red;

/// Configurable visual settings for the UI
#[derive(Debug)]
pub struct Theme {
    /// Line numbers on large text areas
    pub line_number_style: Style,

    /// Highlighted item in a list
    pub list_highlight_style: Style,

    /// Pane border when not selected/focused
    pub pane_border_style: Style,
    /// Pane border when selected/focused
    pub pane_border_selected_style: Style,

    /// Highlighted tab in a tab group
    pub tab_highlight_style: Style,

    /// Table column header text
    pub table_header_style: Style,
    pub table_text_style: Style,
    pub table_alt_style: Style,
    pub table_disabled_style: Style,
    pub table_highlight_style: Style,
    pub table_title_style: Style,

    pub template_preview_text: Style,
    pub template_preview_error: Style,

    /// Text that needs some visual emphasis/separation
    pub text_highlight: Style,
}

impl Theme {
    pub fn pane_border_style(&self, is_focused: bool) -> Style {
        if is_focused {
            self.pane_border_selected_style
        } else {
            self.pane_border_style
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            line_number_style: Style::default().fg(Color::DarkGray),

            list_highlight_style: Style::default()
                .bg(PRIMARY_COLOR)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),

            pane_border_style: Style::default(),
            pane_border_selected_style: Style::default()
                .fg(PRIMARY_COLOR)
                .add_modifier(Modifier::BOLD),

            tab_highlight_style: Style::default()
                .fg(PRIMARY_COLOR)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),

            table_header_style: Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
            table_text_style: Style::default(),
            table_alt_style: Style::default().bg(Color::DarkGray),
            table_disabled_style: Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::CROSSED_OUT),
            table_highlight_style: Style::default()
                .bg(PRIMARY_COLOR)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
            table_title_style: Style::default().add_modifier(Modifier::BOLD),

            template_preview_text: Style::default().fg(Color::Blue),
            template_preview_error: Style::default().bg(ERROR_COLOR),

            text_highlight: Style::default().fg(Color::Black).bg(PRIMARY_COLOR),
        }
    }
}
