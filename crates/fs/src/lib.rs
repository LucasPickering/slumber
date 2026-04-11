//! TODO

mod client;
mod filesystem;
mod http;
mod rpc;
mod util;

use crate::{
    client::FilesystemCommand,
    filesystem::CollectionFilesystem,
    http::{FilesystemHttpProvider, FilesystemPrompter},
    rpc::{
        RequestClientMessage, RequestServerMessage, RpcSink, RpcStream,
        ServerListener,
    },
};
use chrono::Utc;
use clap::Parser;
use futures::{SinkExt, future};
use slumber_config::Config;
use slumber_core::{
    collection::{CollectionFile, HasId, Profile, RecipeId},
    database::{CollectionId, Database},
    http::{BuildOptions, HttpEngine, RequestSeed},
    render::TemplateContext,
};
use slumber_util::ResultTracedAnyhow;
use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::Entry},
    fmt::{self, Display},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};
use thiserror::Error;
use tokio::{select, task};
use tracing::{debug, info, level_filters::LevelFilter};

/// TODO
#[derive(Debug, Parser)]
pub struct Args {
    /// TODO
    #[clap(long, default_value_t = LevelFilter::OFF)]
    pub log_level: LevelFilter,
    #[command(subcommand)]
    pub subcommand: Option<FilesystemCommand>,
}

