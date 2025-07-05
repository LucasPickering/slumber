use ratatui_core::style::Color;
use serde::{Deserialize, Serialize};

/// User-configurable visual settings. These are used to generate the full style
/// set.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub primary_color: Color,
    /// Theoretically we could calculate this bsed on primary color, but for
    /// named or indexed colors, we don't know the exact RGB code since it
    /// depends on the user's terminal theme. It's much easier and less
    /// fallible to just have the user specify it.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub primary_text_color: Color,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub secondary_color: Color,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub success_color: Color,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
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
