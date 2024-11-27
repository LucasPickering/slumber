use crate::{
    http::{RequestBuildError, RequestError},
    lua::{FunctionError, LuaError},
};
use std::{string::FromUtf8Error, sync::Arc};
use thiserror::Error;
use winnow::error::{ContextError, ParseError};

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

/// Any error that can occur during template rendering. The purpose of having a
/// structured error here (while the rest of the app just uses `anyhow`) is to
/// support localized error display in the UI, e.g. showing just one portion of
/// a string in red if that particular template key failed to render.
///
/// The error always holds owned data so it can be detached from the lifetime
/// of the template context. This requires a mild amount of cloning in error
/// cases, but those should be infrequent so it's fine.
#[derive(Debug, Error)]
pub enum TemplateError {
    /// In many contexts, the render output needs to be usable as a string.
    /// This error occurs when we wanted to render to a string, but whatever
    /// bytes we got were not valid UTF-8. The underlying error message is
    /// descriptive enough so we don't need to give additional context.
    #[error(transparent)]
    InvalidUtf8(#[from] FromUtf8Error),

    /// Error occurred in a Lua expression
    #[error(transparent)]
    Lua(#[from] LuaError),
}

impl TemplateError {
    /// Does the given error have *any* error in its chain that contains
    /// [TriggeredRequestError::NotAllowed]? This makes it easy to attach
    /// additional error context.
    pub fn has_trigger_disabled_error(error: &anyhow::Error) -> bool {
        error.chain().any(|error| {
            matches!(
                error.downcast_ref::<FunctionError>(),
                Some(FunctionError::Trigger {
                    error: TriggeredRequestError::NotAllowed,
                    ..
                })
            )
        })
    }
}

#[cfg(test)]
impl PartialEq for TemplateError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

/// Error occurred while trying to build/execute a triggered request.
///
/// This type implements `Clone` so it can be shared between deduplicated chain
/// renders, hence the `Arc`s on inner errors.
#[derive(Clone, Debug, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TriggeredRequestError {
    /// This render was invoked in a way that doesn't support automatic request
    /// execution. In some cases the user needs to explicitly opt in to enable
    /// it (e.g. with a CLI flag)
    #[error("Triggered request execution not allowed in this context")]
    NotAllowed,

    /// Tried to auto-execute a chained request but couldn't build it
    #[error(transparent)]
    Build(#[from] Arc<RequestBuildError>),

    /// Chained request was triggered, sent and failed
    #[error(transparent)]
    Send(#[from] Arc<RequestError>),
}
