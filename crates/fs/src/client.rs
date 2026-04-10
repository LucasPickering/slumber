//! Filesystem client operations
//!
//! Clients exist as short-lived CLI processes. A user executes a command like
//! `slumber fs mount ...`, which calls a function in this module. This
//! communicates with the fs server via a Unix Domain Socket. Once the operation
//! is complete, the client exits and the server lives on.

use crate::message::{
    ClientStream, RequestClientMessage, RequestServerMessage, StateRequest,
};
use slumber_console::ConsolePrompter;
use slumber_core::{
    collection::RecipeId, database::CollectionId, render::Prompter,
};
use slumber_util::ResultTracedAnyhow;

/// Client command to send an HTTP request
///
/// Open a connection with the filesystem server to initiate a request, then
/// listen for state updates.
pub async fn send_request(
    collection_id: CollectionId,
    recipe_id: RecipeId,
) -> anyhow::Result<()> {
    async fn handle_message(
        client: &mut ClientStream<StateRequest>,
        result: anyhow::Result<RequestServerMessage>,
    ) -> anyhow::Result<()> {
        let message = result?;
        match message {
            RequestServerMessage::Building { .. } => {
                eprintln!("Building...");
                Ok(())
            }
            RequestServerMessage::PromptText { id, prompt } => {
                let reply = ConsolePrompter
                    .prompt_text(
                        prompt.message,
                        prompt.default,
                        prompt.sensitive,
                    )
                    .await;
                client
                    .send(RequestClientMessage::PromptTextReply { id, reply })
                    .await
            }
            RequestServerMessage::PromptSelect { id, prompt } => {
                let reply = ConsolePrompter
                    .prompt_select(prompt.message, prompt.options)
                    .await;
                client
                    .send(RequestClientMessage::PromptSelectReply { id, reply })
                    .await
            }
            RequestServerMessage::BuildError { message, .. } => {
                eprintln!("{message}");
                Ok(())
            }
            RequestServerMessage::Loading { .. } => {
                eprintln!("Loading...");
                Ok(())
            }
            RequestServerMessage::Response(summary) => {
                eprintln!("{}", summary.status);
                Ok(())
            }
            RequestServerMessage::RequestError { message, .. } => {
                eprintln!("{message}");
                Ok(())
            }
        }
    }

    let mut client = ClientStream::connect()
        .await?
        .send_request(collection_id, recipe_id)
        .await?;
    while let Some(result) = client.listen().await {
        // Errors aren't fatal
        let _ = handle_message(&mut client, result).await.traced();
    }
    Ok(())
}
