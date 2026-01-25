use ratatui::{
    style::{Color, Modifier, Style},
    widgets::BorderType,
};
use slumber_config::Theme;

/// Concrete styles for the TUI, generated from the theme. We *could* make this
/// entire thing user-configurable, but that would be way too complex. The theme
/// provides users some basic settings, then we figure out the minutae from
/// there. Styles are grouped into sub-structs generally by component.
#[derive(Clone, Debug)]
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
    pub syntax_highlighting: SyntaxHighlightingStyle,
}

/// Styles for the recipe input form
#[derive(Clone, Debug)]
pub struct FormStyles {
    /// Style for a input field title when not selected/focused
    pub title: Style,
    /// Style for a input field title when selected/focused
    pub title_highlight: Style,
    /// Style for an input field value when not selected/focused
    pub content: Style,
}

/// Styles for List component
#[derive(Clone, Debug)]
pub struct ListStyles {
    /// Highlighted item in a list
    pub highlight: Style,
    /// Highlight item in an inactive list (list isn't in focus)
    pub highlight_inactive: Style,
    /// Disabled item in a list
    pub disabled: Style,
    /// Regular item in a list
    pub item: Style,
}

/// Styles for the action menu
#[derive(Clone, Debug)]
pub struct MenuStyles {
    pub border: Style,
    pub border_type: BorderType,
    pub normal: Style,
}

/// Styles for the Modal component
#[derive(Clone, Debug)]
pub struct ModalStyles {
    pub border: Style,
    pub border_type: BorderType,
    pub normal: Style,
}

