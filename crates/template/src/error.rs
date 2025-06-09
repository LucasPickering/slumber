use std::{collections::HashMap, string::FromUtf8Error};
use thiserror::Error;
use tracing::error;
use winnow::error::{ContextError, ParseError};

use crate::{Identifier, Value};

/// An error while parsing a template. This is derived from a nom error
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
pub enum TemplateError {
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

    /// In many contexts, the render output needs to be usable as a string.
    /// This error occurs when we wanted to render to a string, but whatever
    /// bytes we got were not valid UTF-8. The underlying error message is
    /// descriptive enough so we don't need to give additional context.
    #[error(transparent)]
    InvalidUtf8(#[from] FromUtf8Error),

    /// TODO comment
    /// TODO store expected/actual
    #[error("TODO not enough")]
    NotEnoughArguments,

    /// TODO
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),

    /// TODO comment
    /// TODO store expected/actual
    #[error("TODO too many arguments")]
    TooManyArguments {
        position: Vec<Value>,
        keyword: HashMap<String, Value>,
    },

    #[error("Type error; expected `{expected}`, got `{actual}`")]
    Type {
        expected: &'static str,
        actual: Value,
    },

    /// TODO
    #[error("Unknown function `{name}`")]
    UnknownFunction { name: Identifier },
}

impl TemplateError {
    /// TODO
    pub fn other(
        error: impl 'static + std::error::Error + Send + Sync,
    ) -> Self {
        Self::Other(Box::new(error))
    }
}
