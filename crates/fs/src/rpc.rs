//! Remote Procedure Call framework to communicate between the server process
//! and the short-lived client processes.
//!
//! This uses a Unix Domain Socket to communicate between server and clients.

use crate::{
    FilesystemServer,
    http::{PromptId, PromptSelect, PromptText},
};
use anyhow::Context as _;
use chrono::{DateTime, Utc};
use futures::{Sink, SinkExt as _, Stream, StreamExt as _, sink, stream};
use serde::{Deserialize, Serialize};
use slumber_core::{
    collection::RecipeId, database::CollectionId, http::ExchangeSummary,
    util::MaybeStr,
};
use slumber_template::Value;
use slumber_util::{ResultTracedAnyhow, paths};
use std::{
    fmt::Debug,
    fs,
    path::PathBuf,
    pin::Pin,
    sync::atomic::{AtomicU32, Ordering},
    task::{self, Poll},
};
use thiserror::Error;
use tokio::net::{
    UnixListener, UnixStream,
    unix::{OwnedReadHalf, OwnedWriteHalf},
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tracing::{Instrument, info, info_span, instrument};

/// TODO
pub struct ServerListener {
    listener: UnixListener,
}

impl ServerListener {
    /// Bind to the UDS socket
    pub fn bind() -> anyhow::Result<Self> {
        let socket_path = RpcSocket::path();
        // Delete the file if it's already in place
        // TODO what happens if another instance of the fs server is running? we
        // should detect and exit. THERE CAN ONLY BE ONE
        let _ = fs::remove_file(&socket_path).context("TODO").traced();
        let listener = UnixListener::bind(&socket_path).with_context(|| {
            format!("Error binding to socket {}", socket_path.display())
        })?;
        Ok(Self { listener })
    }

    /// Listen for clients on the UDS socket
    ///
    /// For each client that connects, spawn a subtask to handle its
    /// communication. This method never returns.
    #[instrument(level = "info", name = "socket", skip_all)]
    pub async fn listen(self, handler: FilesystemServer) {
        /// Each client gets a unique ID for logging purposes
        static NEXT_CLIENT_ID: AtomicU32 = AtomicU32::new(0);

        info!("Listening for clients");
        loop {
            let result = self.listener.accept().await;
            let Ok((stream, _)) =
                result.context("Error connecting to client").traced()
            else {
                continue;
            };

            // Generate a unique ID for each client so they can be grouped in
            // tracing easily
            let id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
            let stream = RpcServer::new(stream, handler.clone());
            let future = stream
                .handle_messages()
                .instrument(info_span!("client", id));

            // Communicate in a subtask so we can handle multiple
            // clients simultaneously
            tokio::task::spawn_local(future);
        }
    }
}

/// A message that can be sent from client to server over the UDS socket
///
/// This is the initial message received from a client. As such, we have no
/// context about what the client intends to do. This message defines the
/// context and available messages for the rest of the conversation. Each
/// variant correponds to a method on [MessageHandler], which narrows the
/// conversation to only the available message types.
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Mount a new collection
    Mount {
        collection_path: Option<PathBuf>,
        mount_path: PathBuf,
    },
    /// Trigger an HTTP request
    SendRequest {
        collection_id: CollectionId,
        recipe_id: RecipeId,
    },
    /// Unmount a mounted collection
    Unmount {
        // TODO support mount path instead of collection ID
        /// TODO explain
        collection_id: CollectionId,
    },
}

/// The server end of a UDS connection
///
/// `State` is a type state parameter denoting the kind of conversation being
/// had. It starts as [StateInit], meaning no open conversation. The initial
/// message sent defines the types of messages that can be sent/received
/// subsequently.
pub struct RpcServer {
    socket: RpcSocket,
    handler: FilesystemServer,
}

impl RpcServer {
    fn new(stream: UnixStream, handler: FilesystemServer) -> Self {
        Self {
            socket: RpcSocket::new(stream),
            handler,
        }
    }

    /// Handle a conversation with the connected client
    ///
    /// This will read the initial message, initiate some action in the service,
    /// and continue the conversation as needed.
    async fn handle_messages(mut self) {
        // Read the initial message to determine the scope of the conversation
        let message = match self.socket.read().await {
            // None means the socket closed without sending anything. Error is
            // logged in the reader; nothing else we can do here.
            None | Some(Err(_)) => return,
            Some(Ok(message)) => message,
        };

        // Call the appropriate receiver based on the message type
        let handler = self.handler.clone();
        match message {
            ClientMessage::Mount {
                collection_path,
                mount_path,
            } => {
                let result = handler.mount(collection_path, mount_path.clone());
                let message = result
                    .map(|collection_path| Mounted {
                        collection_path,
                        mount_path,
                    })
                    .map_err(StringError::from);
                // If there's an error here, there's nothing we can do. Job's
                // already done.
                let _ = self.socket.write(message).await;
            }
            ClientMessage::SendRequest {
                collection_id,
                recipe_id,
            } => {
                let (stream, sink) = self.socket.split();
                handler
                    .send_request(collection_id, recipe_id, stream, sink)
                    .await;
            }
            ClientMessage::Unmount { .. } => {
                todo!()
            }
        }
    }
}

