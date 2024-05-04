use ratatui::{
    style::{Color, Modifier, Style},
    widgets::BorderType,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Serialize, Deserialize)]
pub struct Theme {
    pub primary_color: String,
    pub error_color: String,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            primary_color: Color::LightGreen.to_string(),
            error_color: Color::Red.to_string(),
        }
    }
}

/// Configurable visual settings for the UI. Styles are grouped into sub-structs
/// generally by component.
#[derive(Debug)]
pub struct Styles {
    pub list: ThemeList,
    pub modal: ThemeModal,
    pub pane: ThemePane,
    pub tab: ThemeTab,
    pub table: ThemeTable,
    pub template_preview: ThemeTemplatePreview,
    pub text: ThemeText,
    pub text_box: ThemeTextBox,
    pub text_window: ThemeTextWindow,
}

impl Styles {
    pub fn from_theme(theme: &Theme) -> Self {
        let primary_color =
            Color::from_str(&theme.primary_color).unwrap_or(Color::LightGreen);
        let error_color =
            Color::from_str(&theme.error_color).unwrap_or(Color::Red);

        Self {
            list: ThemeList {
                highlight: Style::default()
                    .bg(primary_color)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            },
            modal: ThemeModal {
                border: Style::default().fg(primary_color),
                border_type: BorderType::Double,
            },
            pane: ThemePane {
                border: Style::default(),
                border_selected: Style::default()
                    .fg(primary_color)
                    .add_modifier(Modifier::BOLD),
                border_type: BorderType::Plain,
                border_type_selected: BorderType::Double,
            },
            tab: ThemeTab {
                highlight: Style::default()
                    .fg(primary_color)
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
                    .bg(primary_color)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
                title: Style::default().add_modifier(Modifier::BOLD),
            },
            template_preview: ThemeTemplatePreview {
                text: Style::default().fg(Color::Blue),
                error: Style::default().bg(error_color),
            },
            text: ThemeText {
                highlight: Style::default().fg(Color::Black).bg(primary_color),
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

/// Styles for List component
#[derive(Debug)]
pub struct ThemeList {
    /// Highlighted item in a list
    pub highlight: Style,
}

/// Styles for the Modal component
#[derive(Debug)]
pub struct ThemeModal {
    pub border: Style,
    pub border_type: BorderType,
}

/// Styles for Pane component
#[derive(Debug)]
pub struct ThemePane {
    /// Pane border when not selected/focused
    pub border: Style,
    /// Pane border when selected/focused
    pub border_selected: Style,
    /// Pane border characters used when not selected/focused
    pub border_type: BorderType,
    /// Pane border characters used when selected/focused
    pub border_type_selected: BorderType,
}

/// Styles for Tab component
#[derive(Debug)]
pub struct ThemeTab {
    /// Highlighted tab in a tab group
    pub highlight: Style,
}

/// Styles for Table component
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

/// Styles for TemplatePreview component
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

/// Styles for TextBox component
#[derive(Debug)]
pub struct ThemeTextBox {
    pub text: Style,
    pub cursor: Style,
    pub placeholder: Style,
    pub invalid: Style,
}

/// Styles for TextWindow component
#[derive(Debug)]
pub struct ThemeTextWindow {
    /// Line numbers on large text areas
    pub line_number: Style,
}

impl ThemePane {
    /// Get the type and style of the border for a pane
    pub fn border(&self, is_focused: bool) -> (BorderType, Style) {
        if is_focused {
            (self.border_type_selected, self.border_selected)
        } else {
            (self.border_type, self.border)
        }
    }
}
