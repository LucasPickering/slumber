use ratatui::style::{Color, Modifier, Style};

/// Configurable visual settings for the UI. Styles are grouped into sub-structs
/// generally by component.
#[derive(Debug)]
pub struct Theme {
    pub pane: ThemePane,
    pub list: ThemeList,
    pub tab: ThemeTab,
    pub table: ThemeTable,
    pub template_preview: ThemeTemplatePreview,
    pub text: ThemeText,
    pub text_box: ThemeTextBox,
    pub text_window: ThemeTextWindow,
}

impl Theme {
    // Ideally these should be part of the theme, but that requires some sort of
    // two-stage themeing
    pub const PRIMARY_COLOR: Color = Color::LightGreen;
    pub const ERROR_COLOR: Color = Color::Red;
}

#[derive(Debug)]
pub struct ThemeList {
    /// Highlighted item in a list
    pub highlight: Style,
}

#[derive(Debug)]
pub struct ThemePane {
    /// Pane border when not selected/focused
    pub border: Style,
    /// Pane border when selected/focused
    pub border_selected: Style,
}

#[derive(Debug)]
pub struct ThemeTab {
    /// Highlighted tab in a tab group
    pub highlight: Style,
}

#[derive(Debug)]
pub struct ThemeTable {
    /// Table column header text
    pub header: Style,
    pub text: Style,
    pub alt: Style,
    pub disabled: Style,
    pub highlight: Style,
    pub title: Style,
}

#[derive(Debug)]
pub struct ThemeTemplatePreview {
    pub text: Style,
    pub error: Style,
}

/// General text styles
#[derive(Debug)]
pub struct ThemeText {
    /// Text that needs some visual emphasis/separation
    pub highlight: Style,
}

#[derive(Debug)]
pub struct ThemeTextBox {
    pub text: Style,
    pub cursor: Style,
    pub placeholder: Style,
    pub invalid: Style,
}

#[derive(Debug)]
pub struct ThemeTextWindow {
    /// Line numbers on large text areas
    pub line_number: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            pane: ThemePane {
                border: Style::default(),
                border_selected: Style::default()
                    .fg(Self::PRIMARY_COLOR)
                    .add_modifier(Modifier::BOLD),
            },
            list: ThemeList {
                highlight: Style::default()
                    .bg(Self::PRIMARY_COLOR)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            },
            tab: ThemeTab {
                highlight: Style::default()
                    .fg(Self::PRIMARY_COLOR)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            },
            table: ThemeTable {
                header: Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
                text: Style::default(),
                alt: Style::default().bg(Color::DarkGray),
                disabled: Style::default().add_modifier(Modifier::DIM),
                highlight: Style::default()
                    .bg(Self::PRIMARY_COLOR)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
                title: Style::default().add_modifier(Modifier::BOLD),
            },
            template_preview: ThemeTemplatePreview {
                text: Style::default().fg(Color::Blue),
                error: Style::default().bg(Self::ERROR_COLOR),
            },
            text: ThemeText {
                highlight: Style::default()
                    .fg(Color::Black)
                    .bg(Self::PRIMARY_COLOR),
            },
            text_box: ThemeTextBox {
                text: Style::default().bg(Color::DarkGray),
                cursor: Style::default().bg(Color::White).fg(Color::Black),
                placeholder: Style::default().fg(Color::Black),
                invalid: Style::default().bg(Color::LightRed),
            },
            text_window: ThemeTextWindow {
                line_number: Style::default().fg(Color::DarkGray),
            },
        }
    }
}

impl ThemePane {
    pub fn border_style(&self, is_focused: bool) -> Style {
        if is_focused {
            self.border_selected
        } else {
            self.border
        }
    }
}