/// TODO
pub struct RpcClient {
    socket: RpcSocket,
}

impl RpcClient {
    /// Open a connection with the server
    pub async fn connect() -> anyhow::Result<Self> {
        let socket_path = RpcSocket::path();
        let stream =
            UnixStream::connect(&socket_path).await.with_context(|| {
                format!("Error connecting to socket {}", socket_path.display())
            })?;
        info!(?socket_path, "Connected to server");
        Ok(Self {
            socket: RpcSocket::new(stream),
        })
    }

    /// Tell the server to mount a new collection
    pub async fn mount(
        &mut self,
        collection_path: Option<PathBuf>,
        mount_path: PathBuf,
    ) -> anyhow::Result<Mounted> {
        self.socket
            .write(ClientMessage::Mount {
                collection_path,
                mount_path,
            })
            .await?;
        let mounted = self
            .socket
            .read::<MountServerMessage>()
            .await
            .ok_or(SocketClosed)???; // ???
        Ok(mounted)
    }

    /// Tell the server to send a request
    ///
    /// This begins a conversation where the server sends state updates about
    /// the request, and the client can respond to prompts as needed.
    ///
    /// Returns a `(stream, sink)` pair used for continuted communication with
    /// the server. The stream produces state update messages and the sink is
    /// used to provide replies where necessary (e.g. replying to prompts).
    pub async fn send_request(
        &mut self,
        collection_id: CollectionId,
        recipe_id: RecipeId,
    ) -> anyhow::Result<(
        RpcStream<'_, RequestServerMessage>,
        RpcSink<'_, RequestClientMessage>,
    )> {
        self.socket
            .write(ClientMessage::SendRequest {
                collection_id,
                recipe_id,
            })
            .await?;
        Ok(self.socket.split())
    }
}

/// Information about a mounted filesystem
#[derive(Debug, Serialize, Deserialize)]
pub struct Mounted {
    /// Full path to the mounted collection file
    pub collection_path: PathBuf,
    /// Path where the filesystem was mounted to
    pub mount_path: PathBuf,
}

/// TODO
type MountServerMessage = Result<Mounted, StringError>;

/// Server -> client message for [StateRequest]
///
/// Since [StateRequest] is bound to a single root request, these messages all
/// pertain to that request. Triggered requests don't send any state updates;
/// they're transparent to the client.
#[derive(Debug, Serialize, Deserialize)]
pub enum RequestServerMessage {
    /// Server starting building the request
    Building { start_time: DateTime<Utc> },
    /// Server needs the user to answer a question with a text reply
    PromptText { id: PromptId, prompt: PromptText },
    /// Server needs the user to pick an option from a list
    PromptSelect { id: PromptId, prompt: PromptSelect },
    /// Build failed before the request was launched
    BuildError {
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        message: String,
    },
    /// Request has been sent and we're awaiting response
    Loading { start_time: DateTime<Utc> },
    /// We got a valid HTTP response
    Response(ExchangeSummary),
    /// Request failed while in flight
    RequestError {
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        message: String,
    },
}

/// Client -> server message for [StateRequest]
#[derive(Debug, Serialize, Deserialize)]
pub enum RequestClientMessage {
    /// Reply to a text prompt sent from the server
    PromptTextReply {
        /// Unique ID for multiplexing prompts on the socket
        id: PromptId,
        /// User's reply, or `None` if they declined to answer
        reply: Option<String>,
    },
    /// Reply to a list selection sent from the server
    PromptSelectReply {
        /// Unique ID for multiplexing prompts on the socket
        id: PromptId,
        /// User's reply, or `None` if they declined to answer
        reply: Option<Value>,
    },
}

/// An error that can be serialized/deserialized to be sent over the socket
///
/// This provides easy interoperability with [anyhow::Error] and other error
/// types.
#[derive(Debug, Serialize, Deserialize)]
struct StringError(String);

