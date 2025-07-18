use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// User-configurable visual settings. These are used to generate the full style
/// set.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
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

/// ANSI Color
///
/// # Source
///
/// This struct was copied from [Ratatui](https://github.com/ratatui/ratatui/blob/v0.28.1/src/style/color.rs).
/// Inlining it allows us to eliminate Ratatui as dependency for the config
/// (and subsequently the CLI) crate.
///
/// All colors from the [ANSI color table] are supported (though some names are
/// not exactly the same).
///
/// | Color Name     | Color                   | Foreground | Background |
/// |----------------|-------------------------|------------|------------|
/// | `black`        | [`Color::Black`]        | 30         | 40         |
/// | `red`          | [`Color::Red`]          | 31         | 41         |
/// | `green`        | [`Color::Green`]        | 32         | 42         |
/// | `yellow`       | [`Color::Yellow`]       | 33         | 43         |
/// | `blue`         | [`Color::Blue`]         | 34         | 44         |
/// | `magenta`      | [`Color::Magenta`]      | 35         | 45         |
/// | `cyan`         | [`Color::Cyan`]         | 36         | 46         |
/// | `gray`*        | [`Color::Gray`]         | 37         | 47         |
/// | `darkgray`*    | [`Color::DarkGray`]     | 90         | 100        |
/// | `lightred`     | [`Color::LightRed`]     | 91         | 101        |
/// | `lightgreen`   | [`Color::LightGreen`]   | 92         | 102        |
/// | `lightyellow`  | [`Color::LightYellow`]  | 93         | 103        |
/// | `lightblue`    | [`Color::LightBlue`]    | 94         | 104        |
/// | `lightmagenta` | [`Color::LightMagenta`] | 95         | 105        |
/// | `lightcyan`    | [`Color::LightCyan`]    | 96         | 106        |
/// | `white`*       | [`Color::White`]        | 97         | 107        |
///
/// - `gray` is sometimes called `white` - this is not supported as we use
///   `white` for bright white
/// - `gray` is sometimes called `silver` - this is supported
/// - `darkgray` is sometimes called `light black` or `bright black` (both are
///   supported)
/// - `white` is sometimes called `light white` or `bright white` (both are
///   supported)
/// - we support `bright` and `light` prefixes for all colors
/// - we support `-` and `_` and ` ` as separators for all colors
/// - we support both `gray` and `grey` spellings
///
/// `From<Color> for Style` is implemented by creating a style with the
/// foreground color set to the given color. This allows you to use colors
/// anywhere that accepts `Into<Style>`.
///
/// # Example
///
/// ```
/// use slumber_config::Color;
/// use std::str::FromStr;
///
/// assert_eq!(Color::from_str("red"), Ok(Color::Red));
/// assert_eq!("red".parse(), Ok(Color::Red));
/// assert_eq!("lightred".parse(), Ok(Color::LightRed));
/// assert_eq!("light red".parse(), Ok(Color::LightRed));
/// assert_eq!("light-red".parse(), Ok(Color::LightRed));
/// assert_eq!("light_red".parse(), Ok(Color::LightRed));
/// assert_eq!("lightRed".parse(), Ok(Color::LightRed));
/// assert_eq!("bright red".parse(), Ok(Color::LightRed));
/// assert_eq!("bright-red".parse(), Ok(Color::LightRed));
/// assert_eq!("silver".parse(), Ok(Color::Gray));
/// assert_eq!("dark-grey".parse(), Ok(Color::DarkGray));
/// assert_eq!("dark gray".parse(), Ok(Color::DarkGray));
/// assert_eq!("light-black".parse(), Ok(Color::DarkGray));
/// assert_eq!("white".parse(), Ok(Color::White));
/// assert_eq!("bright white".parse(), Ok(Color::White));
/// ```
///
/// [ANSI color table]: https://en.wikipedia.org/wiki/ANSI_escape_code#Colors
#[derive(
    Debug, Default, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub enum Color {
    /// Resets the foreground or background color
    #[default]
    Reset,
    /// ANSI Color: Black. Foreground: 30, Background: 40
    Black,
    /// ANSI Color: Red. Foreground: 31, Background: 41
    Red,
    /// ANSI Color: Green. Foreground: 32, Background: 42
    Green,
    /// ANSI Color: Yellow. Foreground: 33, Background: 43
    Yellow,
    /// ANSI Color: Blue. Foreground: 34, Background: 44
    Blue,
    /// ANSI Color: Magenta. Foreground: 35, Background: 45
    Magenta,
    /// ANSI Color: Cyan. Foreground: 36, Background: 46
    Cyan,
    /// ANSI Color: White. Foreground: 37, Background: 47
    ///
    /// Note that this is sometimes called `silver` or `white` but we use
    /// `white` for bright white
    Gray,
    /// ANSI Color: Bright Black. Foreground: 90, Background: 100
    ///
    /// Note that this is sometimes called `light black` or `bright black` but
    /// we use `dark gray`
    DarkGray,
    /// ANSI Color: Bright Red. Foreground: 91, Background: 101
    LightRed,
    /// ANSI Color: Bright Green. Foreground: 92, Background: 102
    LightGreen,
    /// ANSI Color: Bright Yellow. Foreground: 93, Background: 103
    LightYellow,
    /// ANSI Color: Bright Blue. Foreground: 94, Background: 104
    LightBlue,
    /// ANSI Color: Bright Magenta. Foreground: 95, Background: 105
    LightMagenta,
    /// ANSI Color: Bright Cyan. Foreground: 96, Background: 106
    LightCyan,
    /// ANSI Color: Bright White. Foreground: 97, Background: 107
    /// Sometimes called `bright white` or `light white` in some terminals
    White,
    /// An RGB color.
    ///
    /// Note that only terminals that support 24-bit true color will display
    /// this correctly. Notably versions of Windows Terminal prior to
    /// Windows 10 and macOS Terminal.app do not support this.
    ///
    /// If the terminal does not support true color, code using the
    /// [`TermwizBackend`] will fallback to the default text color.
    /// Crossterm and Termion do not have this capability and the display
    /// will be unpredictable (e.g. Terminal.app may display glitched blinking
    /// text). See <https://github.com/ratatui/ratatui/issues/475> for an example of this problem.
    ///
    /// See also: <https://en.wikipedia.org/wiki/ANSI_escape_code#24-bit>
    ///
    /// [`TermwizBackend`]: crate::backend::TermwizBackend
    Rgb(u8, u8, u8),
    /// An 8-bit 256 color.
    ///
    /// See also <https://en.wikipedia.org/wiki/ANSI_escape_code#8-bit>
    Indexed(u8),
}

