use crate::tui::{
    input::{Action, InputBinding},
    view::Theme,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use slumber_core::config::Config;

pub type TuiConfig = Config<TuiConfigExtension>;

/// TODO
#[derive(Debug, Serialize, Deserialize)]
pub struct TuiConfigExtension {
    /// Should templates be rendered inline in the UI, or should we show the
    /// raw text?
    pub preview_templates: bool,
    /// Overrides for default key bindings
    pub input_bindings: IndexMap<Action, InputBinding>,
    /// Visual configuration for the TUI (e.g. colors)
    pub theme: Theme,
}

impl Default for TuiConfigExtension {
    fn default() -> Self {
        Self {
            preview_templates: true,
            input_bindings: IndexMap::default(),
            theme: Theme::default(),
        }
    }
}
