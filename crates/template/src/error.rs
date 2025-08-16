use bytes::Bytes;
use std::{collections::HashMap, fmt::Display, string::FromUtf8Error};
use thiserror::Error;
use tracing::error;
use winnow::error::{ContextError, ParseError};

use crate::{Identifier, Value};
use serde::de;

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
    /// 2+ futures were rendering the same profile field. One future was doing
    /// the actual rendering and the rest were waiting on the first. If the
    /// first one fails, the rest will return this error. Theoretically this
    /// will never actually be emitted because `try_join` should return after
    /// the initial error, so this is a placeholder.
    #[error("Error rendering cached profile field `{field}`")]
    CacheFailed { field: Identifier },

    /// A profile field key contained an unknown field
    #[error("Unknown field `{field}`")]
    FieldUnknown { field: String },

    /// An bubbled-up error from rendering a profile field value
    #[error("Rendering nested template for field `{field}`")]
    FieldNested {
        field: String,
        #[source]
        error: Box<Self>,
    },

    /// No function by this name
    #[error("Unknown function `{name}`")]
    FunctionUnknown { name: Identifier },

    /// In many contexts, the render output needs to be usable as a string.
    /// This error occurs when we wanted to render to a string, but whatever
    /// bytes we got were not valid UTF-8. The underlying error message is
    /// descriptive enough so we don't need to give additional context.
    #[error(transparent)]
    InvalidUtf8(#[from] FromUtf8Error),

    /// Error parsing JSON data
    #[error("Error parsing bytes as JSON: `{data:?}`")]
    JsonDeserialize {
        data: Bytes,
        #[source]
        error: serde_json::Error,
    },

    /// Not enough arguments provided to a function call
    #[error("Not enough arguments")]
    NotEnoughArguments,

    /// External error type from a function call
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),

    /// Unexpected arguments passed to function
    #[error(
        "Unexpected arguments passed to function: {position:?}, {keyword:?}"
    )]
    TooManyArguments {
        position: Vec<Value>,
        keyword: HashMap<String, Value>,
    },

    /// Function expected one type but a value of a different type was given
    #[error("Type error; expected `{expected}`, got `{actual}`")]
    Type {
        expected: &'static str,
        actual: Value,
    },
}

impl RenderError {
    /// Create a [RenderError::Other] from another error
    pub fn other(
        error: impl 'static + Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Other(error.into())
    }
}

impl de::Error for RenderError {
    fn custom<T>(msg: T) -> Self
    where
        T: Display,
    {
        RenderError::Other(msg.to_string().into())
    }
}
