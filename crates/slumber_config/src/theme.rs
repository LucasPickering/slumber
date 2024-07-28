use ratatui::style::Color;
use serde::{Deserialize, Serialize};

/// User-configurable visual settings. These are used to generate the full style
/// set.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
