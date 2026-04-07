//! TODO

mod filesystem;
mod message;
mod node;
mod util;

use crate::{
    filesystem::{CollectionFilesystem, Context},
    message::{
        ClientStream, MessageHandler, RequestStateSummary, ServerListener,
    },
};
use anyhow::Context as _;
use chrono::Utc;
use clap::Parser;
use futures::{Stream, stream};
use reqwest::StatusCode;
use slumber_core::{
    collection::RecipeId,
    http::{ExchangeSummary, RequestId},
};
use std::path::PathBuf;
use tokio::{select, task};
use tracing::{debug, info, level_filters::LevelFilter};

/// TODO
#[derive(Debug, Parser)]
pub struct Args {
    /// TODO
    #[clap(long, default_value_t = LevelFilter::OFF)]
    pub log_level: LevelFilter,
    #[command(subcommand)]
    pub subcommand: FilesystemCommand,
}

/// TODO
#[derive(Clone, Debug, clap::Subcommand)]
pub enum FilesystemCommand {
    /// Run the filesystem server
    Run {
        #[clap(long = "file", short = 'f')]
        collection_path: Option<PathBuf>,
        #[clap(long = "mount")]
        mount_path: PathBuf,
    },
    /// Send an HTTP request
    Request { recipe_id: RecipeId },
}

/// TODO
pub async fn run(args: Args) -> anyhow::Result<()> {
    match args.subcommand {
        FilesystemCommand::Run {
            collection_path,
            mount_path,
        } => run_server(collection_path, mount_path).await,
        FilesystemCommand::Request { recipe_id } => {
            send_request(recipe_id).await
        }
    }
}

/// Run the filesystem server
async fn run_server(
    collection_path: Option<PathBuf>,
    mount_path: PathBuf,
) -> anyhow::Result<()> {
    let filesystem = CollectionFilesystem::new(collection_path, mount_path)?;
    // Spawn the filesystem in a background thread. Once the handle is
    // dropped, it will be unmounted.
    let fs_handle = filesystem.spawn()?;

    // Open a UDS socket
    let socket = ServerListener::bind()?;

    // Run in a local set so all tasks can be spawned on the main
    // thread. This server does very little CPU work (I think) so it
    // should all be able to run on one thread. The FUSE server runs on
    // a background thread, so there's not much for the main thread to
    // do.
    let local = task::LocalSet::new();
    let result = local
        .run_until(async move {
            select! {
                // These futures all run indefinitely. If any terminates, exit
                // the process.
                // TODO use an actor setup for these? Should be non-lethal
                () = socket.listen(TodoHandler {}) => Ok(()),
                result = util::signals() => result, // Listen for exit signal
            }
        })
        .await;

    // Unmount the file system
    info!("Exiting...");
    fs_handle
        .umount_and_join()
        .context("Error unmounting filesystem")?;

    result
}

/// Client command to send an HTTP request
///
/// Open a connection with the filesystem server to initiate a request, then
/// listen for state updates.
async fn send_request(recipe_id: RecipeId) -> anyhow::Result<()> {
    let mut client = ClientStream::connect()
        .await?
        .send_request(recipe_id)
        .await?;
    loop {
        let Some(message) = client.listen().await? else {
            break Ok(());
        };
        match message {
            RequestStateSummary::Building { .. } => {
                println!("Building...");
            }
            RequestStateSummary::BuildCancelled { .. } => println!("Cancelled"),
            // TODO show errors
            RequestStateSummary::BuildError { .. } => println!("Build error"),
            RequestStateSummary::Loading { .. } => {
                println!("Loading...");
            }
            RequestStateSummary::LoadingCancelled { .. } => {
                println!("Cancelled");
            }
            RequestStateSummary::Response(_) => println!("Done"),
            RequestStateSummary::RequestError { .. } => {
                println!("Request error");
            }
        }
    }
}

/// Receiver for all messages from clients
#[derive(Clone, Debug)]
struct TodoHandler {}

impl MessageHandler for TodoHandler {
    fn send_request(
        self,
        recipe_id: RecipeId,
    ) -> impl Stream<Item = RequestStateSummary> {
        // Fake a response for now
        debug!("Faking request for {recipe_id}");
        let id = RequestId::new();
        let now = Utc::now();
        stream::iter([
            RequestStateSummary::Building {
                id,
                start_time: now,
            },
            RequestStateSummary::Loading {
                id,
                start_time: now,
            },
            RequestStateSummary::Response(ExchangeSummary {
                id,
                recipe_id,
                profile_id: None,
                start_time: now,
                end_time: now,
                status: StatusCode::OK,
            }),
        ])
    }
}
