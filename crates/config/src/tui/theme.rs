use ratatui_core::style::Color as RatColor;
use serde::Serialize;

/// User-configurable visual settings. These are used to generate the full style
/// set.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    /// Color for primary content such as the selected pane
    pub primary_color: Color,
    // Theoretically we could calculate this based on primary color, but for
    // named or indexed colors, we don't know the exact RGB code since it
    // depends on the user's terminal theme. It's much easier and less
    // fallible to just have the user specify it.
    /// Color for text on top of the primary color. This should contrast with
    /// the primary color well
    pub primary_text_color: Color,
    /// Color for secondary accented content
    pub secondary_color: Color,
    /// Color representing success (e.g. for 2xx status codes)
    pub success_color: Color,
    /// Color representing error (e.g. for 4xx status codes)
    pub error_color: Color,
    /// Color for regular text
    pub text_color: Color,
    /// Color for the background of the application
    pub background_color: Color,
    /// Color for inactive text and components
    pub inactive_color: Color,
    /// Color for hint text
    pub hint_text_color: Color,
    /// Color for the background of text boxes
    pub textbox_background_color: Color,
    /// Color of the background underneath the cursor
    pub cursor_background_color: Color,
    /// Color of the text underneath the cursor
    pub cursor_text_color: Color,
    /// Color of the background of the gutter text
    pub gutter_background_color: Color,
    /// Color of the gutter text
    pub gutter_text_color: Color,
    /// Color of the background of alternating table rows
    pub alternate_row_background_color: Color,
    /// Color of the text of alternating table rows
    pub alternate_row_text_color: Color,
    /// User-configurable visual settings for syntax highlighting
    pub syntax: Syntax,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            primary_color: RatColor::Blue.into(),
            secondary_color: RatColor::Yellow.into(),
            success_color: RatColor::Green.into(),
            error_color: RatColor::Red.into(),
            text_color: RatColor::Reset.into(),
            background_color: RatColor::Reset.into(),
            primary_text_color: RatColor::White.into(),
            syntax: Default::default(),
            inactive_color: RatColor::DarkGray.into(),
            hint_text_color: RatColor::DarkGray.into(),
            textbox_background_color: RatColor::DarkGray.into(),
            cursor_background_color: RatColor::Blue.into(),
            cursor_text_color: RatColor::DarkGray.into(),
            gutter_background_color: RatColor::Reset.into(),
            gutter_text_color: RatColor::DarkGray.into(),
            alternate_row_background_color: RatColor::White.into(),
            alternate_row_text_color: RatColor::DarkGray.into(),
        }
    }
}

/// User-configurable visual settings for syntax highlighting.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Syntax {
    /// Color for comments
    pub comment_color: Color,
    /// Color for builtins
    pub builtin_color: Color,
    /// Color for escape characters
    pub escape_color: Color,
    /// Color for numbers
    pub number_color: Color,
    /// Color for strings
    pub string_color: Color,
    /// Color for special characters
    pub special_color: Color,
}

impl Default for Syntax {
    fn default() -> Self {
        Self {
            comment_color: RatColor::Gray.into(),
            builtin_color: RatColor::Blue.into(),
            escape_color: RatColor::Green.into(),
            number_color: RatColor::Cyan.into(),
            string_color: RatColor::LightGreen.into(),
            special_color: RatColor::Green.into(),
        }
    }
}

/// ANSI color code
///
/// This type accepts input beyond the enumerated values, but for simplicity
/// this type only declares the named colors. The other available options
/// are very rarely used and make the schema harder to read.
///
/// For a full list of allowed types, see
/// [the ratatui docs](https://docs.rs/ratatui/0.29.0/ratatui/style/enum.Color.html#impl-FromStr-for-Color).
#[derive(Copy, Clone, Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    feature = "schema",
    schemars(rename = "Color", schema_with = "color_schema")
)]
// This wrapper lets us define deserialization and schema generation easily
pub struct Color(RatColor);

impl From<RatColor> for Color {
    fn from(color: RatColor) -> Self {
        Self(color)
    }
}

impl From<Color> for RatColor {
    fn from(color: Color) -> Self {
        color.0
    }
}

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
