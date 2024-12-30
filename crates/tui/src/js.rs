use slumber_core::{
    collection::FunctionId,
    js::{JsRuntime, Renderer},
    template::TemplateContext,
};
use std::sync::Arc;
use tokio::{
    runtime::Handle,
    sync::{
        mpsc::{self, UnboundedSender},
        oneshot,
    },
    task::LocalSet,
};

// TODO rename module

/// TODO
pub fn run() -> RenderQueue {
    let (tx, mut rx) = mpsc::unbounded_channel::<RenderMessage>();

    // TODO explain
    let tokio_runtime = Handle::current();
    std::thread::spawn(move || {
        let local = LocalSet::new();

        local.spawn_local(async move {
            let runtime = JsRuntime::new();
            while let Some(message) = rx.recv().await {
                let output = String::new(); // TODO
                message.channel.send(Ok(output));
            }
        });

        // This will return once all senders are dropped and all
        // spawned tasks have returned.
        tokio_runtime.block_on(local);
    });

    RenderQueue { messages_tx: tx }
}

#[derive(Debug)]
pub struct RenderQueue {
    messages_tx: UnboundedSender<RenderMessage>,
}

impl RenderQueue {
    pub fn renderer(&self, context: TemplateContext) -> BackgroundRenderer {
        BackgroundRenderer {
            context: context.into(),
            messages_tx: self.messages_tx.clone(),
        }
    }
}

struct RenderMessage {
    function_id: FunctionId,
    context: Arc<TemplateContext>,
    channel: oneshot::Sender<anyhow::Result<String>>,
}

pub struct BackgroundRenderer {
    context: Arc<TemplateContext>,
    messages_tx: UnboundedSender<RenderMessage>,
}

impl Renderer for BackgroundRenderer {
    async fn render_function(
        &self,
        function_id: &FunctionId,
    ) -> anyhow::Result<String> {
        let (tx, rx) = oneshot::channel();
        self.messages_tx
            .send(RenderMessage {
                function_id: *function_id,
                context: Arc::clone(&self.context),
                channel: tx,
            })
            .expect("TODO");
        rx.await.expect("TODO")
    }

    fn context(&self) -> &TemplateContext {
        &self.context
    }
}