impl From<anyhow::Error> for StringError {
    fn from(error: anyhow::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<StringError> for anyhow::Error {
    fn from(error: StringError) -> Self {
        anyhow::Error::msg(error.0)
    }
}

/// Wrapper for [UnixStream] to handle encoding/decoding of messages
struct RpcSocket {
    /// Stream to read from the socket
    read: RpcSocketRead,
    /// Sink to write to the socket
    write: RpcSocketWrite,
}

impl RpcSocket {
    /// Get the path to the UDS file
    ///
    /// Since there is only a single system-wide server, there is only one
    /// possible socket path.
    fn path() -> PathBuf {
        paths::data_directory().join("slumber.sock")
    }

    /// Wrap a stream for convenneniencene
    fn new(stream: UnixStream) -> Self {
        let codec = LengthDelimitedCodec::new();
        let (read, write) = stream.into_split();
        let read = RpcSocketRead::new(read, codec.clone());
        let write = RpcSocketWrite::new(write, codec);
        Self { read, write }
    }

    /// Read one message from the socket
    ///
    /// Return `None` if the stream is closed.
    async fn read<M>(&mut self) -> Option<anyhow::Result<M>>
    where
        M: for<'de> Deserialize<'de> + Debug,
    {
        Self::read_inner(&mut self.read).await
    }

    async fn read_inner<M>(
        read: &mut RpcSocketRead,
    ) -> Option<anyhow::Result<M>>
    where
        M: for<'de> Deserialize<'de> + Debug,
    {
        // Load the message
        let result = read.next().await?.context("Socket read error");
        // Parse the message
        let result = result
            .and_then(|frame| {
                serde_json::from_slice::<M>(&frame).with_context(|| {
                    format!("Invalid client message {}", MaybeStr(&frame))
                })
            })
            .traced();
        if let Ok(message) = &result {
            info!(?message, "Received message");
        }
        Some(result)
    }

    /// Send one message to the socket
    async fn write<M>(&mut self, message: M) -> anyhow::Result<()>
    where
        M: Serialize + Debug,
    {
        Self::write_inner(&mut self.write, message).await
    }

    async fn write_inner<M>(
        write: &mut RpcSocketWrite,
        message: M,
    ) -> anyhow::Result<()>
    where
        M: Serialize + Debug,
    {
        info!(?message, "Sending message");
        let data = serde_json::to_vec(&message)
            .with_context(|| format!("Error encoding message {message:?}"))
            .traced()?;
        write
            .send(data.into())
            .await
            .with_context(|| format!("Error sending message {message:?}"))
            .traced()?;
        Ok(())
    }

    /// Split the socket into a read stream and a write sink
    ///
    /// Use this for external communication directly over the socket. Reads and
    /// writes are restricted to the specified static message types `RM` and
    /// `WM`, respectively.
    fn split<RM, WM>(&mut self) -> (RpcStream<'_, RM>, RpcSink<'_, WM>)
    where
        RM: for<'de> Deserialize<'de> + Debug,
        WM: 'static + Serialize + Debug,
    {
        let stream = stream::unfold(&mut self.read, async |read| {
            match Self::read_inner::<RM>(read).await {
                // Errors get logged and dropped. There's nothing the consumer
                // can do about a lost message, so there's no
                // point in conveying it
                None | Some(Err(_)) => None,
                Some(Ok(message)) => Some((message, read)),
            }
        });
        let sink = sink::unfold(&mut self.write, async |write, message| {
            Self::write_inner(write, message).await.map(|()| write)
        });
        (RpcStream(Box::pin(stream)), RpcSink(Box::pin(sink)))
    }
}

type RpcSocketRead = FramedRead<OwnedReadHalf, LengthDelimitedCodec>;
type RpcSocketWrite = FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>;

/// A stream that produces parsed messages from the RPC socket
///
/// Any message that fails to be loaded will be logged and tossed. Consumers of
/// this stream don't have to handle any errors themselves.
pub struct RpcStream<'a, M>(Pin<Box<dyn 'a + Stream<Item = M>>>);

impl<M> Stream for RpcStream<'_, M> {
    type Item = M;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.0.poll_next_unpin(cx)
    }
}

/// A sink that writes messages to the RPC socket
///
/// Any message that fails to be written will be logged and tossed. Users of
/// this sink don't have to handle any errors themselves.
pub struct RpcSink<'a, M>(Pin<Box<dyn 'a + Sink<M, Error = anyhow::Error>>>);

impl<M> Sink<M> for RpcSink<'_, M> {
    type Error = anyhow::Error;

    fn poll_ready(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready_unpin(cx)
    }

    fn start_send(
        mut self: Pin<&mut Self>,
        item: M,
    ) -> Result<(), Self::Error> {
        self.0.start_send_unpin(item)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.0.poll_flush_unpin(cx)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.0.poll_close_unpin(cx)
    }
}

/// Error: socket was closed when we expected as message
#[derive(Debug, Error)]
#[error("Socket closed")]
struct SocketClosed;
