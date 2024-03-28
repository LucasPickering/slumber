use crate::{
    config::Config,
    db::CollectionDatabase,
    tui::{
        input::InputEngine,
        message::{Message, MessageSender},
        view::Theme,
    },
};
use std::sync::OnceLock;

/// The singleton value for the theme. Initialized once during startup, then
/// freely available *read only* everywhere.
static CONTEXT: OnceLock<TuiContext> = OnceLock::new();

/// Globally available context for the TUI. This is initialized once during
/// **TUI** creation (not view creation), meaning there is only one per session.
/// Data that can change through the lifespan of the process, e.g. by user input
/// or collection reload, should *not* go in here.
///
/// The purpose of this is to make it easy for components in the view to access
/// global data without needing to drill it all down the tree. This is purely
/// for convenience. The invariants that make this work are simple and easy to
/// enforce.
///
/// Context data falls into two categories:
/// - Read-only
/// - Concurrently modifiable
/// Both are safe to access in statics!
#[derive(Debug)]
pub struct TuiContext {
    /// App-level configuration
    pub config: Config,
    /// Visual theme. Colors!
    pub theme: Theme,
    /// Input:action bindings
    pub input_engine: InputEngine,
    /// Async message queue. Used to trigger async tasks and mutations from the
    /// view.
    pub messages_tx: MessageSender,
    /// Persistence database
    pub database: CollectionDatabase,
}

impl TuiContext {
    /// Initialize global context. Should be called only once, during startup.
    pub fn init(
        config: Config,
        messages_tx: MessageSender,
        database: CollectionDatabase,
    ) {
        let input_engine = InputEngine::new(config.input_bindings.clone());
        CONTEXT
            .set(Self {
                config,
                theme: Theme::default(),
                input_engine,
                messages_tx,
                database,
            })
            .expect("Global context is already initialized");
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

/// Test fixture for using context. This will initialize it once for all tests
#[cfg(test)]
#[rstest::fixture]
#[once]
pub fn tui_context() {
    use tokio::sync::mpsc;
    TuiContext::init(
        Config::default(),
        MessageSender::new(mpsc::unbounded_channel().0),
        CollectionDatabase::testing(),
    );
}
