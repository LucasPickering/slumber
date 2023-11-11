use serde_json_path::ExactlyOneError;
use std::{env::VarError, io, path::PathBuf, string::FromUtf8Error};
use thiserror::Error;

pub type TemplateResult = Result<String, TemplateError>;

/// Any error that can occur during template rendering. The purpose of having a
/// structured error here (while the rest of the app just uses `anyhow`) is to
/// support localized error display in the UI, e.g. showing just one portion of
/// a string in red if that particular template key failed to render.
///
/// The error always holds owned data so it can be detached from the lifetime
/// of the template context. This requires a mild amount of cloning in error
/// cases, but those should be infrequent so it's fine.
#[derive(Debug, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub enum TemplateError {
    /// Template key could not be parsed
    #[error("Failed to parse template key {key:?}")]
    InvalidKey { key: String },

    /// A basic field key contained an unknown field
    #[error("Unknown field {field:?}")]
    FieldUnknown { field: String },

    #[error("Error resolving chain {chain_id:?}")]
    Chain {
        chain_id: String,
        #[source]
        error: ChainError,
    },

    /// Variable either didn't exist or had non-unicode content
    #[error("Error accessing environment variable {variable:?}")]
    EnvironmentVariable {
        variable: String,
        #[source]
        error: VarError,
    },
}

/// An error sub-type, for any error that occurs while resolving a chained
/// value. This is factored out because they all need to be paired with a chain
/// ID.
#[derive(Debug, Error)]
pub enum ChainError {
    /// Reference to a chain that doesn't exist
    #[error("Unknown chain")]
    Unknown,
    /// An error occurred accessing the request repository. This error is
    /// generated by our code so we don't need any extra context.
    #[error(transparent)]
    Repository(anyhow::Error),
    /// The chain ID is valid, but the corresponding recipe has no successful
    /// response
    #[error("No response available")]
    NoResponse,
    /// Failed to parse the response body before applying a selector
    #[error("Error parsing response")]
    ParseResponse {
        #[source]
        error: anyhow::Error,
    },
    /// Got either 0 or 2+ results for JSON path query
    #[error("Expected exactly one result from selector")]
    InvalidResult {
        #[source]
        error: ExactlyOneError,
    },
    /// User gave an empty list for the command
    #[error("No command given")]
    CommandMissing,
    #[error("Error executing command {command:?}")]
    Command {
        command: Vec<String>,
        #[source]
        error: io::Error,
    },
    #[error("Error decoding output for {command:?}")]
    CommandInvalidUtf8 {
        command: Vec<String>,
        #[source]
        error: FromUtf8Error,
    },
    #[error("Error reading from file {path:?}")]
    File {
        path: PathBuf,
        #[source]
        error: io::Error,
    },
    /// Never got a response from the prompt channel. Do *not* store the
    /// `RecvError` here, because it provides useless extra output to the user.
    #[error("No response from prompt")]
    PromptNoResponse,
}

/// Placeholder implementation to allow equality checks for *other*
/// `TemplateError` variants. This one is hard to do because `anyhow::Error`
/// doesn't impl `PartialEq`
#[cfg(test)]
impl PartialEq for ChainError {
    fn eq(&self, _: &Self) -> bool {
        unimplemented!("PartialEq for ChainError is hard to implement")
    }
}