/// Styles for Pane component
#[derive(Clone, Debug)]
pub struct PaneStyles {
    /// Pane border when not selected/focused
    pub border: Style,
    /// Pane border when selected/focused
    pub border_selected: Style,
    /// Pane border characters used when not selected/focused
    pub border_type: BorderType,
    /// Pane border characters used when selected/focused
    pub border_type_selected: BorderType,
    /// Pane generic style
    pub generic: Style,
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
#[derive(Clone, Debug)]
pub struct StatusCodeStyles {
    pub success: Style,
    pub error: Style,
}

/// Styles for Tab component
#[derive(Clone, Debug)]
pub struct TabStyles {
    /// Disabled tab text
    pub disabled: Style,
    /// Highlighted (selected) tab text
    pub highlight: Style,
}

/// Styles for Table component
#[derive(Clone, Debug)]
pub struct TableStyles {
    /// Table column header text
    pub header: Style,
    pub text: Style,
    pub alt: Style,
    pub disabled: Style,
    pub highlight: Style,
    pub title: Style,
    pub background_color: Color,
}

/// Styles for TemplatePreview component
#[derive(Clone, Debug)]
pub struct TemplatePreviewStyles {
    pub text: Style,
    pub error: Style,
}

/// General text styles
#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
pub struct TextBoxStyle {
    pub text: Style,
    pub cursor: Style,
    pub placeholder: Style,
    pub invalid: Style,
}

/// Styles for TextWindow component
#[derive(Clone, Debug)]
pub struct TextWindowStyle {
    /// Line numbers on large text areas
    pub gutter: Style,
}

/// Styles for syntax highlighting
#[derive(Clone, Debug)]
pub struct SyntaxHighlightingStyle {
    pub comment: Style,
    pub builtin: Style,
    pub escape: Style,
    pub number: Style,
    pub string: Style,
    pub special: Style,
}

impl Styles {
    pub fn new(theme: &Theme) -> Self {
        Self {
            form: FormStyles {
                title: Style::default()
                    .fg(theme.text_color)
                    .add_modifier(Modifier::UNDERLINED),
                title_highlight: Style::default()
                    .fg(theme.primary_color)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                content: Style::default().fg(theme.inactive_color),
            },
            list: ListStyles {
                highlight: Style::default()
                    .bg(theme.primary_color)
                    .fg(theme.primary_text_color)
                    .add_modifier(Modifier::BOLD),
                highlight_inactive: Style::default()
                    .bg(theme.inactive_color)
                    .fg(theme.primary_text_color)
                    .add_modifier(Modifier::BOLD),
                disabled: Style::default()
                    .bg(theme.background_color)
                    .fg(theme.inactive_color),
                item: Style::default().fg(theme.text_color),
            },
            menu: MenuStyles {
                border: Style::default()
                    .fg(theme.primary_color)
                    .bg(theme.background_color),
                border_type: BorderType::Rounded,
                normal: Style::default()
                    .bg(theme.background_color)
                    .fg(theme.text_color),
            },
            modal: ModalStyles {
                border: Style::default()
                    .fg(theme.primary_color)
                    .bg(theme.background_color),
                border_type: BorderType::Double,
                normal: Style::default()
                    .bg(theme.background_color)
                    .fg(theme.text_color),
            },
            pane: PaneStyles {
                border: Style::default().fg(theme.border_color),
                border_selected: Style::default()
                    .fg(theme.primary_color)
                    .add_modifier(Modifier::BOLD),
                border_type: BorderType::Rounded,
                border_type_selected: BorderType::Double,
                generic: Style::default()
                    .bg(theme.background_color)
                    .fg(theme.text_color),
            },
            status_code: StatusCodeStyles {
                success: Style::default()
                    .bg(theme.success_color)
                    .fg(theme.primary_text_color),
                error: Style::default()
                    .bg(theme.error_color)
                    .fg(theme.primary_text_color),
            },
            tab: TabStyles {
                disabled: Style::default().fg(theme.inactive_color),
                highlight: Style::default()
                    .fg(theme.primary_color)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            },
            table: TableStyles {
                header: Style::default()
                    .fg(theme.text_color)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
                text: Style::default().fg(theme.text_color),
                background_color: theme.background_color,
                alt: Style::default()
                    .bg(theme.inactive_color)
                    .fg(theme.primary_text_color),
                disabled: Style::default().fg(theme.inactive_color),
                highlight: Style::default()
                    .bg(theme.primary_color)
                    .fg(theme.primary_text_color)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
                title: Style::default()
                    .fg(theme.text_color)
                    .add_modifier(Modifier::BOLD),
            },
            template_preview: TemplatePreviewStyles {
                text: Style::default()
                    .fg(theme.secondary_color)
                    .add_modifier(Modifier::UNDERLINED),
                error: Style::default()
                    .bg(theme.error_color)
                    .fg(theme.primary_text_color),
            },
            text: TextStyle {
                highlight: Style::default()
                    .fg(theme.primary_text_color)
                    .bg(theme.primary_color),
                hint: Style::default().fg(theme.inactive_color),
                primary: Style::default().fg(theme.primary_color),
                edited: Style::default()
                    .fg(theme.text_color)
                    .add_modifier(Modifier::ITALIC),
                error: Style::default().fg(theme.error_color),
                title: Style::default()
                    .fg(theme.text_color)
                    .add_modifier(Modifier::BOLD),
            },
            text_box: TextBoxStyle {
                text: Style::default()
                    .fg(theme.primary_text_color)
                    .bg(theme.inactive_color),
                cursor: Style::default()
                    .bg(theme.primary_text_color)
                    .fg(theme.inactive_color),
                placeholder: Style::default().fg(theme.text_color),
                invalid: Style::default()
                    .bg(theme.error_color)
                    .fg(theme.primary_text_color),
            },
            text_window: TextWindowStyle {
                gutter: Style::default().fg(theme.inactive_color),
            },
            syntax_highlighting: SyntaxHighlightingStyle {
                // We only style by foreground for syntax
                comment: Style::default()
                    .fg(theme.syntax_highlighting.comment_color),
                builtin: Style::default()
                    .fg(theme.syntax_highlighting.builtin_color),
                escape: Style::default()
                    .fg(theme.syntax_highlighting.escape_color),
                number: Style::default()
                    .fg(theme.syntax_highlighting.number_color),
                string: Style::default()
                    .fg(theme.syntax_highlighting.string_color),
                special: Style::default()
                    .fg(theme.syntax_highlighting.special_color),
            },
        }
    }
}