/// TODO
pub async fn run(args: Args) -> anyhow::Result<()> {
    match args.subcommand {
        None => FilesystemServer::new()?.run().await,
        Some(subcommand) => subcommand.run().await,
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
    /// Make HTTP go brrr
    http_engine: HttpEngine,
    /// A map of all collections actively mounted
    ///
    /// TODO explain interior mutability
    collections: Rc<RefCell<HashMap<CollectionId, CollectionFilesystem>>>,
}

impl FilesystemServer {
    fn new() -> anyhow::Result<Self> {
        let config = Config::load()?;
        let database = Database::load()?;
        let http_engine = HttpEngine::new(&config.http);
        Ok(Self {
            database,
            http_engine,
            collections: Default::default(),
        })
    }

    /// Spawn the filesystem server
    async fn run(self) -> anyhow::Result<()> {
        // Open a UDS socket
        let socket = ServerListener::bind()?;

        // In dev, mount the default collection
        // TODO do this differently like
        if cfg!(debug_assertions) {
            self.mount(CollectionFile::new(None)?, "myfs".into())?;
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
        &self,
        collection_file: CollectionFile,
        mount_path: PathBuf,
    ) -> Result<(), anyhow::Error> {
        let database =
            self.database.clone().into_collection(&collection_file)?;
        let collection_id = database.collection_id();
        let mut collections = self.collections.borrow_mut();

        // Make sure it's not already mounted first, or we would lose the old
        // handle
        match collections.entry(collection_id) {
            Entry::Occupied(entry) => Err(AlreadyMounted {
                collection_path: collection_file.path().to_owned(),
                already_at: entry.get().mount_path().to_owned(),
                requested_at: mount_path,
            }
            .into()),
            Entry::Vacant(entry) => {
                let filesystem = CollectionFilesystem::mount(
                    collection_file,
                    database,
                    mount_path,
                )?;
                entry.insert(filesystem);
                Ok(())
            }
        }
    }

    /// TODO
    fn unmount(
        &self,
        collection_file: CollectionFile,
    ) -> anyhow::Result<PathBuf> {
        let collection_id =
            self.database.get_collection_id(collection_file.path())?;
        let mut collections = self.collections.borrow_mut();

        match collections.entry(collection_id) {
            Entry::Occupied(entry) => {
                let filesystem = entry.remove();
                let mount_path = filesystem.mount_path().to_owned();
                // TODO this blocks - bad!!
                filesystem.unmount()?;
                Ok(mount_path)
            }
            Entry::Vacant(_) => Err(NotMounted {
                collection_path: collection_file.path().to_owned(),
            }
            .into()),
        }
    }

    /// Unmount all filesystems, waiting for each one to unmount
    ///
    /// If any unmount fails, log it and move on.
    fn unmount_all(self) {
        for (_, fs) in self.collections.borrow_mut().drain() {
            let _ = fs.unmount().traced();
        }
    }

    /// Send an HTTP request
    ///
    /// This will emit state updates to the given `reply` function as the
    /// request progresses.
    async fn send_request(
        self,
        collection_id: CollectionId,
        recipe_id: RecipeId,
        mut socket_read: RpcStream<'_, RequestClientMessage>,
        mut socket_write: RpcSink<'_, RequestServerMessage>,
    ) {
        debug!("Sending request for {collection_id}/{recipe_id}");

        // TODO explain promptery
        let (prompter, prompt_mux) = http::prompter();
        let (context, database) = {
            // Ensure the refcell is dropped immediately when we're done with it
            let collections = self.collections.borrow();
            let collection_fs = collections.get(&collection_id).expect("TODO");
            (
                self.template_context(collection_fs, prompter),
                collection_fs.database().clone(),
            )
        };
        let http_engine = self.http_engine.clone();

        let seed = RequestSeed::new(recipe_id, BuildOptions::default());
        let _ = socket_write
            .send(RequestServerMessage::Building {
                start_time: Utc::now(),
            })
            .await;

        // Run the build. Prompts require sending messages back to the client
        // over the socket to get answers. That is handled by the multiplexer,
        // which has to run concurrently.
        let result = select! {
            result = http_engine.build(seed, &context) => result,
            // Generally the multiplexer will run until the build is complete
            // (when the context is dropped), but if it exits early we *don't*
            // want to kill the build.
            //
            // This is structually similar to a background task, but this
            // future isn't 'static so we can't spawn it in another task.
            () = async {
                prompt_mux.multiplex(&mut socket_read, &mut socket_write).await;
                // Await forever so we don't kill the build
                future::pending::<()>().await;
            } => unreachable!(),
        };
        let ticket = match result {
            Ok(ticket) => ticket,
            Err(error) => {
                let _ = socket_write
                    .send(RequestServerMessage::BuildError {
                        start_time: error.start_time,
                        end_time: error.end_time,
                        // TODO include error chain
                        message: format!("{error:#}"),
                    })
                    .await;
                return;
            }
        };
        let _ = socket_write
            .send(RequestServerMessage::Loading {
                start_time: Utc::now(),
            })
            .await;

        // Send the request
        match ticket.send(Some(database)).await {
            Ok(exchange) => {
                let _ = socket_write
                    .send(RequestServerMessage::Response(exchange.summary()))
                    .await;
            }
            Err(error) => {
                let _ = socket_write
                    .send(RequestServerMessage::RequestError {
                        start_time: error.start_time,
                        end_time: error.end_time,
                        // TODO include error chain
                        message: format!("{error:#}"),
                    })
                    .await;
            }
        }
    }

    fn template_context(
        &self,
        collection_fs: &CollectionFilesystem,
        prompter: FilesystemPrompter,
    ) -> TemplateContext {
        let collection = collection_fs.collection();
        let database = collection_fs.database().clone();
        TemplateContext {
            collection: Arc::clone(collection),
            // TODO track selected profile somehow
            selected_profile: collection
                .default_profile()
                .map(Profile::id)
                .cloned(),
            http_provider: Box::new(FilesystemHttpProvider::new(
                database,
                self.http_engine.clone(),
            )),
            overrides: Default::default(),
            prompter: Box::new(prompter),
            show_sensitive: true,
            root_dir: collection_fs.collection_file().parent().to_owned(),
            state: Default::default(),
        }
    }
}

/// Error: attempted to mount a collection that is already mounted
#[derive(Debug, Error)]
struct AlreadyMounted {
    /// Path to the collection file
    collection_path: PathBuf,
    /// Path that the collection is already mounted at
    already_at: PathBuf,
    /// Path the user tried to mount it at
    requested_at: PathBuf,
}

impl Display for AlreadyMounted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO is this too verbose in logging?
        write!(
            f,
            "Collection `{collection_path}` already mounted at `{already_at}`. \
            Each collection can only be mounted a single time. To unmount the \
            old location first:

  slumber fs unmount {already_at}

If you want it accessible at both locations, try a symlink:

  ln -s {already_at} {requested_at}",
            collection_path = self.collection_path.display(),
            already_at = self.already_at.display(),
            requested_at = self.requested_at.display(),
        )
    }
}

/// Error: attempted to unmount a collection that isn't mounted
#[derive(Debug, Error)]
#[error("Collection {} is not mounted", collection_path.display())]
struct NotMounted {
    /// TODO
    collection_path: PathBuf,
}
