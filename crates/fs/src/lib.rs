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
use chrono::Utc;
use clap::Parser;
use futures::{Stream, stream};
use reqwest::StatusCode;
use slumber_core::{
    collection::{CollectionFile, RecipeId},
    database::{CollectionId, Database},
    http::{ExchangeSummary, RequestId},
};
use slumber_util::ResultTracedAnyhow;
use std::{collections::HashMap, path::PathBuf};
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
    ///
    /// Only one instance of the server can be running at a time.
    Run,
    /// Send an HTTP request
    Request {
        collection_id: CollectionId,
        recipe_id: RecipeId,
    },
}

/// TODO
pub async fn run(args: Args) -> anyhow::Result<()> {
    match args.subcommand {
        FilesystemCommand::Run => FilesystemServer::new()?.run().await,
        FilesystemCommand::Request {
            collection_id,
            recipe_id,
        } => send_request(collection_id, recipe_id).await,
    }
}

/// A single-instance server to mount collections as filesystems
///
/// The filesystem module uses a client-server architecture. This struct is the
/// server instance, and short-lived CLI processes are the clients. They
/// communicate over a Unix Domain Socket (UDS).
///
/// There is only instance of this server per process, and there should only be
/// one server process running on a machine at a time. This is enforced via
/// ownership of the UDS ([ServerListener]). Generally this will be run as a
/// system service via systemd or similar.
///
/// This server performs two main tasks:
/// - Expose collection and request data as one or more FUSE filesystems
/// - Listen for client messages via a global UDS. These messages trigger
///   actions such as mounting/unmounting filesystems or sending requests.
struct FilesystemServer {
    /// SQLite DB for all collections
    database: Database,
    /// A map of all collections actively mounted
    collections: HashMap<CollectionId, CollectionFilesystem>,
}

impl FilesystemServer {
    fn new() -> anyhow::Result<Self> {
        let database = Database::load()?;
        Ok(Self {
            database,
            collections: HashMap::new(),
        })
    }

    /// Spawn the filesystem server
    async fn run(mut self) -> anyhow::Result<()> {
        // Open a UDS socket
        let socket = ServerListener::bind()?;

        // In dev, mount the default collection
        // TODO do this differently like
        if cfg!(debug_assertions) {
            let file = CollectionFile::new(None)?;
            self.mount(file, "myfs".into())?;
        }

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
        self.unmount_all();

        result
    }

    /// Mount a filesystem for a collection
    fn mount(
        &mut self,
        collection_file: CollectionFile,
        mount_path: PathBuf,
    ) -> anyhow::Result<()> {
        // Get a scoped DB handle just for this collection
        let database =
            self.database.clone().into_collection(&collection_file)?;
        let collection_id = database.collection_id();
        let filesystem =
            CollectionFilesystem::mount(collection_file, database, mount_path)?;
        self.collections.insert(collection_id, filesystem);

        Ok(())
    }

    /// Unmount all filesystems, waiting for each one to unmount
    ///
    /// If any unmount fails, log it and move on.
    fn unmount_all(self) {
        for fs in self.collections.into_values() {
            let _ = fs.unmount().traced();
        }
    }
}

/// Client command to send an HTTP request
///
/// Open a connection with the filesystem server to initiate a request, then
/// listen for state updates.
async fn send_request(
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
        collection_id: CollectionId,
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
