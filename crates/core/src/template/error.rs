use crate::{
    collection::{ChainId, ProfileId, RecipeId},
    http::{RequestBuildError, RequestError, query::QueryError},
    template::TemplateKey,
};
use itertools::Itertools;
use slumber_util::doc_link;
use std::{fmt::Display, io, path::PathBuf, string::FromUtf8Error, sync::Arc};
use thiserror::Error;
use tracing::error;
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
///
/// These error messages are generally shown with additional parent context, so
/// they should be pretty brief.
///
/// This type implements `Clone` so it can be shared between deduplicated chain
/// renders.
#[derive(Clone, Debug, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TemplateError {
    /// Tried to load profile data with no profile selected
    #[error("No profile selected")]
    NoProfileSelected,

    /// Unknown profile ID
    #[error("Unknown profile `{profile_id}`")]
    ProfileUnknown { profile_id: ProfileId },

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
    InvalidUtf8(FromUtf8Error),

    /// Cycle detected in nested template keys. We store the entire cycle stack
    /// for presentation
    #[error("Infinite loop detected in template: {}", format_cycle(.0))]
    InfiniteLoop(Vec<TemplateKey>),

    #[error("Resolving chain `{chain_id}`")]
    Chain {
        chain_id: ChainId,
        #[source]
        error: ChainError,
    },
}

/// An error sub-type, for any error that occurs while resolving a chained
/// value. This is factored out because they all need to be paired with a chain
/// ID.
///
/// This type implements `Clone` so it can be shared between deduplicated chain
/// renders, hence the `Arc`s on inner errors.
#[derive(Clone, Debug, Error)]
pub enum ChainError {
    /// Reference to a chain that doesn't exist
    #[error("Unknown chain: {_0}")]
    ChainUnknown(ChainId),

    /// Reference to a recipe that doesn't exist
    #[error("Unknown request recipe: {_0}")]
    RecipeUnknown(RecipeId),

    /// An error occurred accessing the persistence database. This error is
    /// generated by our code so we don't need any extra context.
    #[error(transparent)]
    Database(Arc<anyhow::Error>),

    /// The chain ID is valid, but the corresponding recipe has no successful
    /// response
    #[error("No response available")]
    NoResponse,

    /// Couldn't guess content type from request/file/etc. metadata
    #[error(
        "Selector cannot be applied; content type not provided and could not \
        be determined from metadata. See docs for supported content types: {}",
        doc_link("api/request_collection/content_type")
    )]
    UnknownContentType,

    /// Something bad happened while triggering a request dependency
    #[error("Triggering upstream recipe `{recipe_id}`")]
    Trigger {
        recipe_id: RecipeId,
        #[source]
        error: TriggeredRequestError,
    },

    /// Failed to parse the response body before applying a selector
    #[error("Parsing response")]
    ParseResponse {
        #[source]
        error: Arc<anyhow::Error>,
    },

    /// Got either 0 or 2+ results for JSON path query. This is generated by
    /// internal code so we don't need extra context
    #[error(transparent)]
    Query(#[from] QueryError),

    /// User gave an empty list for the command
    #[error("No command given")]
    CommandMissing,

    /// Error executing an external command
    #[error("Executing command {command:?}")]
    Command {
        command: Vec<String>,
        #[source]
        error: Arc<io::Error>,
    },

    /// Error opening/reading a file
    #[error("Reading file `{path}`")]
    File {
        path: PathBuf,
        #[source]
        error: Arc<io::Error>,
    },

    /// Never got a response from the prompt channel. Do *not* store the
    /// `RecvError` here, because it provides useless extra output to the user.
    #[error("No response from prompt/select")]
    PromptNoResponse,

    /// We hit some sort of deserialization error while trying to build dynamic
    /// options
    #[error("Dynamic option list failed to deserialize as JSON")]
    DynamicSelectOptions {
        #[source]
        error: Arc<serde_json::Error>,
    },

    /// A bubbled-up error from rendering a nested template in the chain
    /// arguments
    #[error("Rendering nested template for field `{field}`")]
    Nested {
        /// Specific field that contained the error, to give the user context
        field: String,
        #[source]
        error: Box<TemplateError>,
    },

    /// Specified !header did not exist in the response
    #[error("Header `{header}` not in response")]
    MissingHeader { header: String },
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

impl From<RequestBuildError> for TriggeredRequestError {
    fn from(error: RequestBuildError) -> Self {
        Self::Build(error.into())
    }
}

impl From<RequestError> for TriggeredRequestError {
    fn from(error: RequestError) -> Self {
        Self::Send(error.into())
    }
}

/// Placeholder implementation to allow equality checks for *other*
/// `TemplateError` variants. This one is hard to do because `anyhow::Error`
/// doesn't impl `PartialEq`
#[cfg(test)]
impl PartialEq for ChainError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ChainUnknown(l0), Self::ChainUnknown(r0)) => l0 == r0,
            (Self::RecipeUnknown(l0), Self::RecipeUnknown(r0)) => l0 == r0,
            (Self::Database(l0), Self::Database(r0)) => Arc::ptr_eq(l0, r0),
            (
                Self::Trigger {
                    recipe_id: l_recipe_id,
                    error: l_error,
                },
                Self::Trigger {
                    recipe_id: r_recipe_id,
                    error: r_error,
                },
            ) => l_recipe_id == r_recipe_id && l_error == r_error,
            (
                Self::ParseResponse { error: l_error },
                Self::ParseResponse { error: r_error },
            ) => Arc::ptr_eq(l_error, r_error),
            (Self::Query(l0), Self::Query(r0)) => l0 == r0,
            (
                Self::Command {
                    command: l_command,
                    error: l_error,
                },
                Self::Command {
                    command: r_command,
                    error: r_error,
                },
            ) => l_command == r_command && Arc::ptr_eq(l_error, r_error),
            (
                Self::File {
                    path: l_path,
                    error: l_error,
                },
                Self::File {
                    path: r_path,
                    error: r_error,
                },
            ) => l_path == r_path && Arc::ptr_eq(l_error, r_error),
            (
                Self::Nested {
                    field: l_field,
                    error: l_error,
                },
                Self::Nested {
                    field: r_field,
                    error: r_error,
                },
            ) => l_field == r_field && l_error == r_error,
            (
                Self::MissingHeader { header: l_header },
                Self::MissingHeader { header: r_header },
            ) => l_header == r_header,
            _ => {
                core::mem::discriminant(self) == core::mem::discriminant(other)
            }
        }
    }
}

fn format_cycle(stack: &[TemplateKey]) -> impl '_ + Display {
    stack.iter().format(" -> ")
}
