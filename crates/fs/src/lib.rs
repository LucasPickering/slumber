//! TODO

use crate::filesystem::{CollectionFilesystem, Context};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use slumber_core::collection::RecipeId;
use slumber_util::{ResultTracedAnyhow, paths};
use std::{error::Error, fs, path::PathBuf};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    select,
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
};
use tracing::{debug, error, info};

mod filesystem;
mod node;

type MessagesTx = UnboundedSender<Message>;
type MessagesRx = UnboundedReceiver<Message>;

/// TODO
pub async fn run(
    collection_path: Option<PathBuf>,
    mount_path: PathBuf,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::unbounded_channel::<Message>();
    let filesystem = CollectionFilesystem::new(collection_path, mount_path)?;

    select! {
        result = handle_messages(rx) => result,
        result = listen_todo(tx) => result,
        result = filesystem.spawn() => result,
    }
}

/// TODO
pub async fn send_message(message: Message) -> anyhow::Result<()> {
    let socket_path = socket_path();
    let mut stream =
        UnixStream::connect(&socket_path).await.with_context(|| {
            format!("Error connecting to socket {}", socket_path.display())
        })?;
    let data = serde_json::to_vec(&message).expect("TODO");
    stream.write_all(&data).await.context("TODO")
}

/// TODO
#[derive(Debug, Serialize, Deserialize)]
pub enum Message {
    /// Trigger an HTTP request
    SendRequest { recipe_id: RecipeId },
}

/// TODO
async fn listen_todo(messages_tx: MessagesTx) -> anyhow::Result<()> {
    let socket_path = paths::data_directory().join("slumber.sock");
    // Delete the file if it's already in place
    // TODO what happens if another instance of the fs server is running? we
    // should detect and exit. THERE CAN ONLY BE ONE
    let _ = fs::remove_file(&socket_path).context("TODO").traced();
    let listener = UnixListener::bind(&socket_path).with_context(|| {
        format!("Error binding to socket {}", socket_path.display())
    })?;
    info!(?socket_path, "Socket: listening for clients");
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                info!(?addr, "Socket: client connected");
                listen_stream(stream, messages_tx.clone()).await;
            }
            Err(_) => {} // TODO log error
        }
    }
}

/// TODO
async fn listen_stream(mut socket: UnixStream, messages_tx: MessagesTx) {
    let mut buf = [0; 1024]; // TODO shrink this probably
    loop {
        match socket.read(&mut buf).await {
            Ok(0) => {
                info!("Client disconnected");
                return;
            }
            // Messages are small enough that they're always sent in a single
            // packet
            Ok(n) => {
                let data = &buf[0..n];
                match serde_json::from_slice::<Message>(data) {
                    Ok(message) => {
                        info!(?message, "Received message");
                        messages_tx.send(message).expect("TODO");
                    }
                    Err(error) => {
                        error!(
                            error = &error as &dyn Error,
                            ?data,
                            "Invalid message"
                        );
                    }
                }
            }
            Err(error) => {
                error!(
                    error = &error as &dyn Error,
                    "Error reading message from client"
                );
            }
        }
    }
}

async fn handle_messages(mut rx: MessagesRx) -> anyhow::Result<()> {
    loop {
        let Some(message) = rx.recv().await else {
            return Ok(());
        };
        debug!(?message, "Received message");
        // TODO use message
    }
}

/// TODO
fn socket_path() -> PathBuf {
    paths::data_directory().join("slumber.sock")
}
