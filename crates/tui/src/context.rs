use crate::input::InputEngine;
use slumber_config::Config;
use std::sync::{Arc, OnceLock};

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
    /// Input:action bindings
    pub input_engine: InputEngine,
}

impl TuiContext {
    /// Initialize global context. Should be called only once, during startup.
    pub fn init(config: Arc<Config>) {
        // This *should* panic if the thing is already set, but I disabled that
        // when adding integration tests. Need to figure out an alternative to
        // this.
        // TODO re-enable panic or fix this some other way
        let _ = INSTANCE.set(Self::new(config));
    }

    /// Initialize the global context for tests. This will use a default config,
    /// and if the context is already initialized, do nothing.
    #[cfg(test)]
    pub fn init_test() {
        INSTANCE.get_or_init(|| Self::new(Config::default().into()));
    }

    fn new(config: Arc<Config>) -> Self {
        let input_engine = InputEngine::new(config.tui.input_bindings.clone());

        Self { input_engine }
    }

    /// Get a reference to the global context
    pub fn get() -> &'static Self {
        // Right now the theme isn't configurable so this is fine. To make it
        // configurable we'll need to populate the static value during startup
        INSTANCE.get().expect("Global context is not initialized")
    }
}
