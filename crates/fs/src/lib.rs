//! TODO

use crate::filesystem::{CollectionFilesystem, Context};
use slumber_core::collection::RecipeId;
use std::path::PathBuf;
use tokio::{
    select,
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
};

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
    let filesystem =
        CollectionFilesystem::new(collection_path, mount_path, tx)?;

    select! {
        result = handle_messages(rx) => result,
        result = filesystem.spawn() => result,
    }
}

/// TODO
enum Message {
    /// Trigger an HTTP request
    SendRequest { recipe_id: RecipeId },
}

async fn handle_messages(mut rx: MessagesRx) -> anyhow::Result<()> {
    loop {
        let Some(message) = rx.recv().await else {
            return Ok(());
        };
        match message {
            Message::SendRequest { recipe_id } => todo!(),
        }
    }
}
