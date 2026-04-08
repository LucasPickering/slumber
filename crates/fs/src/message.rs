//! The message socket allows slumber_fs processes to communicate with each
//! other. There is a single long-lived server process and many short-lived
//! client processes. The clients trigger events such as sending requests or
//! mounting new collections. Server and client communicate over a two-way UDS
//! socket.

use crate::{FilesystemServer, http::RequestState};
use anyhow::Context as _;
use futures::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use slumber_core::{
    collection::RecipeId, database::CollectionId, util::MaybeStr,
};
use slumber_util::{ResultTracedAnyhow, paths};
use std::{error::Error, fmt::Debug, fs, marker::PhantomData, path::PathBuf};
use tokio::{
    net::{UnixListener, UnixStream},
    task,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{error, info};

/// TODO
pub struct ServerListener {
    listener: UnixListener,
}

impl ServerListener {
    /// Bind to the UDS socket
    pub fn bind() -> anyhow::Result<Self> {
        let socket_path = SocketStream::socket_path();
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
    pub async fn listen(self, handler: FilesystemServer) {
        // TODO use a tracing span.
        info!("Socket: listening for clients");
        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => {
                    // Communicate in a subtask so we can handle multiple
                    // clients simultaneously
                    info!(?addr, "Socket: client connected");
                    let stream = ServerStream::new(stream, handler.clone());
                    task::spawn_local(stream.handle());
                }
                Err(error) => {
                    error!(
                        error = &error as &dyn Error,
                        "Socket: error connecting to client"
                    );
                }
            }
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
enum ClientMessage {
    /// Trigger an HTTP request
    SendRequest {
        collection_id: CollectionId,
        recipe_id: RecipeId,
    },
}

/// The server end of a UDS connection
///
/// `State` is a type state parameter denoting the kind of conversation being
/// had. It starts as [StateNew], meaning no open conversation. The initial
/// message sent defines the types of messages that can be sent/received
/// subsequently.
pub struct ServerStream<State> {
    stream: SocketStream,
    handler: FilesystemServer,
    type_state: PhantomData<State>,
}

impl ServerStream<StateNew> {
    fn new(stream: UnixStream, handler: FilesystemServer) -> Self {
        Self {
            stream: SocketStream::new(stream),
            handler,
            type_state: PhantomData,
        }
    }

    /// Handle a conversation with the connected client
    ///
    /// This will read the initial message, initiate some action in the service,
    /// and continue the conversation as needed.
    async fn handle(mut self) {
        // Read the initial message to determine the scope of the conversation
        let Some(message) =
            self.stream.read::<ClientMessage>().await.expect("TODO")
        else {
            return;
        };

        // Call the appropriate receiver based on the message type
        match message {
            ClientMessage::SendRequest {
                collection_id,
                recipe_id,
            } => {
                // Forward state updates back over the socket to the client
                self.handler
                    .send_request(collection_id, recipe_id, async |message| {
                        self.stream.write(message).await.expect("TODO");
                    })
                    .await;
            }
        }
    }
}

/// The client end of a UDS connection
///
/// Use this to send messages from short-lived clients to the server.
///
/// `State` is a type state parameter denoting the kind of conversation being
/// had. It starts as [StateNew], meaning no open conversation. The initial
/// message sent defines the types of messages that can be sent/received
/// subsequently.
pub struct ClientStream<State> {
    stream: SocketStream,
    type_state: PhantomData<State>,
}

/// Initial type state for [ServerStream]/[ClientStream]
pub struct StateNew;

impl ClientStream<StateNew> {
    /// Open a connection with the server
    pub async fn connect() -> anyhow::Result<Self> {
        let socket_path = SocketStream::socket_path();
        let stream =
            UnixStream::connect(&socket_path).await.with_context(|| {
                format!("Error connecting to socket {}", socket_path.display())
            })?;
        info!(?socket_path, "Connected to server");
        Ok(Self {
            stream: SocketStream::new(stream),
            type_state: PhantomData,
        })
    }

    /// Tell the server to send a request
    ///
    /// This begins a conversation where the server sends state updates about
    /// the request, and the client can respond to prompts as needed.
    pub async fn send_request(
        mut self,
        collection_id: CollectionId,
        recipe_id: RecipeId,
    ) -> anyhow::Result<ClientStream<StateRequest>> {
        let message = ClientMessage::SendRequest {
            collection_id,
            recipe_id,
        };
        self.stream.write(message).await?;
        Ok(self.into_state())
    }

    /// Transform the type state parameter
    fn into_state<State>(self) -> ClientStream<State> {
        ClientStream {
            stream: self.stream,
            type_state: PhantomData,
        }
    }
}

/// Type state for [ServerStream]/[ClientStream] when sending an HTTP request
pub struct StateRequest;

impl ClientStream<StateRequest> {
    /// Listen for a request state updates from the server
    ///
    /// Blocks until a message is received from the server. Return `Ok(None)`
    /// if the stream is closed.
    pub async fn listen(&mut self) -> anyhow::Result<Option<RequestState>> {
        self.stream.read::<RequestState>().await
    }
}

/// Wrapper for [UnixStream] to handle encoding/decoding of messages
struct SocketStream {
    /// A unified Stream/Sink for reading and writing messages. This uses a
    /// length header to delimit each frame (message).
    transport: Framed<UnixStream, LengthDelimitedCodec>,
}

impl SocketStream {
    /// Wrap a stream for convenneniencene
    fn new(stream: UnixStream) -> Self {
        let stream = Framed::new(stream, LengthDelimitedCodec::new());
        Self { transport: stream }
    }

    /// Get the path to the UDS file
    ///
    /// Since there is only a single system-wide server, there is only one
    /// possible socket path.
    fn socket_path() -> PathBuf {
        paths::data_directory().join("slumber.sock")
    }

    /// Read one message from the stream
    ///
    /// Return `Ok(None)` if the stream is closed.
    async fn read<M>(&mut self) -> anyhow::Result<Option<M>>
    where
        M: for<'de> Deserialize<'de> + Debug,
    {
        let frame = self
            .transport
            .next()
            .await
            .transpose()
            .context("Socket read error")?;

        match frame {
            None => Ok(None),
            Some(frame) => {
                let message = serde_json::from_slice::<M>(&frame)
                    .with_context(|| {
                        format!("Invalid client message {}", MaybeStr(&frame))
                    })
                    .traced()?;
                info!(?message, "Received message");
                Ok(Some(message))
            }
        }
    }

    /// Send one message to the stream
    async fn write<M>(&mut self, message: M) -> anyhow::Result<()>
    where
        M: Serialize + Debug,
    {
        info!(?message, "Sending message");
        let data = serde_json::to_vec(&message).expect("TODO");
        self.transport.send(data.into()).await.expect("TODO");
        Ok(())
    }
}
