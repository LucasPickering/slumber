use crate::{Expression, Identifier, Value};
use derive_more::derive::Display;
use indexmap::IndexMap;
use itertools::Itertools;
use serde::de;
use std::{
    fmt::Display,
    num::{ParseFloatError, ParseIntError},
    str::Utf8Error,
};
use thiserror::Error;
use tracing::error;
use winnow::error::{ContextError, ParseError};

/// An error while parsing a template. The string is provided by winnow
#[derive(Debug, Error)]
#[error("{0}")]
pub struct TemplateParseError(String);

/// Convert winnow's error type into ours. This stringifies the error so we can
/// dump the reference to the input
impl From<ParseError<&str, ContextError>> for TemplateParseError {
    fn from(error: ParseError<&str, ContextError>) -> Self {
        Self(error.to_string())
    }
}

/// Any error that can occur during template rendering.
///
/// The error always holds owned data so it can be detached from the lifetime
/// of the template and context. This requires a mild amount of cloning in error
/// cases, but those should be infrequent so it's fine.
///
/// These error messages are generally shown with additional parent context, so
/// they should be pretty brief.
#[derive(Debug, Error)]
pub enum RenderError {
    /// A profile field key contained an unknown field
    #[error("Unknown field `{field}`")]
    FieldUnknown { field: Identifier },

    /// An bubbled-up error from rendering a profile field value
    #[error("Rendering nested template for field `{field}`")]
    FieldNested {
        field: String,
        #[source]
        error: Box<Self>,
    },

    /// No function by this name. Name doesn't need to be given because this
    /// will be wrapped in the `Function` variant
    #[error("Unknown function")]
    FunctionUnknown,

    /// External error type from a function call
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),

    /// Not enough arguments provided to a function call
    #[error("Not enough arguments")]
    TooFewArguments,

    /// Unexpected arguments passed to function
    #[error("Extra arguments {}", extra_args(.position, .keyword))]
    TooManyArguments {
        position: Vec<Value>,
        keyword: IndexMap<String, Value>,
    },

    /// Error converting a [Value] to another type
    #[error(transparent)]
    Value(#[from] ValueError),

    /// An error with additional context attached. Used to locate errors in
    /// function calls that could be deeply nested
    #[error("{context}")]
    WithContext {
        context: Box<RenderErrorContext>,
        #[source]
        error: Box<Self>,
    },
}

impl RenderError {
    /// Create a [RenderError::Other] from another error
    pub fn other(
        error: impl 'static + Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Other(error.into())
    }

    /// Attach context to this error
    #[must_use]
    pub fn context(self, context: RenderErrorContext) -> Self {
        Self::WithContext {
            context: Box::new(context),
            error: Box::new(self),
        }
    }
}

/// Information about where an error occurred
#[derive(Debug, Display)]
pub enum RenderErrorContext {
    /// Error in a function call expression
    #[display("{_0}()")]
    Function(Identifier),

    /// Error rendering an argument expression
    #[display("argument {argument}={expression}")]
    ArgumentRender {
        argument: String,
        expression: Expression,
    },

    /// Error while converting an argument value into whatever type the function
    /// wants
    #[display("argument {argument}={value}")]
    ArgumentConvert { argument: String, value: Value },
}

/// Format the extra positional and/or keyword arguments given in a function
/// call
fn extra_args<'a>(
    position: &'a [Value],
    keyword: &'a IndexMap<String, Value>,
) -> impl 'a + Display {
    // Build a list like `1, 2, a=3, b=4`
    position
        .iter()
        .map(|arg| format!("{arg}"))
        .chain(
            keyword
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        )
        .format(", ")
}

/// An error with a value attached. Use this for errors that originated from a
/// particular value, so that the offending value can be included in the error
/// message. This does not implement `Error` itself as it's just meant as a
/// container to pass an error+value together. It should be unpacked into
/// another error variant to provide better context to the user.
#[derive(Debug)]
pub struct WithValue<E> {
    /// Value that failed to convert
    pub value: Value,
    /// The error that occurred during conversion. This error is transparent,
    /// meaning we include its message in our own `Display` impl and
    /// `Error::source` returns its source
    pub error: E,
}

impl<E> WithValue<E> {
    /// Pair a value with the error it generated
    pub fn new(value: Value, error: impl Into<E>) -> Self {
        Self {
            value,
            error: error.into(),
        }
    }

    /// Move the inner error out
    pub fn into_error(self) -> E {
        self.error
    }
}

/// An error that can occur while converting from [Value] to some other type.
/// This is returned from [TryFromValue](crate::TryFromValue).
#[derive(Debug, Error)]
pub enum ValueError {
    /// Failed to parse a string to a float
    #[error(transparent)]
    Float(#[from] ParseFloatError),

    /// Failed to parse a string to an integer
    #[error(transparent)]
    Integer(#[from] ParseIntError),

    /// In many contexts, the render output needs to be usable as a string.
    /// This error occurs when we wanted to render to a string, but whatever
    /// bytes we got were not valid UTF-8. The underlying error message is
    /// descriptive enough so we don't need to give additional context.
    #[error(transparent)]
    InvalidUtf8(#[from] Utf8Error),

    /// Error parsing JSON data
    #[error("Error parsing JSON")]
    Json(
        #[from]
        #[source]
        serde_json::Error,
    ),

    /// External error type
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),

    /// Function expected one type but a value of a different type was given
    #[error("Expected {expected}")]
    Type { expected: Expected },
}

impl ValueError {
    /// Create a [Self::Other] from another error
    pub fn other(
        error: impl 'static + Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Other(error.into())
    }
}

impl de::Error for ValueError {
    fn custom<T>(msg: T) -> Self
    where
        T: Display,
    {
        Self::Other(msg.to_string().into())
    }
}

/// When a value of a particular type is expected but something else is given
#[derive(Debug, derive_more::Display)]
pub enum Expected {
    #[display("null")]
    Null,
    #[display("boolean")]
    Boolean,
    #[display("integer")]
    Integer,
    #[display("float")]
    Float,
    #[display("string")]
    String,
    /// Array of any type
    #[display("array")]
    Array,
    /// Union
    #[display("one of {}", display_union(_0))]
    OneOf(&'static [&'static Self]),
    /// User-provided descriptor of what they wanted
    #[display("{_0}")]
    Custom(&'static str),
}

/// Display a union list of values
fn display_union(values: &[impl Display]) -> String {
    match values {
        [] => String::new(),
        [value] => value.to_string(),
        [a, b] => format!("{a} or {b}"),
        [head @ .., tail] => format!("{}, or {tail}", head.iter().join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::empty(&[], "")]
    #[case::one(&["a"], "a")]
    #[case::two(&["a", "b"], "a or b")]
    #[case::three(&["a", "b", "c"], "a, b, or c")]
    fn test_display_union(#[case] values: &[&str], #[case] expected: &str) {
        assert_eq!(display_union(values), expected);
    }
}
