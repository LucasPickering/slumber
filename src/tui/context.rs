use crate::tui::{
    input::InputEngine,
    message::{Message, MessageSender},
    view::Theme,
};
use std::sync::OnceLock;

/// The singleton value for the theme. Initialized once during startup, then
/// freely available *read only* everywhere.
static CONTEXT: OnceLock<TuiContext> = OnceLock::new();

/// Globally available read-only context for the TUI. This is initialized
/// once during **TUI** creation (not view creation), meaning there is only
/// one per session. Data that can change through the lifespan of the process,
/// e.g. by user input or collection reload, should *not* go in here.
///
/// The purpose of this is to make it easy for components in the view to access
/// global data without needing to drill it all down the tree. This is purely
/// for convenience. The invariants that make this work are simple andeasy to
/// enforce.
#[derive(Debug)]
pub struct TuiContext {
    /// Visual theme. Colors!
    pub theme: Theme,
    /// Input:action bindings
    pub input_engine: InputEngine,
    /// Async message queue. Used to trigger async tasks and mutations from the
    /// view.
    pub messages_tx: MessageSender,
}

impl TuiContext {
    /// Initialize global context. Should be called only once, during startup.
    pub fn init(messages_tx: MessageSender) {
        CONTEXT
            .set(Self {
                theme: Theme::default(),
                input_engine: InputEngine::default(),
                messages_tx,
            })
            .expect("Global context is already initialized");
    }

    #[cfg(test)]
    pub fn init_test() {
        use tokio::sync::mpsc;
        Self::init(MessageSender::new(mpsc::unbounded_channel().0))
    }

    /// Get a reference to the global context
    pub fn get() -> &'static Self {
        // Right now the theme isn't configurable so this is fine. To make it
        // configurable we'll need to populate the static value during startup
        CONTEXT.get().expect("Global context is not initialized")
    }

    /// Send a message to trigger an async action
    pub fn send_message(message: Message) {
        Self::get().messages_tx.send(message);
    }
}