/// Error type indicating a failure to parse a color string.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct ParseColorError;

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Failed to parse color")
    }
}

impl std::error::Error for ParseColorError {}

/// Converts a string representation to a `Color` instance.
///
/// The `from_str` function attempts to parse the given string and convert it to
/// the corresponding `Color` variant. It supports named colors, RGB values, and
/// indexed colors. If the string cannot be parsed, a `ParseColorError` is
/// returned.
///
/// See the [`Color`] documentation for more information on the supported color
/// names.
///
/// # Examples
///
/// ```
/// use slumber_config::Color;
/// use std::str::FromStr;
///
/// let color: Color = Color::from_str("blue").unwrap();
/// assert_eq!(color, Color::Blue);
///
/// let color: Color = Color::from_str("#FF0000").unwrap();
/// assert_eq!(color, Color::Rgb(255, 0, 0));
///
/// let color: Color = Color::from_str("10").unwrap();
/// assert_eq!(color, Color::Indexed(10));
///
/// let color: Result<Color, _> = Color::from_str("invalid_color");
/// assert!(color.is_err());
/// ```
impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(
            // There is a mix of different color names and formats in the wild.
            // This is an attempt to support as many as possible.
            match s
                .to_lowercase()
                .replace([' ', '-', '_'], "")
                .replace("bright", "light")
                .replace("grey", "gray")
                .replace("silver", "gray")
                .replace("lightblack", "darkgray")
                .replace("lightwhite", "white")
                .replace("lightgray", "white")
                .as_ref()
            {
                "reset" => Self::Reset,
                "black" => Self::Black,
                "red" => Self::Red,
                "green" => Self::Green,
                "yellow" => Self::Yellow,
                "blue" => Self::Blue,
                "magenta" => Self::Magenta,
                "cyan" => Self::Cyan,
                "gray" => Self::Gray,
                "darkgray" => Self::DarkGray,
                "lightred" => Self::LightRed,
                "lightgreen" => Self::LightGreen,
                "lightyellow" => Self::LightYellow,
                "lightblue" => Self::LightBlue,
                "lightmagenta" => Self::LightMagenta,
                "lightcyan" => Self::LightCyan,
                "white" => Self::White,
                _ => {
                    if let Ok(index) = s.parse::<u8>() {
                        Self::Indexed(index)
                    } else if let Some((r, g, b)) = parse_hex_color(s) {
                        Self::Rgb(r, g, b)
                    } else {
                        return Err(ParseColorError);
                    }
                }
            },
        )
    }
}

