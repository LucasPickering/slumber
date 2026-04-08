//! Filesystem client operations
//!
//! Clients exist as short-lived CLI processes. A user executes a command like
//! `slumber fs mount ...`, which calls a function in this module. This
//! communicates with the fs server via a Unix Domain Socket. Once the operation
//! is complete, the client exits and the server lives on.

use crate::{http::RequestState, message::ClientStream};
use slumber_core::{collection::RecipeId, database::CollectionId};

/// Client command to send an HTTP request
///
/// Open a connection with the filesystem server to initiate a request, then
/// listen for state updates.
pub async fn send_request(
    collection_id: CollectionId,
    recipe_id: RecipeId,
) -> anyhow::Result<()> {
    let mut client = ClientStream::connect()
        .await?
        .send_request(collection_id, recipe_id)
        .await?;
    loop {
        let Some(message) = client.listen().await? else {
            break Ok(());
        };
        match message {
            RequestState::Building { .. } => {
                eprintln!("Building...");
            }
            RequestState::BuildError { message, .. } => {
                eprintln!("{message}");
            }
            RequestState::Loading { .. } => {
                eprintln!("Loading...");
            }
            RequestState::Response(summary) => {
                eprintln!("{}", summary.status);
            }
            RequestState::RequestError { message, .. } => {
                eprintln!("{message}");
            }
        }
    }
}
