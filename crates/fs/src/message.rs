//! The message socket allows slumber_fs processes to communicate with each
//! other. There is a single long-lived server process and many short-lived
//! client processes. The clients trigger events such as sending requests or
//! mounting new collections. Server and client communicate over a two-way UDS
//! socket.

use crate::{
    FilesystemServer,
    http::{PromptId, PromptSelect, PromptText},
};
use anyhow::Context as _;
use chrono::{DateTime, Utc};
use futures::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use slumber_core::{
    collection::RecipeId, database::CollectionId, http::ExchangeSummary,
    util::MaybeStr,
};
use slumber_template::Value;
use slumber_util::{ResultTracedAnyhow, paths};
use std::{error::Error, fmt::Debug, fs, marker::PhantomData, path::PathBuf};
use tokio::{
    net::{
        UnixListener, UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    task,
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tracing::{error, info};

/// TODO
pub struct ServerListener {
    listener: UnixListener,
}

impl ServerListener {
    /// Bind to the UDS socket
    pub fn bind() -> anyhow::Result<Self> {
        let socket_path = socket_path();
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
pub enum ClientMessage {
    /// Trigger an HTTP request
    SendRequest {
        collection_id: CollectionId,
        recipe_id: RecipeId,
    },
}

/// The server end of a UDS connection
///
/// `State` is a type state parameter denoting the kind of conversation being
/// had. It starts as [StateInit], meaning no open conversation. The initial
/// message sent defines the types of messages that can be sent/received
/// subsequently.
pub struct ServerStream<State: StreamState> {
    stream: SocketStream<State::ClientMessage, State::ServerMessage>,
    handler: FilesystemServer,
    type_state: PhantomData<State>,
}

impl<State: StreamState> ServerStream<State> {
    /// Send a message to the client
    ///
    /// The sent message *cannot* change the socket state.
    pub async fn send(
        &mut self,
        message: State::ServerMessage,
    ) -> anyhow::Result<()> {
        self.stream.write.write(message).await
    }

    /// TODO
    pub fn split(
        &mut self,
    ) -> (
        &mut SocketRead<State::ClientMessage>,
        &mut SocketWrite<State::ServerMessage>,
    ) {
        (&mut self.stream.read, &mut self.stream.write)
    }
}

impl ServerStream<StateInit> {
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
        let Some(result) = self.stream.read.read().await else {
            return;
        };
        let message = result.expect("TODO");

        // Call the appropriate receiver based on the message type
        match message {
            ClientMessage::SendRequest {
                collection_id,
                recipe_id,
            } => {
                // TODO explain
                let handler = self.handler.clone();
                let stream = self.into_state::<StateRequest>();
                handler.send_request(collection_id, recipe_id, stream).await;
            }
        }
    }

    /// Transform the type state parameter
    fn into_state<State: StreamState>(self) -> ServerStream<State> {
        ServerStream {
            stream: self.stream.into_state(),
            handler: self.handler,
            type_state: PhantomData,
        }
    }
}

// TODO standardize on socket vs stream naming (probably socket)

/// The client end of a UDS connection
///
/// Use this to send messages from short-lived clients to the server.
///
/// `State` is a type state parameter denoting the kind of conversation being
/// had. It starts as [StateInit], meaning no open conversation. The initial
/// message sent defines the types of messages that can be sent/received
/// subsequently.
pub struct ClientStream<State: StreamState> {
    stream: SocketStream<State::ServerMessage, State::ClientMessage>,
    type_state: PhantomData<State>,
}

/// TODO
pub trait StreamState {
    /// TODO
    type ClientMessage: Debug + Serialize + DeserializeOwned;
    /// TODO
    type ServerMessage: Debug + Serialize + DeserializeOwned;
}

impl<State: StreamState> ClientStream<State> {
    /// Listen for the next message from the server
    ///
    /// Blocks until a message is received from the server. Return `Ok(None)`
    /// if the stream is closed.
    pub async fn listen(
        &mut self,
    ) -> Option<anyhow::Result<State::ServerMessage>> {
        self.stream.read.read().await
    }

    /// Send a message to the server
    ///
    /// The sent message *cannot* change the channel state
    pub async fn send(
        &mut self,
        message: State::ClientMessage,
    ) -> anyhow::Result<()> {
        self.stream.write.write(message).await
    }
}

/// Initial type state for [ServerStream]/[ClientStream]
pub struct StateInit;

impl StreamState for StateInit {
    type ClientMessage = ClientMessage;
    // Client starts the conversation; server can't send messages yet
    type ServerMessage = ();
}

impl ClientStream<StateInit> {
    /// Open a connection with the server
    pub async fn connect() -> anyhow::Result<Self> {
        let socket_path = socket_path();
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
        self.send(ClientMessage::SendRequest {
            collection_id,
            recipe_id,
        })
        .await?;
        Ok(self.into_state())
    }

    /// Transform the type state parameter
    fn into_state<State: StreamState>(self) -> ClientStream<State> {
        ClientStream {
            stream: self.stream.into_state(),
            type_state: PhantomData,
        }
    }
}

/// Type state for [ServerStream]/[ClientStream] when sending an HTTP request
///
/// While in this state, the client and server communicate about building and
/// sending an HTTP request. This is bound to a single root request, but
/// other triggered requests can occur within the scope of that root request.
pub struct StateRequest;

impl StreamState for StateRequest {
    type ClientMessage = RequestClientMessage;
    type ServerMessage = RequestServerMessage;
}

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

/// Wrapper for [UnixStream] to handle encoding/decoding of messages
struct SocketStream<RM, WM> {
    /// Stream to read from the socket
    read: SocketRead<RM>,
    /// Sink to write to the socket
    write: SocketWrite<WM>,
}

impl<RM, WM> SocketStream<RM, WM> {
    /// Wrap a stream for convenneniencene
    fn new(stream: UnixStream) -> Self {
        let codec = LengthDelimitedCodec::new();
        let (read, write) = stream.into_split();
        let read = SocketRead {
            stream: FramedRead::new(read, codec.clone()),
            message_type: PhantomData,
        };
        let write = SocketWrite {
            sink: FramedWrite::new(write, codec),
            message_type: PhantomData,
        };
        Self { read, write }
    }

    /// Transform the message type parameters
    fn into_state<RM2, WM2>(self) -> SocketStream<RM2, WM2> {
        SocketStream {
            read: SocketRead {
                stream: self.read.stream,
                message_type: PhantomData,
            },
            write: SocketWrite {
                sink: self.write.sink,
                message_type: PhantomData,
            },
        }
    }
}

/// TODO
pub struct SocketRead<M> {
    stream: FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
    message_type: PhantomData<M>,
}

impl<M> SocketRead<M> {
    /// Read one message from the stream
    ///
    /// Return `None` if the stream is closed.
    pub async fn read(&mut self) -> Option<anyhow::Result<M>>
    where
        M: for<'de> Deserialize<'de> + Debug,
    {
        let result = self.stream.next().await?.context("Socket read error");

        // Parse the message
        Some(result.and_then(|frame| {
            let message = serde_json::from_slice::<M>(&frame)
                .with_context(|| {
                    format!("Invalid client message {}", MaybeStr(&frame))
                })
                .traced()?;
            info!(?message, "Received message");
            Ok(message)
        }))
    }
}

/// TODO
pub struct SocketWrite<M> {
    sink: FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
    message_type: PhantomData<M>,
}

impl<M> SocketWrite<M> {
    /// Send one message to the stream
    pub async fn write(&mut self, message: M) -> anyhow::Result<()>
    where
        M: Serialize + Debug,
    {
        info!(?message, "Sending message");
        let data = serde_json::to_vec(&message)
            .with_context(|| format!("Error encoding message {message:?}"))?;
        self.sink
            .send(data.into())
            .await
            .with_context(|| format!("Error sending message {message:?}"))?;
        Ok(())
    }
}

/// Get the path to the UDS file
///
/// Since there is only a single system-wide server, there is only one
/// possible socket path.
fn socket_path() -> PathBuf {
    paths::data_directory().join("slumber.sock")
}
