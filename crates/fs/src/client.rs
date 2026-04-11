//! Filesystem client operations
//!
//! Clients exist as short-lived CLI processes. A user executes a command like
//! `slumber fs mount ...`, which calls a function in this module. This
//! communicates with the fs server via a Unix Domain Socket. Once the operation
//! is complete, the client exits and the server lives on.

use crate::rpc::{RequestClientMessage, RequestServerMessage, RpcClient};
use futures::{SinkExt, StreamExt};
use slumber_console::ConsolePrompter;
use slumber_core::{
    collection::{CollectionFile, RecipeId},
    database::CollectionId,
    render::Prompter,
};
use slumber_util::ResultTracedAnyhow;
use std::path::PathBuf;

/// TODO
#[derive(Clone, Debug, clap::Subcommand)]
pub enum FilesystemCommand {
    /// Mount a collection as a virtual filesystem
    ///
    /// This will select the collection to mount based on the current directory,
    /// the same as other CLI commands and the TUI. To pass a specific path to
    /// a collection file to mount:
    ///
    ///   slumber --file <collection_path> fs mount <mount_path>
    Mount {
        /// Path to mount the virtual filesystem to
        mount_path: PathBuf,
    },
    /// Send an HTTP request
    ///
    /// TODO is this meant to be called directly? if so, make collection ID
    /// optional. if not, hide it
    Request {
        collection_id: CollectionId,
        recipe_id: RecipeId,
    },
    /// Unmount the virtual filesystem for a collection
    ///
    /// This will select the collection to unmount based on the current
    /// directory, the same as other CLI commands and the TUI. To pass a
    /// specific path to a collection file to mount:
    ///
    ///   slumber --file <collection_path> fs unmount
    #[clap(visible_aliases = ["umount"])]
    Unmount,
}

impl FilesystemCommand {
    /// Execute the subcommand
    pub async fn run(self) -> anyhow::Result<()> {
        match self {
            // TODO receive collection override path somehow
            FilesystemCommand::Mount { mount_path } => {
                mount(None, mount_path).await
            }
            FilesystemCommand::Request {
                collection_id,
                recipe_id,
            } => send_request(collection_id, recipe_id).await,
            FilesystemCommand::Unmount => unmount(None).await,
        }
    }
}

/// Mount a new collection
async fn mount(
    collection_path: Option<PathBuf>,
    mount_path: PathBuf,
) -> anyhow::Result<()> {
    let mut client = RpcClient::connect().await?;
    let file = CollectionFile::new(collection_path)?;
    let paths = client.mount(file, mount_path.clone()).await?;
    println!(
        "Mounted {collection_path} at {mount_path}",
        collection_path = paths.collection_path.display(),
        mount_path = paths.mount_path.display()
    );
    Ok(())
}

/// Unmount a mounted collection
async fn unmount(collection_path: Option<PathBuf>) -> anyhow::Result<()> {
    let mut client = RpcClient::connect().await?;
    let file = CollectionFile::new(collection_path)?;
    let paths = client.unmount(file).await?;
    println!(
        "Mounted {collection_path} at {mount_path}",
        collection_path = paths.collection_path.display(),
        mount_path = paths.mount_path.display()
    );
    Ok(())
}

/// Client command to send an HTTP request
///
/// Open a connection with the filesystem server to initiate a request, then
/// listen for state updates.
async fn send_request(
    collection_id: CollectionId,
    recipe_id: RecipeId,
) -> anyhow::Result<()> {
    async fn handle_message(
        message: RequestServerMessage,
    ) -> Option<RequestClientMessage> {
        match message {
            RequestServerMessage::Building { .. } => {
                eprintln!("Building...");
                None
            }
            RequestServerMessage::PromptText { id, prompt } => {
                let reply = ConsolePrompter
                    .prompt_text(
                        prompt.message,
                        prompt.default,
                        prompt.sensitive,
                    )
                    .await;
                Some(RequestClientMessage::PromptTextReply { id, reply })
            }
            RequestServerMessage::PromptSelect { id, prompt } => {
                let reply = ConsolePrompter
                    .prompt_select(prompt.message, prompt.options)
                    .await;
                Some(RequestClientMessage::PromptSelectReply { id, reply })
            }
            RequestServerMessage::BuildError { message, .. } => {
                eprintln!("{message}");
                None
            }
            RequestServerMessage::Loading { .. } => {
                eprintln!("Loading...");
                None
            }
            RequestServerMessage::Response(summary) => {
                eprintln!("{}", summary.status);
                None
            }
            RequestServerMessage::RequestError { message, .. } => {
                eprintln!("{message}");
                None
            }
        }
    }

    let mut client = RpcClient::connect().await?;
    let (mut rx, mut tx) =
        client.send_request(collection_id, recipe_id).await?;
    while let Some(message) = rx.next().await {
        if let Some(reply) = handle_message(message).await {
            // Errors aren't fatal, just log em
            let _ = tx.send(reply).await.traced();
        }
    }
    Ok(())
}
