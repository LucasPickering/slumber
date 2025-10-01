use ratatui_core::style::Color;
use serde::{Deserialize, Serialize};

/// User-configurable visual settings. These are used to generate the full style
/// set.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    /// Color for primary content such as the selected pane
    #[cfg_attr(feature = "schema", schemars(with = "schema::Color"))]
    pub primary_color: Color,
    // Theoretically we could calculate this bsed on primary color, but for
    // named or indexed colors, we don't know the exact RGB code since it
    // depends on the user's terminal theme. It's much easier and less
    // fallible to just have the user specify it.
    /// Color for text on top of the primary color. This should contrast with
    /// the primary color well
    #[cfg_attr(feature = "schema", schemars(with = "schema::Color"))]
    pub primary_text_color: Color,
    /// Color for secondary accented content
    #[cfg_attr(feature = "schema", schemars(with = "schema::Color"))]
    pub secondary_color: Color,
    /// Color representing success (e.g. for 2xx status codes)
    #[cfg_attr(feature = "schema", schemars(with = "schema::Color"))]
    pub success_color: Color,
    /// Color representing error (e.g. for 4xx status codes)
    #[cfg_attr(feature = "schema", schemars(with = "schema::Color"))]
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

/// Helpers for JSON Schema generation
#[cfg(feature = "schema")]
mod schema {
    /// ANSI color code
    ///
    /// This type accepts input beyond the enumerated values, but for simplicity
    /// this type only declares the named colors. The other available options
    /// are very rarely used and make the schema harder to read.
    ///
    /// For a full list of allowed types, see
    /// [the ratatui docs](https://docs.rs/ratatui/0.29.0/ratatui/style/enum.Color.html#impl-FromStr-for-Color).
    #[cfg(feature = "schema")]
    #[derive(schemars::JsonSchema)]
    #[schemars(rename = "Color", schema_with = "color_schema")]
    // This type is just a vessel for a JSON Schema. We replace ratatui's Color
    // with this in the schema
    pub struct Color;

    #[cfg(feature = "schema")]
    fn color_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "enum": [
                "black",
                "red",
                "green",
                "yellow",
                "blue",
                "magenta",
                "cyan",
                "gray",
                "darkgray",
                "lightred",
                "lightgreen",
                "lightyellow",
                "lightblue",
                "lightmagenta",
                "lightcyan",
                "white",
                "reset",
            ]
        })
    }
}