fn parse_hex_color(input: &str) -> Option<(u8, u8, u8)> {
    if !input.starts_with('#') || input.len() != 7 {
        return None;
    }
    let r = u8::from_str_radix(input.get(1..3)?, 16).ok()?;
    let g = u8::from_str_radix(input.get(3..5)?, 16).ok()?;
    let b = u8::from_str_radix(input.get(5..7)?, 16).ok()?;
    Some((r, g, b))
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reset => write!(f, "Reset"),
            Self::Black => write!(f, "Black"),
            Self::Red => write!(f, "Red"),
            Self::Green => write!(f, "Green"),
            Self::Yellow => write!(f, "Yellow"),
            Self::Blue => write!(f, "Blue"),
            Self::Magenta => write!(f, "Magenta"),
            Self::Cyan => write!(f, "Cyan"),
            Self::Gray => write!(f, "Gray"),
            Self::DarkGray => write!(f, "DarkGray"),
            Self::LightRed => write!(f, "LightRed"),
            Self::LightGreen => write!(f, "LightGreen"),
            Self::LightYellow => write!(f, "LightYellow"),
            Self::LightBlue => write!(f, "LightBlue"),
            Self::LightMagenta => write!(f, "LightMagenta"),
            Self::LightCyan => write!(f, "LightCyan"),
            Self::White => write!(f, "White"),
            Self::Rgb(r, g, b) => write!(f, "#{r:02X}{g:02X}{b:02X}"),
            Self::Indexed(i) => write!(f, "{i}"),
        }
    }
}

// For serialization
impl From<Color> for String {
    fn from(color: Color) -> Self {
        color.to_string()
    }
}

// For deserialization
impl TryFrom<String> for Color {
    type Error = <Self as FromStr>::Err;

