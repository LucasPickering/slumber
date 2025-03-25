use crate::{
    collection::{ProfileId, RecipeId},
    http::{RequestBuildError, RequestError},
};
use std::{string::FromUtf8Error, sync::Arc};
use thiserror::Error;

/// Error for [OverrideKey](crate::template::OverrideKey)'s `FromStr` impl.
#[derive(Debug, Error)]
#[error("Invalid override key")]
pub struct OverrideKeyParseError;

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
    #[error("Infinite loop detected in template")]
    InfiniteLoop,

    /// Something bad happened while triggering a request dependency
    #[error("Triggering upstream recipe `{recipe_id}`")]
    Trigger {
        recipe_id: RecipeId,
        #[source]
        error: TriggeredRequestError,
    },
}

impl TemplateError {
    /// Does the given error have *any* error in its chain that contains
    /// [TriggeredRequestError::NotAllowed]? This makes it easy to attach
    /// additional error context.
    pub fn has_trigger_disabled_error(error: &anyhow::Error) -> bool {
        error.chain().any(|error| {
            matches!(
                error.downcast_ref(),
                Some(Self::Trigger {
                    error: TriggeredRequestError::NotAllowed,
                    ..
                })
            )
        })
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
