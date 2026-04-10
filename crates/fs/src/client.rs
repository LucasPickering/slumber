//! Filesystem client operations
//!
//! Clients exist as short-lived CLI processes. A user executes a command like
//! `slumber fs mount ...`, which calls a function in this module. This
//! communicates with the fs server via a Unix Domain Socket. Once the operation
//! is complete, the client exits and the server lives on.

use crate::{
    http::{PromptSelect, PromptText},
    message::{
        ClientStream, RequestClientMessage, RequestServerMessage, StateRequest,
    },
};
use anyhow::Context as _;
use dialoguer::{Input, Password, Select};
use slumber_core::{collection::RecipeId, database::CollectionId};
use slumber_template::Value;
use slumber_util::ResultTracedAnyhow;
use tokio::task;
use tracing::warn;

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
                let reply = prompt_text(prompt).await;
                client
                    .send(RequestClientMessage::PromptTextReply { id, reply })
                    .await
            }
            RequestServerMessage::PromptSelect { id, prompt } => {
                let reply = prompt_select(prompt).await;
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

/// Prompt the user for some text input
async fn prompt_text(prompt: PromptText) -> Option<String> {
    // dialoguer is blocking so do it in a background thread
    task::spawn_blocking(move || {
        if prompt.sensitive {
            // Dialoguer doesn't support default values here so there's nothing
            // we can do
            if prompt.default.is_some() {
                warn!(
                    "Default value not supported for sensitive prompts in CLI"
                );
            }

            Password::new()
                .with_prompt(prompt.message)
                .allow_empty_password(true)
                .interact()
        } else {
            let mut input =
                Input::new().with_prompt(prompt.message).allow_empty(true);
            if let Some(default) = prompt.default {
                input = input.default(default);
            }
            input.interact()
        }
        .ok()
    })
    .await
    .context("Prompt thread panicked")
    .traced()
    .ok()
    .flatten()
}

/// Prompt the user to select an item from a list
async fn prompt_select(mut prompt: PromptSelect) -> Option<Value> {
    task::spawn_blocking(move || {
        let index = Select::new()
            .with_prompt(prompt.message)
            .items(&prompt.options)
            .default(0)
            .interact()
            .ok()?;
        Some(prompt.options.swap_remove(index).value)
    })
    .await
    .context("Prompt thread panicked")
    .traced()
    .ok()
    .flatten()
}
