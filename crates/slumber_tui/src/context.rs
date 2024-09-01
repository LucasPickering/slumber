use crate::{input::InputEngine, view::Styles};
use slumber_config::Config;
use slumber_core::http::HttpEngine;
use std::sync::OnceLock;

/// The singleton value for the context. Initialized once during startup, then
/// freely available *read only* everywhere.
static INSTANCE: OnceLock<TuiContext> = OnceLock::new();

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
    /// Visual styles, derived from the theme
    pub styles: Styles,
    /// Input:action bindings
    pub input_engine: InputEngine,
    /// For sending HTTP requests
    pub http_engine: HttpEngine,
}

impl TuiContext {
    /// Initialize global context. Should be called only once, during startup.
    pub fn init(config: Config) {
        INSTANCE
            .set(Self::new(config))
            .expect("Global context is already initialized");
    }

    /// Initialize the global context for tests. This will use a default config,
    /// and if the context is already initialized, do nothing.
    #[cfg(test)]
    pub fn init_test() {
        INSTANCE.get_or_init(|| Self::new(Config::default()));
    }

    fn new(config: Config) -> Self {
        let styles = Styles::new(&config.theme);
        let input_engine = InputEngine::new(config.input_bindings.clone());
        let http_engine = HttpEngine::new(&config.http);
        Self {
            config,
            styles,
            input_engine,
            http_engine,
        }
    }

    /// Get a reference to the global context
    pub fn get() -> &'static Self {
        // Right now the theme isn't configurable so this is fine. To make it
        // configurable we'll need to populate the static value during startup
        INSTANCE.get().expect("Global context is not initialized")
    }
}
