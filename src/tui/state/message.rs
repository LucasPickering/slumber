//! Async message passing! This is how inputs and other external events trigger
//! state updates.

use crate::{
    config::{RequestCollection, RequestRecipeId},
    http::RequestRecord,
};
use derive_more::{Display, From};
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedSender;
use tracing::trace;

/// Wrapper around a sender for async messages. Cheap to clone and pass around
#[derive(Clone, Debug, From)]
pub struct MessageSender(UnboundedSender<Message>);

impl MessageSender {
    /// Send an async message, to be handled by the main loop
    pub fn send(&self, message: Message) {
        trace!(%message, "Queueing message");
        self.0.send(message).expect("Message queue is closed")
    }
}

/// A message triggers some *asynchronous* action. Most state modifications can
/// be made synchronously by the input handler, but some require async handling
/// at the top level. The controller is responsible for both triggering and
/// handling messages.
#[derive(Debug, Display)]
pub enum Message {
    /// Trigger collection reload
    CollectionStartReload,
    /// Store a reloaded collection value in state
    #[display(fmt = "EndReloadCollection(collection_file:?)")]
    CollectionEndReload {
        collection_file: PathBuf,
        collection: RequestCollection,
    },

    /// Launch an HTTP request from the currently selected recipe. Errors if
    /// the recipe list is empty.
    HttpSendRequest,
    /// We received an HTTP response
    #[display(
        fmt = "HttpResponse(id={}, status={})",
        "record.id()",
        "record.response.status"
    )]
    HttpResponse { record: RequestRecord },
    #[display(fmt = "HttpError(recipe={}, error={})", recipe_id, error)]
    HttpError {
        recipe_id: RequestRecipeId,
        error: anyhow::Error,
    },

    /// Load the most recent response for a recipe from the repository
    RepositoryStartLoad { recipe_id: RequestRecipeId },
    /// Finished loading a response from the repository
    #[display(fmt = "RepositoryEndLoad(id={})", "record.id()")]
    RepositoryEndLoad { record: RequestRecord },

    /// An error occurred in some async process and should be shown to the user
    Error { error: anyhow::Error },
}
