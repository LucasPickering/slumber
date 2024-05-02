use crate::{
    config::Config,
    db::CollectionDatabase,
    http::HttpEngine,
    tui::{input::InputEngine, view::Theme},
};
use std::sync::OnceLock;

/// The singleton value for the context. Initialized once during startup, then
/// freely available *read only* everywhere.
static CONTEXT: OnceLock<TuiContext> = OnceLock::new();

/// Globally available context for the TUI. This is initialized once during
/// **TUI** creation (not view creation), meaning there is only one per session.
/// Data that can change through the lifespan of the process, e.g. by user input
/// or collection reload, should *not* go in here.
///
/// The purpose of this is to make it easy for components in the view to access
/// **read-only** global data without needing to drill it all down the tree.
/// This is purely for convenience.
#[derive(Debug)]
pub struct TuiContext {
    /// App-level configuration
    pub config: Config,
    /// Visual theme. Colors!
    pub theme: Theme,
    /// Input:action bindings
    pub input_engine: InputEngine,
    /// For sending HTTP requests
    pub http_engine: HttpEngine,
    /// Persistence database. The TUI only ever needs to run DB ops related to
    /// our collection, so we can use a collection-restricted DB handle
    pub database: CollectionDatabase,
}

impl TuiContext {
    /// Initialize global context. Should be called only once, during startup.
    pub fn init(config: Config, database: CollectionDatabase) {
        let input_engine = InputEngine::new(config.input_bindings.clone());
        let http_engine = HttpEngine::new(&config, database.clone());
        CONTEXT
            .set(Self {
                config,
                theme: Theme::default(),
                input_engine,
                http_engine,
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
}
