//! Utilities for building and sending HTTP requests

use crate::message::{
    RequestClientMessage, RequestServerMessage, ServerStream, SocketRead,
    SocketWrite, StateRequest,
};
use anyhow::{Context as _, anyhow, bail};
use async_trait::async_trait;
use futures::future;
use serde::{Deserialize, Serialize};
use slumber_core::{
    collection::{ProfileId, RecipeId},
    database::CollectionDatabase,
    http::{
        Exchange, HttpEngine, RequestSeed, StoredRequestError,
        TriggeredRequestError,
    },
    render::{HttpProvider, Prompter, SelectOption, TemplateContext},
};
use slumber_template::Value;
use slumber_util::ResultTracedAnyhow;
use std::collections::HashMap;
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

/// TODO
#[derive(Debug)]
pub struct FilesystemHttpProvider {
    database: CollectionDatabase,
    http_engine: HttpEngine,
}

impl FilesystemHttpProvider {
    pub fn new(database: CollectionDatabase, http_engine: HttpEngine) -> Self {
        Self {
            database,
            http_engine,
        }
    }
}

#[async_trait(?Send)]
impl HttpProvider for FilesystemHttpProvider {
    async fn get_latest_request(
        &self,
        profile_id: Option<&ProfileId>,
        recipe_id: &RecipeId,
    ) -> Result<Option<Exchange>, StoredRequestError> {
        self.database
            .get_latest_request(profile_id.into(), recipe_id)
            .map_err(StoredRequestError::new)
    }

    async fn send_request(
        &self,
        seed: RequestSeed,
        template_context: &TemplateContext,
    ) -> Result<Exchange, TriggeredRequestError> {
        let ticket = self.http_engine.build(seed, template_context).await?;
        let exchange = ticket.send(Some(self.database.clone())).await?;
        Ok(exchange)
    }
}

/// TODO
pub fn prompter() -> (FilesystemPrompter, PromptMultiplexer) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        FilesystemPrompter { tx },
        PromptMultiplexer {
            rx,
            tx: Mutex::default(),
        },
    )
}

/// TODO
pub struct PromptMultiplexer {
    /// Receiver channel for prompt requests from the template engine
    rx: mpsc::UnboundedReceiver<PromptChannel>,
    /// Reply channels for pending prompts
    ///
    /// Mutex is needed so the two directions can insert/remove concurrently.
    tx: Mutex<HashMap<PromptId, PromptReplyChannel>>,
}

impl PromptMultiplexer {
    /// TODO
    pub async fn multiplex(mut self, socket: &mut ServerStream<StateRequest>) {
        let (socket_rx, socket_tx) = socket.split();
        future::join(
            Self::server_to_client(socket_tx, &mut self.rx, &self.tx),
            Self::client_to_server(socket_rx, &self.tx),
        )
        .await;
    }

    /// Receive messages from the template engine prompter via the mpsc channel
    /// and forward them to the client over the socket
    async fn server_to_client(
        stream: &mut SocketWrite<RequestServerMessage>,
        mpsc_rx: &mut mpsc::UnboundedReceiver<PromptChannel>,
        tx: &Mutex<HashMap<PromptId, PromptReplyChannel>>,
    ) {
        // This loop runs as long as there's at least one tx alive. Once the
        // request render is done, the template context and its contained
        // prompter are dropped, and this loop is killed.
        while let Some(prompt) = mpsc_rx.recv().await {
            let id = PromptId::new();
            let message = match prompt {
                PromptChannel::Text { prompt, channel } => {
                    tx.lock()
                        .await
                        .insert(id, PromptReplyChannel::Text(channel));
                    RequestServerMessage::PromptText { id, prompt }
                }
                PromptChannel::Select { prompt, channel } => {
                    tx.lock()
                        .await
                        .insert(id, PromptReplyChannel::Select(channel));
                    RequestServerMessage::PromptSelect { id, prompt }
                }
            };
            let _ = stream.write(message).await.traced();
        }
    }

