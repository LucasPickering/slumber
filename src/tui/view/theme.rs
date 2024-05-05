use ratatui::{
    style::{Color, Modifier, Style},
    widgets::BorderType,
};
use serde::{Deserialize, Serialize};

/// User-configurable visual settings. These are used to generate the full style
/// set.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub primary_color: Color,
    /// Theoretically we could calculate this bsed on primary color, but for
    /// named or indexed colors, we don't know the exact RGB code since it
    /// depends on the user's terminal theme. It's much easier and less
    /// fallible to just have the user specify it.
    pub primary_text_color: Color,
    pub secondary_color: Color,
    pub success_color: Color,
    pub error_color: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            primary_color: Color::Blue,
            primary_text_color: Color::White,
            secondary_color: Color::Yellow,
            success_color: Color::Green,
            error_color: Color::Red,
        }
    }
}

/// Concrete styles for the TUI, generated from the theme. We *could* make this
/// entire thing user-configurable, but that would be way too complex. The theme
/// provides users some basic settings, then we figure out the minutae from
/// there. Styles are grouped into sub-structs generally by component.
#[derive(Debug)]
pub struct Styles {
    pub list: ListStyles,
    pub modal: ModalStyles,
    pub pane: PaneStyles,
    pub status_code: StatusCodeStyles,
    pub tab: TabStyles,
    pub table: TableStyles,
    pub template_preview: TemplatePreviewStyles,
    pub text: TextStyle,
    pub text_box: TextBoxStyle,
    pub text_window: TextWindowStyle,
}

impl Styles {
    pub fn new(theme: &Theme) -> Self {
        Self {
            list: ListStyles {
                highlight: Style::default()
                    .bg(theme.primary_color)
                    .fg(theme.primary_text_color)
                    .add_modifier(Modifier::BOLD),
            },
            modal: ModalStyles {
                border: Style::default().fg(theme.primary_color),
                border_type: BorderType::Double,
            },
            pane: PaneStyles {
                border: Style::default(),
                border_selected: Style::default()
                    .fg(theme.primary_color)
                    .add_modifier(Modifier::BOLD),
                border_type: BorderType::Plain,
                border_type_selected: BorderType::Double,
            },
            status_code: StatusCodeStyles {
                success: Style::default()
                    .fg(Color::Black)
                    .bg(theme.success_color),
                error: Style::default().bg(theme.error_color),
            },
            tab: TabStyles {
                highlight: Style::default()
                    .fg(theme.primary_color)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            },
            table: TableStyles {
                header: Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
                text: Style::default(),
                alt: Style::default().bg(Color::DarkGray),
                disabled: Style::default().add_modifier(Modifier::DIM),
                highlight: Style::default()
                    .bg(theme.primary_color)
                    .fg(theme.primary_text_color)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
                title: Style::default().add_modifier(Modifier::BOLD),
            },
            template_preview: TemplatePreviewStyles {
                text: Style::default().fg(theme.secondary_color),
                error: Style::default().bg(theme.error_color),
            },
            text: TextStyle {
                highlight: Style::default()
                    .fg(theme.primary_text_color)
                    .bg(theme.primary_color),
            },
            text_box: TextBoxStyle {
                text: Style::default().bg(Color::DarkGray),
                cursor: Style::default().bg(Color::White).fg(Color::Black),
                placeholder: Style::default().fg(Color::Black),
                invalid: Style::default().bg(Color::LightRed),
            },
            text_window: TextWindowStyle {
                line_number: Style::default().fg(Color::DarkGray),
            },
        }
    }
}

/// Styles for List component
#[derive(Debug)]
pub struct ListStyles {
    /// Highlighted item in a list
    pub highlight: Style,
}

/// Styles for the Modal component
#[derive(Debug)]
pub struct ModalStyles {
    pub border: Style,
    pub border_type: BorderType,
}

/// Styles for Pane component
#[derive(Debug)]
pub struct PaneStyles {
    /// Pane border when not selected/focused
    pub border: Style,
    /// Pane border when selected/focused
    pub border_selected: Style,
    /// Pane border characters used when not selected/focused
    pub border_type: BorderType,
    /// Pane border characters used when selected/focused
    pub border_type_selected: BorderType,
}

impl PaneStyles {
    /// Get the type and style of the border for a pane
    pub fn border(&self, is_focused: bool) -> (BorderType, Style) {
        if is_focused {
            (self.border_type_selected, self.border_selected)
        } else {
            (self.border_type, self.border)
        }
    }
}

/// Styles for HTTP status code display
#[derive(Debug)]
pub struct StatusCodeStyles {
    pub success: Style,
    pub error: Style,
}

/// Styles for Tab component
#[derive(Debug)]
pub struct TabStyles {
    /// Highlighted tab in a tab group
    pub highlight: Style,
}

/// Styles for Table component
#[derive(Debug)]
pub struct TableStyles {
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
pub struct TemplatePreviewStyles {
    pub text: Style,
    pub error: Style,
}

/// General text styles
#[derive(Debug)]
pub struct TextStyle {
    /// Text that needs some visual emphasis/separation
    pub highlight: Style,
}

/// Styles for TextBox component
#[derive(Debug)]
pub struct TextBoxStyle {
    pub text: Style,
    pub cursor: Style,
    pub placeholder: Style,
    pub invalid: Style,
}

/// Styles for TextWindow component
#[derive(Debug)]
pub struct TextWindowStyle {
    /// Line numbers on large text areas
    pub line_number: Style,
}
