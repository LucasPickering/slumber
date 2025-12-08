use ratatui::{
    style::{Color, Modifier, Style},
    widgets::BorderType,
};
use slumber_config::Theme;

/// Concrete styles for the TUI, generated from the theme. We *could* make this
/// entire thing user-configurable, but that would be way too complex. The theme
/// provides users some basic settings, then we figure out the minutae from
/// there. Styles are grouped into sub-structs generally by component.
#[derive(Debug)]
pub struct Styles {
    pub form: FormStyles,
    pub list: ListStyles,
    pub menu: MenuStyles,
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

/// Styles for the recipe input form
#[derive(Debug)]
pub struct FormStyles {
    /// Style for a input field title when not selected/focused
    pub title: Style,
    /// Style for a input field title when selected/focused
    pub title_highlight: Style,
}

/// Styles for List component
#[derive(Debug)]
pub struct ListStyles {
    /// Highlighted item in a list
    pub highlight: Style,
    /// Highlight item in an inactive list (list isn't in focus)
    pub highlight_inactive: Style,
    /// Disabled item in a list
    pub disabled: Style,
}

/// Styles for the action menu
#[derive(Debug)]
pub struct MenuStyles {
    pub border_type: BorderType,
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
    pub fn border(&self, has_focus: bool) -> (BorderType, Style) {
        if has_focus {
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
    /// Disabled tab text
    pub disabled: Style,
    /// Highlighted (selected) tab text
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
    /// Informational text that should be de-emphasized
    pub hint: Style,
    /// Text in the primary color
    pub primary: Style,
    /// Templates that have been overridden in this session
    pub edited: Style,
    /// Text that means BAD BUSINESS
    pub error: Style,
    /// Text at the top of something
    pub title: Style,
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
    pub gutter: Style,
}

impl Styles {
    pub fn new(theme: &Theme) -> Self {
        Self {
            form: FormStyles {
                title: Style::default().add_modifier(Modifier::UNDERLINED),
                title_highlight: Style::default()
                    .fg(theme.primary_color)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            },
            list: ListStyles {
                highlight: Style::default()
                    .bg(theme.primary_color)
                    .fg(theme.primary_text_color)
                    .add_modifier(Modifier::BOLD),
                highlight_inactive: Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
                disabled: Style::default().add_modifier(Modifier::DIM),
            },
            menu: MenuStyles {
                border_type: BorderType::Rounded,
            },
            modal: ModalStyles {
                border: Style::default(),
                border_type: BorderType::Double,
            },
            pane: PaneStyles {
                border: Style::default(),
                border_selected: Style::default()
                    .fg(theme.primary_color)
                    .add_modifier(Modifier::BOLD),
                border_type: BorderType::Rounded,
                border_type_selected: BorderType::Double,
            },
            status_code: StatusCodeStyles {
                success: Style::default()
                    .fg(Color::Black)
                    .bg(theme.success_color),
                error: Style::default().bg(theme.error_color),
            },
            tab: TabStyles {
                disabled: Style::default().add_modifier(Modifier::DIM),
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
                text: Style::default()
                    .fg(theme.secondary_color)
                    .add_modifier(Modifier::UNDERLINED),
                error: Style::default()
                    .fg(Color::default()) // Override syntax highlighting
                    .bg(theme.error_color),
            },
            text: TextStyle {
                highlight: Style::default()
                    .fg(theme.primary_text_color)
                    .bg(theme.primary_color),
                hint: Style::default().fg(Color::DarkGray),
                primary: Style::default().fg(theme.primary_color),
                edited: Style::default().add_modifier(Modifier::ITALIC),
                error: Style::default().bg(theme.error_color),
                title: Style::default().add_modifier(Modifier::BOLD),
            },
            text_box: TextBoxStyle {
                text: Style::default().bg(Color::DarkGray),
                cursor: Style::default().bg(Color::White).fg(Color::Black),
                placeholder: Style::default().fg(Color::Black),
                invalid: Style::default().bg(Color::LightRed),
            },
            text_window: TextWindowStyle {
                gutter: Style::default().fg(Color::DarkGray),
            },
        }
    }
}
