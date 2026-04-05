//! Template and value stringification

use crate::{
    Expression, FunctionCall, Literal, Template, TemplateChunk, Value,
    parse::{ESCAPE, EXPRESSION_CLOSE, EXPRESSION_OPEN, NULL},
};
use itertools::Itertools;
use regex::Regex;
use std::{
    borrow::Cow,
    fmt::{self, Display, Write},
    sync::LazyLock,
};

impl Template {
    /// Convert the template to a string. This will only allocate for escaped or
    /// dynamic templates. This is not guaranteed to return the exact string
    /// that was parsed to create the template, as whitespace within expressions
    /// is variable.
    pub fn display(&self) -> Cow<'_, str> {
        let mut buf = Cow::Borrowed("");

        // Re-stringify the template
        for chunk in &self.chunks {
            match chunk {
                TemplateChunk::Raw(s) => {
                    // Add underscores between { to escape them. Any sequence
                    // of {_* followed by another { needs to be escaped. Regex
                    // matches have to be non-overlapping so we can't just use
                    // {_*{, because that wouldn't catch cases like {_{_{. So
                    // we have to do our own lookahead.
                    //
                    // Keep in mind that escape sequences are going to be an
                    // extreme rarity, so we need to optimize for the case where
                    // there are none and only allocate when necessary.
                    static REGEX: LazyLock<Regex> =
                        LazyLock::new(|| Regex::new(r"\{_*").unwrap());
                    // Track how far into s we've copied, so we can do as few
                    // copies as possible
                    let mut last_copied = 0;
                    for m in REGEX.find_iter(s) {
                        let rest = &s[m.end()..];
                        // Don't allocate until we know this needs an escape
                        // sequence
                        if rest.starts_with('{') {
                            let buf = buf.to_mut();
                            buf.push_str(&s[last_copied..m.end()]);
                            buf.push('_');
                            last_copied = m.end();
                        }
                    }

                    // If this is the first chunk and there were no regex
                    // matches, don't allocate yet
                    if buf.is_empty() {
                        buf = Cow::Borrowed(s);
                    } else {
                        // Fencepost: get everything from the last escape
                        // sequence to the end
                        buf.to_mut().push_str(&s[last_copied..]);
                    }
                }
                TemplateChunk::Expression(expression) => {
                    // If the previous chunk ends with a potential escape
                    // sequence, add an underscore to escape the upcoming key
                    static REGEX: LazyLock<Regex> =
                        LazyLock::new(|| Regex::new(r"\{_*$").unwrap());
                    if REGEX.is_match(&buf) {
                        buf.to_mut().push_str(ESCAPE);
                    }

                    write!(
                        buf.to_mut(),
                        "{EXPRESSION_OPEN} {expression} {EXPRESSION_CLOSE}"
                    )
                    .unwrap();
                }
            }
        }

        buf
    }
}

impl Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(literal) => write!(f, "{literal}"),
            Self::Field(identifier) => write!(f, "{identifier}"),
            Self::Array(expressions) => {
                write!(f, "[{}]", expressions.iter().format(", "))
            }
            Self::Object(entries) => {
                write!(
                    f,
                    "{{{}}}",
                    entries.iter().format_with(", ", |(key, value), f| f(
                        &format_args!("{key}: {value}")
                    ))
                )
            }
            Self::Call(call) => write!(f, "{call}"),
            Self::Pipe { expression, call } => {
                write!(f, "{expression} | {call}")
            }
        }
    }
}

impl Display for Literal {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Null => write!(fmt, "null"),
            Literal::Boolean(b) => write!(fmt, "{b}"),
            Literal::Integer(i) => write!(fmt, "{i}"),
            // Always show ".0" for floats that are whole numbers to
            // distinguish from ints
            Literal::Float(f) => {
                if f.fract() == 0.0 {
                    write!(fmt, "{f:.1}")
                } else {
                    write!(fmt, "{f}")
                }
            }
            Literal::String(s) => fmt_string(fmt, s),
            Self::Bytes(bytes) => fmt_bytes(fmt, bytes),
        }
    }
}

impl Display for FunctionCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        enum Argument<'a> {
            Position(&'a Expression),
            Keyword(&'a str, &'a Expression),
        }

        impl Display for Argument<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    Self::Position(expression) => write!(f, "{expression}"),
                    Self::Keyword(key, expression) => {
                        write!(f, "{key}={expression}")
                    }
                }
            }
        }

        write!(
            f,
            "{}({})",
            self.function,
            self.position
                .iter()
                .map(Argument::Position)
                .chain(self.keyword.iter().map(|(name, expression)| {
                    Argument::Keyword(name, expression)
                }))
                .join(", ")
        )
    }
}

impl Display for Value {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(fmt, "{NULL}"),
            Self::Boolean(b) => write!(fmt, "{b}"),
            Self::Integer(i) => write!(fmt, "{i}"),
            Self::Float(f) => write!(fmt, "{f}"),
            Self::String(s) => fmt_string(fmt, s),
            Self::Bytes(bytes) => fmt_bytes(fmt, bytes),
            Self::Array(array) => {
                write!(fmt, "[{}]", array.iter().format(", "))
            }
            Self::Object(object) => {
                write!(
                    fmt,
                    "{{{}}}",
                    object.iter().format_with(", ", |(k, v), f| f(
                        &format_args!("{k}: {v}")
                    ))
                )
            }
        }
    }
}

/// Format a string value/literal. Always format with single quotes because it's
/// simple and more compatible with YAML than double quotes
fn fmt_string(fmt: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    write!(fmt, "'")?;
    for c in s.chars() {
        match c {
            // Escape characters as needed
            '\'' => write!(fmt, "\\'"),
            '\\' => write!(fmt, "\\\\"),
            '\n' => write!(fmt, "\\n"),
            '\r' => write!(fmt, "\\r"),
            '\t' => write!(fmt, "\\t"),
            _ => write!(fmt, "{c}"),
        }?;
    }
    write!(fmt, "'")?;
    Ok(())
}

/// Format a byte value/literal. Always format with single quotes because it's
/// simple and more compatible with YAML than double quotes
fn fmt_bytes(fmt: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    write!(fmt, "b'")?;
    for byte in bytes {
        match *byte {
            // Escape visible characters
            b'\'' => write!(fmt, "\\'")?,
            b'\\' => write!(fmt, "\\\\")?,
            // If the byte is printable ASCII, print it
            byte if byte.is_ascii() && !byte.is_ascii_control() => {
                write!(fmt, "{}", byte as char)?;
            }
            // Otherwise print the raw byte value
            byte => write!(fmt, "\\x{byte:02x}")?,
        }
    }
    write!(fmt, "'")?;
    Ok(())
}
