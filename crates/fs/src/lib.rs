//! TODO

mod client;
mod filesystem;
mod message;
mod util;

use crate::{
    filesystem::CollectionFilesystem,
    message::{RequestStateSummary, ServerListener},
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
use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};
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
        } => client::send_request(collection_id, recipe_id).await,
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
///
/// Each FUSE system runs on its own background thread. The UDS operations all
/// run on background tasks within the main tokio thread (via a [LocalSet]).
/// To share data across tasks on the main thread, this struct uses interior
/// mutability. This type implements `Clone` so it can be shared between those
/// tasks within the main thread.
#[derive(Clone, Debug)]
struct FilesystemServer {
    /// SQLite DB for all collections
    database: Database,
    /// A map of all collections actively mounted
    ///
    /// TODO explain interior mutability
    collections: Rc<RefCell<HashMap<CollectionId, CollectionFilesystem>>>,
}

impl FilesystemServer {
    fn new() -> anyhow::Result<Self> {
        let database = Database::load()?;
        Ok(Self {
            database,
            collections: Default::default(),
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
        // should all be able to run on one thread. The FUSE servers run on
        // background threads, so there's not much for the main thread to
        // do.
        let local = task::LocalSet::new();
        let handler = self.clone();
        let result = local
            .run_until(async move {
                select! {
                    // These futures all run indefinitely. If any terminates, exit
                    // the process.
                    // TODO use an actor setup for these? Should be non-lethal
                    () = socket.listen(handler) => Ok(()),
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
        self.collections
            .borrow_mut()
            .insert(collection_id, filesystem);

        Ok(())
    }

    /// Unmount all filesystems, waiting for each one to unmount
    ///
    /// If any unmount fails, log it and move on.
    fn unmount_all(self) {
        for (_, fs) in self.collections.borrow_mut().drain() {
            let _ = fs.unmount().traced();
        }
    }

    fn send_request(
        self,
        collection_id: CollectionId,
        recipe_id: RecipeId,
    ) -> impl Stream<Item = RequestStateSummary> {
        // Fake a response for now
        debug!("Faking request for {collection_id}/{recipe_id}");
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
