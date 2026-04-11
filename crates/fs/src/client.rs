//! Filesystem client operations
//!
//! Clients exist as short-lived CLI processes. A user executes a command like
//! `slumber fs mount ...`, which calls a function in this module. This
//! communicates with the fs server via a Unix Domain Socket. Once the operation
//! is complete, the client exits and the server lives on.

use crate::message::{
    ClientStream, MountServerMessage, RequestClientMessage,
    RequestServerMessage, StateRequest,
};
use slumber_console::ConsolePrompter;
use slumber_core::{
    collection::{CollectionFile, RecipeId},
    database::{CollectionId, Database},
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
            FilesystemCommand::Unmount => todo!(),
        }
    }
}

/// Mount a new collection
async fn mount(
    collection_path: Option<PathBuf>,
    mount_path: PathBuf,
) -> anyhow::Result<()> {
    let mut client = ClientStream::connect()
        .await?
        .mount(collection_path, mount_path.clone())
        .await?;

    // We only expect one message back, but use a loop anyway cause it's fun
    while let Some(result) = client.listen().await {
        match result? {
            MountServerMessage::Ok {
                collection_path,
                mount_path,
            } => println!(
                "Mounted {} at {}",
                collection_path.display(),
                mount_path.display()
            ),
            MountServerMessage::AlreadyMounted {
                collection_path,
                already_at,
            } => eprintln!(
                "Collection `{collection_path}` already mounted at \
                `{already_at}`. Each collection can only be mounted a single \
                time. To unmount the old location first:

  slumber fs unmount {already_at}

If you want it accessible at both locations, try a symlink:

  ln -s {already_at} {requested_at}
                ",
                collection_path = collection_path.display(),
                already_at = already_at.display(),
                requested_at = mount_path.display(),
            ),
            MountServerMessage::Error { message } => eprintln!("{message}"),
        }
    }
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