    /// Receive messages from the client over the socket and forward them back
    /// to the template prompter via the stored oneshot channels
    async fn client_to_server(
        stream: &mut SocketRead<RequestClientMessage>,
        tx_map: &Mutex<HashMap<PromptId, PromptReplyChannel>>,
    ) {
        while let Some(result) = stream.read().await {
            let mut tx_map = tx_map.lock().await;
            let _ = Self::handle_client_message(&mut tx_map, result)
                .context("Error handling client message")
                .traced();
        }
    }

    /// TODO
    fn handle_client_message(
        tx_map: &mut HashMap<PromptId, PromptReplyChannel>,
        result: anyhow::Result<RequestClientMessage>,
    ) -> anyhow::Result<()> {
        let message = result?;
        let mut get_tx = |id| {
            tx_map
                .remove(&id)
                .ok_or_else(|| anyhow!("Unknown prompt ID {id:?}"))
        };

        #[expect(clippy::match_wildcard_for_single_variants)]
        match message {
            // Remove the tx from the map BEFORE checking if the reply was none,
            // because we want to drop the tx to indicate that
            RequestClientMessage::PromptTextReply { id, reply } => {
                let tx = get_tx(id)?;
                if let Some(reply) = reply {
                    let tx = match tx {
                        PromptReplyChannel::Text(tx) => tx,
                        other => bail!(
                            "Expected text prompt, but received {other:?}"
                        ),
                    };
                    tx.send(reply).map_err(|_| anyhow!("Send error"))?;
                }
                Ok(())
            }
            RequestClientMessage::PromptSelectReply { id, reply } => {
                let tx = get_tx(id)?;
                if let Some(reply) = reply {
                    let tx = match tx {
                        PromptReplyChannel::Select(tx) => tx,
                        other => bail!(
                            "Expected select prompt, but received {other:?}"
                        ),
                    };
                    tx.send(reply).map_err(|_| anyhow!("Send error"))?;
                }
                Ok(())
            }
        }
    }
}

/// TODO
#[derive(Debug)]
enum PromptReplyChannel {
    Text(oneshot::Sender<String>),
    Select(oneshot::Sender<Value>),
}

/// TODO
#[derive(Debug)]
pub struct FilesystemPrompter {
    tx: mpsc::UnboundedSender<PromptChannel>,
}

#[async_trait(?Send)]
impl Prompter for FilesystemPrompter {
    async fn prompt_text(
        &self,
        message: String,
        default: Option<String>,
        sensitive: bool,
    ) -> Option<String> {
        // Whole lot of channels going on here! We send the prompt to the mpsc
        // channel, and the multiplexer forwards it over the socket to the
        // client process. The response then comes back over the socket and
        // the muxer replies directly to us via the oneshot channel we gave it.
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(PromptChannel::Text {
                prompt: PromptText {
                    message,
                    default,
                    sensitive,
                },
                channel: tx,
            })
            .ok()?;
        rx.await.ok()
    }

    async fn prompt_select(
        &self,
        message: String,
        options: Vec<SelectOption>,
    ) -> Option<Value> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(PromptChannel::Select {
                prompt: PromptSelect { message, options },
                channel: tx,
            })
            .ok()?;
        rx.await.ok()
    }
}

/// TODO
#[derive(Debug, Serialize, Deserialize)]
pub struct PromptText {
    /// Tell the user what we're asking for
    pub message: String,
    /// Value used to pre-populate the text box
    pub default: Option<String>,
    /// Should the value the user is typing be masked? E.g. password input
    pub sensitive: bool,
}

/// TODO
#[derive(Debug, Serialize, Deserialize)]
pub struct PromptSelect {
    /// Tell the user what we're asking for
    pub message: String,
    /// List of choices the user can pick from. This will never be empty.
    pub options: Vec<SelectOption>,
}

/// A unique ID for a prompt
///
/// This allows the multiplexer and client to differentiate between different
/// prompts running currently and sent over the same socket.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct PromptId(Uuid);

impl PromptId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// TODO
/// TODO rename
#[derive(Debug)]
enum PromptChannel {
    /// Ask the user for text input
    Text {
        prompt: PromptText,
        channel: oneshot::Sender<String>,
    },
    /// Ask the user to pick a value from a list
    Select {
        prompt: PromptSelect,
        channel: oneshot::Sender<Value>,
    },
}