    fn try_from(color: String) -> Result<Self, Self::Error> {
        color.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::{Deserialize, IntoDeserializer};
    use std::error::Error;

    #[test]
    fn from_rgb_color() {
        let color: Color = Color::from_str("#FF0000").unwrap();
        assert_eq!(color, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn from_indexed_color() {
        let color: Color = Color::from_str("10").unwrap();
        assert_eq!(color, Color::Indexed(10));
    }

    #[test]
    fn from_ansi_color() -> Result<(), Box<dyn Error>> {
        assert_eq!(Color::from_str("reset")?, Color::Reset);
        assert_eq!(Color::from_str("black")?, Color::Black);
        assert_eq!(Color::from_str("red")?, Color::Red);
        assert_eq!(Color::from_str("green")?, Color::Green);
        assert_eq!(Color::from_str("yellow")?, Color::Yellow);
        assert_eq!(Color::from_str("blue")?, Color::Blue);
        assert_eq!(Color::from_str("magenta")?, Color::Magenta);
        assert_eq!(Color::from_str("cyan")?, Color::Cyan);
        assert_eq!(Color::from_str("gray")?, Color::Gray);
        assert_eq!(Color::from_str("darkgray")?, Color::DarkGray);
        assert_eq!(Color::from_str("lightred")?, Color::LightRed);
        assert_eq!(Color::from_str("lightgreen")?, Color::LightGreen);
        assert_eq!(Color::from_str("lightyellow")?, Color::LightYellow);
        assert_eq!(Color::from_str("lightblue")?, Color::LightBlue);
        assert_eq!(Color::from_str("lightmagenta")?, Color::LightMagenta);
        assert_eq!(Color::from_str("lightcyan")?, Color::LightCyan);
        assert_eq!(Color::from_str("white")?, Color::White);

        // aliases
        assert_eq!(Color::from_str("lightblack")?, Color::DarkGray);
        assert_eq!(Color::from_str("lightwhite")?, Color::White);
        assert_eq!(Color::from_str("lightgray")?, Color::White);

        // silver = grey = gray
        assert_eq!(Color::from_str("grey")?, Color::Gray);
        assert_eq!(Color::from_str("silver")?, Color::Gray);

        // spaces are ignored
        assert_eq!(Color::from_str("light black")?, Color::DarkGray);
        assert_eq!(Color::from_str("light white")?, Color::White);
        assert_eq!(Color::from_str("light gray")?, Color::White);

        // dashes are ignored
        assert_eq!(Color::from_str("light-black")?, Color::DarkGray);
        assert_eq!(Color::from_str("light-white")?, Color::White);
        assert_eq!(Color::from_str("light-gray")?, Color::White);

        // underscores are ignored
        assert_eq!(Color::from_str("light_black")?, Color::DarkGray);
        assert_eq!(Color::from_str("light_white")?, Color::White);
        assert_eq!(Color::from_str("light_gray")?, Color::White);

        // bright = light
        assert_eq!(Color::from_str("bright-black")?, Color::DarkGray);
        assert_eq!(Color::from_str("bright-white")?, Color::White);

        // bright = light
        assert_eq!(Color::from_str("brightblack")?, Color::DarkGray);
        assert_eq!(Color::from_str("brightwhite")?, Color::White);

        Ok(())
    }

    #[test]
    fn from_invalid_colors() {
        let bad_colors = [
            "invalid_color", // not a color string
            "abcdef0",       // 7 chars is not a color
            " bcdefa",       // doesn't start with a '#'
            "#abcdef00",     // too many chars
            "#1🦀2",         // len 7 but on char boundaries shouldnt panic
            "resett",        // typo
            "lightblackk",   // typo
        ];

        for bad_color in bad_colors {
            assert!(
                Color::from_str(bad_color).is_err(),
                "bad color: '{bad_color}'"
            );
        }
    }

    #[test]
    fn display() {
        assert_eq!(format!("{}", Color::Black), "Black");
        assert_eq!(format!("{}", Color::Red), "Red");
        assert_eq!(format!("{}", Color::Green), "Green");
        assert_eq!(format!("{}", Color::Yellow), "Yellow");
        assert_eq!(format!("{}", Color::Blue), "Blue");
        assert_eq!(format!("{}", Color::Magenta), "Magenta");
        assert_eq!(format!("{}", Color::Cyan), "Cyan");
        assert_eq!(format!("{}", Color::Gray), "Gray");
        assert_eq!(format!("{}", Color::DarkGray), "DarkGray");
        assert_eq!(format!("{}", Color::LightRed), "LightRed");
        assert_eq!(format!("{}", Color::LightGreen), "LightGreen");
        assert_eq!(format!("{}", Color::LightYellow), "LightYellow");
        assert_eq!(format!("{}", Color::LightBlue), "LightBlue");
        assert_eq!(format!("{}", Color::LightMagenta), "LightMagenta");
        assert_eq!(format!("{}", Color::LightCyan), "LightCyan");
        assert_eq!(format!("{}", Color::White), "White");
        assert_eq!(format!("{}", Color::Indexed(10)), "10");
        assert_eq!(format!("{}", Color::Rgb(255, 0, 0)), "#FF0000");
        assert_eq!(format!("{}", Color::Reset), "Reset");
    }

    #[test]
    fn deserialize() -> Result<(), serde::de::value::Error> {
        assert_eq!(
            Color::Black,
            Color::deserialize("Black".into_deserializer())?
        );
        assert_eq!(
            Color::Magenta,
            Color::deserialize("magenta".into_deserializer())?
        );
        assert_eq!(
            Color::LightGreen,
            Color::deserialize("LightGreen".into_deserializer())?
        );
        assert_eq!(
            Color::White,
            Color::deserialize("bright-white".into_deserializer())?
        );
        assert_eq!(
            Color::Indexed(42),
            Color::deserialize("42".into_deserializer())?
        );
        assert_eq!(
            Color::Rgb(0, 255, 0),
            Color::deserialize("#00ff00".into_deserializer())?
        );
        Ok(())
    }

    #[test]
    fn deserialize_error() {
        let color: Result<_, serde::de::value::Error> =
            Color::deserialize("invalid".into_deserializer());
        assert!(color.is_err());

        let color: Result<_, serde::de::value::Error> =
            Color::deserialize("#00000000".into_deserializer());
        assert!(color.is_err());
    }
}
