mod cereal;
mod error;
mod functions;

use crate::{
    collection::{Collection, LoadedCollection},
    template::TemplateContext,
};
use anyhow::{anyhow, Context};
use petit_js::{function::Function, Engine, Object, Process, Value};
use std::{future::Future, path::Path, sync::Arc};
use tokio::{
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task,
};
use tracing::{info, info_span};

/// TODO
#[derive(Clone, derive_more::Debug)]
pub struct JsEngine {
    engine: petit_js::Engine,
}

impl JsEngine {
    /// TODO
    pub fn new() -> Self {
        let _ = info_span!("Initializing JS engine").entered();
        let mut engine = Engine::new();
        functions::register_all(&mut engine);
        Self { engine }
    }

    /// Load a recipe collection from a JS file. Also return the process that
    /// it was loaded from, so we can execute further functions
    pub fn load_collection(
        &self,
        path: &Path,
    ) -> impl Future<Output = anyhow::Result<LoadedCollection>> {
        let path = path.to_owned();
        info!(?path, "Loading collection file");
        // Parse the file outside the thread so we can drop the engine ref
        let process_result = self.engine.parse(&*path);

        async move {
            task::spawn_blocking(move || {
                let process = process_result?;
                let exports = process.execute()?;
                // Default exported value should be the collection
                let value = exports
                    .default
                    .ok_or_else(|| anyhow!("Collection not exported TODO"))?;
                let collection: Collection = serde_path_to_error::deserialize(
                    value.into_deserializer(),
                )?;

                Ok::<_, anyhow::Error>(LoadedCollection {
                    process,
                    collection,
                })
            })
            .await
            .expect("TODO")
            .with_context(|| format!("Error loading collection from {path:?}"))
        }
    }
}

impl Default for JsEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// TODO
pub struct RenderQueue {
    process: Process,
    messages_rx: UnboundedReceiver<RenderMessage>,
}

impl RenderQueue {
    /// TODO
    pub fn spawn(self) {
        task::spawn_local(async move {
            let Self {
                mut process,
                mut messages_rx,
            } = self;
            while let Some(message) = messages_rx.recv().await {
                // Get the current profile as a JS object
                let profile = match message.context.profile() {
                    // TODO serialization should be easier than this
                    Some(profile) => {
                        petit_js::serde::to_value(profile).expect("TODO")
                    }
                    None => Object::default().into(),
                };
                process.set_app_data(message.context).expect("TODO");
                let return_value =
                    process.call(&message.function, &[profile]).context("TODO");
                message.channel.send(return_value).expect("TODO");
            }
        });
    }
}

/// TODO
#[derive(Clone, Debug)]
pub struct RenderQueueHandle(UnboundedSender<RenderMessage>);

impl RenderQueueHandle {
    pub async fn render(
        &self,
        function: Function,
        context: Arc<TemplateContext>,
    ) -> anyhow::Result<Value> {
        let (tx, rx) = oneshot::channel();
        self.0
            .send(RenderMessage {
                function,
                context,
                channel: tx,
            })
            .context("TODO")?;
        rx.await?
    }
}

/// TODO
struct RenderMessage {
    function: Function,
    context: Arc<TemplateContext>,
    /// Channel that the render queue will reply on
    channel: oneshot::Sender<anyhow::Result<Value>>,
}

// TODO newtype for sender?
